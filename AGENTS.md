# AGENTS.md — KizunaLink

Rust-native Lavalink v4 audio node for Discord. Cargo workspace:
`kizunalink` (lib, under `kizunalink/kizuna-voice/`) + `kizuna-server` (axum binary).

## Build & run

```bash
export LIBOPUS_STATIC=1 OPUS_STATIC=1   # required; libopus is linked statically
cargo build --release --workspace        # release: panic="unwind" (decoder catch_unwind), strip=true; ~5 min cold
cargo check --workspace --all-targets
cargo test --workspace                   # 249 tests (+1 ignored soak)
cargo test -p kizuna-server -- --ignored soak --nocapture   # soak harness
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo deny check advisories
cargo audit                             # authoritative advisory gate (see Security)
```

Build metadata (`BUILD_TIME*`, `GIT_COMMIT*` shown in the banner and `/v4/info`)
is read via `option_env!`; release CI and the Dockerfile inject it. Unset env
vars render as `unknown`/`0`, so local builds are unaffected.

The binary loads `config.toml` from the current working directory. Set
`KIZUNA_CONFIG_PATH` to an absolute path to load a config from elsewhere (useful
when the working directory is read-only or shared). Copy
`config.example.toml` to `config.toml`.

`server.authorization` has **no default** and must be set. Startup fails if it is
missing, empty, or a known placeholder (e.g. `youshallnotpass`), on **any** bind
address including loopback. Set a strong, unique secret — preferably via
`KIZUNA_AUTHORIZATION` rather than the file. Secrets are overridable via
`KIZUNA_*` env vars (list in `kizunalink/kizuna-voice/config/mod.rs`), which is
how `docker-compose.yml` injects them. A reverse proxy is not a substitute for
the secret: bind to a private address and keep the secret set.

## Testing against a running node

```bash
KIZUNA_ADDRESS=127.0.0.1 KIZUNA_PORT=2333 KIZUNA_AUTHORIZATION=test-secret-123 ./target/release/kizuna-server
curl -H "Authorization: test-secret-123" http://127.0.0.1:2333/v4/info
```

`/health` is unauthenticated; `/v4/*` and `/metrics` require the `Authorization`
header (401 = missing, 403 = wrong). The WebSocket handshake requires
`Authorization`; `User-Id` (numeric Discord user id) and `Client-Name` are
optional. A connection without a valid `User-Id` is accepted, but voice updates
are dropped (`opcodes.rs` logs and ignores them), so real clients must send it.
`Session-Id` enables resumption. `python3` + `pip install websockets` works for
protocol tests.

`scripts/e2e_pause_resume.py` is the real-client regression for the
pause/resume-mid-ramp silent-playback bug (see Gotchas). It drives discord.py +
wavelink against a live node, then bursts `pause(); resume()` with a gap short
enough that the resume lands inside the 500 ms tape stop-ramp, and gates on the
player still advancing position afterwards. It needs `DISCORD_TOKEN` and a
second member in the voice channel (DAVE must form, or the RTP loop emits
nothing). Run it after any change to `TapeEffect`, `FlowController` or
`Mixer::mix`:

```bash
DISCORD_TOKEN=... GUILD=<id> VOICE_CHANNEL=<id> KIZUNA_AUTHORIZATION=... \
  python3 scripts/e2e_pause_resume.py
```

Exit 0 = gate passed, 1 = regression, 2 = inconclusive (no DAVE-ready audio
path).

## Layout

- `kizuna-server/src/{main.rs, health.rs, tls.rs, monitoring/, server/, api/{rest,ws}/}`
- `kizunalink/kizuna-voice/{config, engine, media, discord, lavalink, common}/`
- `vendor/davey` — vendored DAVE (E2EE) crate, patched via `[patch.crates-io]`

## Security

- Dependency advisories are checked by BOTH `cargo deny check advisories`
  (`deny.toml`) and `cargo audit` (`.cargo/audit.toml`). cargo-deny silently
  skips advisories that constrain only `affected.functions` with no floor/ceiling
  in `[versions]` (currently RUSTSEC-2026-0209/0211/0124/0330/0331), so
  **`cargo audit` is the authoritative gate** — do not trust deny alone.
- All current ignores are transitive `libcrux-*` advisories inside the DAVE/MLS
  crypto stack (`davey → openmls_rust_crypto → hpke-rs → hpke-rs-libcrux →
  libcrux-*`); the exact libcrux version is pinned upstream, so there is no
  upgrade path. Keep the two ignore lists in sync and update the reachability
  comments when the crypto stack is bumped.

## Gotchas

- The WebSocket client op set is intentionally small (`IncomingMessage` in
  `kizunalink/.../lavalink/protocol/opcodes.rs`): `voiceUpdate`, `play`, `stop`,
  `destroy`, `configureResuming`. Unknown ops fail to deserialize and are logged
  and dropped. When adding or changing a client op, mirror it in REST (`PATCH
  /v4/sessions/{id}`) so both transports stay in sync, and add a `ws/tests.rs`
  case.
- **`IncomingMessage` field names must be `#[serde(rename = "...")]`-ed one by
  one.** The enum is internally tagged (`tag = "op"`), and in that form
  `rename_all = "camelCase"` only renames the *variant* names — it does NOT touch
  field names. Lavalink clients send `guildId`/`sessionId`/`channelId`, so without
  the explicit renames every real client frame fails with `missing field
  'guild_id'` and is silently dropped. The unit tests in `opcodes.rs` send the
  exact camelCase wire shapes to catch this; keep them when editing the enum.
- DAVE E2EE: `dave.can_send_media()` gates the RTP send loop, so a session that
  never reaches MLS readiness emits nothing (the bot appears silent). Discord only
  completes the MLS group when the channel has other members; a bot alone in a
  channel may never receive the proposal/commit that makes the session ready. Do
  not conclude "DAVE is broken" from a lone-bot test — put a second client in the
  channel, or check for `DAVE session (v1) is READY` in the logs.
- **"Player says PLAYING but the channel is silent" after a pause/resume.**
  `TapeEffect` (the varispeed stop/start ramp) must never be pinned at its 0.01
  floor while the state is `Playing`. `Mixer::mix` drives the ramp from the
  *target* state plus the current rate (`tape.rate()`), not from the state
  transition edge: a `resume()` that lands inside the stop-ramp window
  (`tape_stop_duration_ms`, default 500 ms — wavelink's `pause(); resume()` does
  exactly this) used to skip the start ramp, so the stop ramp completed, the
  state rolled `Starting -> Playing`, and the tape stayed at 0.01 emitting
  permanent silence. Regression test:
  `engine::mix::mixer::tests::tape_recovers_when_resume_lands_mid_stop_ramp`.
  When debugging silent playback, probe RMS *after* `FlowController::process_frame`
  (i.e. after `tape.process`) — decode/resample/encode can all be healthy and the
  tape will still zero the frame.
- YouTube (the most common source) supports a per-source `proxy` like every other
  source; it is what lets the node work from datacenter IPs that YouTube blocks
  (HTTP 403 from `googlevideo.com`). See `default_playback_clients()` and
  `YouTubeConfig::proxy`. The proxy-aware client is threaded into every YouTube
  client *and* into `YouTubeOAuth`; do not construct a bare
  `reqwest::Client::new()` there or OAuth token refresh will bypass the proxy and
  still leave from the blocked host.
- `loadtracks` `loadType` is a closed set (`track`/`playlist`/`search`/`empty`/
  `error`); there is NO `loadFailed` load type. Use `empty` for "no match" and
  `error` (with a `LoadError` payload) for a provider/transport failure. Do not
  collapse a network/HTTP/parse failure into `empty` — clients treat `empty` as a
  normal "nothing found" and swallow the real cause. `LoadResult::Error` is for
  load-time failures; `TrackEndReason::LoadFailed` is a separate WS event reason.
- A `TrackEnd` after a mid-stream decoder error must be `finished`, not
  `loadFailed`: audio already played, so clients should advance the queue. Only a
  failure before any frame is emitted is `loadFailed`. Both still emit a preceding
  `TrackException` carrying the cause. See `stop_reason_after_error` in `monitor.rs`.
- Regional/Indian catalogs (and any YouTube-blocked case) are best served by
  region-native sources: `jssearch:` (JioSaavn) and `gnsearch:` (Gaana) stream from
  their own CDNs and are unaffected by the `googlevideo.com` 403. JioSaavn/Gaana do
  not resolve a bare ISRC, so they belong in the mirrors list as `%QUERY%`
  providers, not `%ISRC%` providers.
- Release uses `panic = "unwind"` so `AudioProcessor::run_guarded` (a `catch_unwind`
  wrapper around the decoder loop) can isolate a panic on one bad stream instead of
  aborting the node. Do not set `panic = "abort"` — it silently makes the guard dead
  code.
- Every REST handler extracts path/query/JSON through `api::rest::json::{ApiPath,
  ApiQuery, ApiJson}` rather than axum's built-ins, so any rejection (bad body, bad
  query, bad path param) returns the same `KizunaLinkError` JSON. An unmatched path
  hits `rest_fallback`, which also emits that shape. Keep new routes on these
  extractors to preserve the single error contract.
- `engine/filters/*` clamp their own inputs (timescale, echo, reverb, phaser), so
  extreme API filter values are safe; do not assume validation lives in the handler.
- State collection takes player read locks with non-blocking `try_read`; unrelated
  to the network I/O done under the player write lock in the REST/WS handlers.
- `docker-compose.yml` runs the image `read_only`; the default config logs to
  `./logs/kizunalink.log`, so the compose file mounts a `kizuna-logs` volume there.
- `TrackHandle` stores a single `AtomicU64` sample counter; `get_position`,
  `seek` and `Mixer::mix` all update it, so the WS `position` and `/players`
  agree. Both directions of the ms↔samples conversion saturate — the client
  controls `position_ms` via `PATCH /players`, and a wrapping `u64` mul used to
  let one oversized value desync the two. Keep them consistent, not just
  panic-free.
- `configureResuming.timeout` is clamped to `MAX_RESUME_TIMEOUT_SECS` (1 h).
  The retained session (players, mixer, decoder tasks) lives for that window, so
  an unbounded value was a cheap DoS. Upstream Lavalink clamps to the same order
  of magnitude.
- The lavalink protocol codec (`lavalink/protocol/codec`) is a binary decoder on
  client input. String fields are `u16`-length-prefixed (`read_string`), so they
  are bounded at 64 KiB and cannot drive a huge allocation; truncated buffers
  error via `read_exact`. When extending it, keep lengths bounded before
  allocating.
- `/version` and `/v4/info.version.semver` must agree: both come from `api::rest::routes::stats::info::protocol_version()`. `/version` used to return the crate semver, which breaks clients that gate on the Lavalink protocol version. Do not read `CARGO_PKG_VERSION` for either.
- Official Lavalink always emits `preRelease` (`""`) and `frameStats` (`null`) keys on `/v4/info` and `/v4/stats` even when empty, so those `Option`s are serialized unconditionally — do not add `skip_serializing_if`.
- The `/v4/websocket` upgrade requires a parseable `User-Id` header (400 otherwise) and rejects missing/bad `Authorization` (401/403); `Client-Name` is optional. A session created without a user id can never send a voice update.
- A real Lavalink 4.1.1 jar is the parity oracle for this repo, not the docs (the docs are stale on `frameStats` and `POST /v4/sessions`). Run it on a spare port and drive both nodes through identical REST+WS calls; see `evidence/parity-lavalink-4.1.1-2026-10-11/`.
- Audius search returns tracks whose `is_streamable` is `false` (gated/processing). Their `/v1/tracks/{id}/stream` answers 404, so `build_track` drops them and search/playlist/resolve fall through to playable results. A missing `is_streamable` field is treated as streamable — only an explicit `false` is rejected.
- YouTube resolution is rate-limited on datacenter egress: a burst of track loads makes *every* client answer "no playable format" for a few seconds, then it self-heals. `track.rs` retries the whole client set twice with backoff (750 ms, 1.5 s) before emitting `TrackException`; this rides over a brief hiccup but does not defeat a sustained throttle — that needs `sources.youtube.proxy`.
- YouTube returns SABR-only adaptive formats (every adaptive format carries no direct `url`; only muxed itag 18 is playable) when a client advertises an `ANDROID_VR`/`ANDROID`/`IOS` `clientVersion` above ~1.65. KizunaLink has no SABR playback path, so keep those versions at or below the threshold. `ANDROID_VR` is pinned to 1.60.19 / Android 12L / SDK 32 (matching Lavalink `youtube-source`; yt-dlp uses 1.65.10). A unit test in `clients/android_vr.rs` guards the 1.65 ceiling.
- YouTube enforces a **Proof-of-Origin (PoToken)** for GVS format URLs on the WEB/MWEB/ANDROID/IOS clients ("Since 2026.07, intermittent/selective POT enforcement for non-HLS formats"). `sources.youtube.pot` points at a `bgutil-ytdlp-pot-provider` HTTP server; `clients/common.rs` fetches a token **bound to the video id** (the GVS `content` binding, not visitorData) via `pot::fetch_global` and attaches it as `serviceIntegrityDimensions.poToken`. Tokens are cached per video (~4 h). `ANDROID_VR` needs no GVS PoToken and muxed itag 18 needs none either.
- PoToken is verified working (provider `2.0.2` mints tokens bound to the video id). OAuth via the built-in TV device flow is also verified working, but requires `YOUTUBE_OAUTH_CLIENT_SECRET` to be set to the **public** YouTube-on-TV client secret (`SboVhoG9s0rNafixCSGGKXAT`; Google's limited-input-device client, shipped by lavalink-devs/youtube-source, yt-dlp, ytmusicapi). Without it the OAuth path silently no-ops: `refresh_tokens` are loaded but `get_access_token` returns `None` (logged as `YOUTUBE_OAUTH_CLIENT_SECRET is not configured`), and every request goes out unauthenticated.
- **Root cause of the persistent 403s (confirmed by A/B):** the datacenter egress itself. On a Google Compute Engine IP (`AS396982`, e.g. `34.10.175.217`) YouTube *resolves* metadata fine but refuses the CDN stream: first fetches resolve + then `403` after a burst, and past the burst YouTube returns `playabilityStatus=ERROR`/"no playable format" (URLs withheld entirely � nothing a PoToken can authorize). Adding OAuth + PoToken did **not** fix it, and forcing IPv4 did not either (the failing URLs carry `ip=fda3:�`, minted for an IPv6 route the host doesn't even have � `/proc/net/if_inet6` is empty and `disable_ipv6=1`). **Only `sources.youtube.proxy` (clean/residential/mobile egress) fixes this layer.** PoToken + OAuth + proxy is the full combination; the first two are already wired and verified.
- When a proxy is set, the **local** bgutil sidecar must not be reached through it: `pot.rs` builds a `no_proxy()` client for a loopback `pot.url` and keeps the shared proxy-aware client only for remote providers. Without this, enabling `sources.youtube.proxy` silently breaks token minting for a `127.0.0.1` provider.
- **Verified end-to-end source matrix** (real audio, measured as player position advancing over a live voice session � not just `loadtracks` metadata): **JioSaavn, Gaana, Audius, Audiomack, Mixcloud** stream their own audio natively and pass; **Spotify/Apple Music are metadata-only mirrors**, so Spotify passes (mirrors to JioSaavn/Gaana) but Apple Music fails when the mirror list finds no match. **YouTube** resolves metadata but the CDN stream 403s intermittently on this datacenter IP � it can pass a single burst and then fail, so treat it as flaky, not working. **SoundCloud** returns `HTTP 401` (stale client_id) � a credentials issue, not the IP block. Deezer/Qobuz/Yandex/Tidal/NetEase/Bandcamp/Anghami/etc. return `loadType=empty` for search because their credentials are not configured.
- Testing tip: this node uses **string opcodes** (`{"op":"voiceUpdate",...}`, `{"op":"play",...}`, `{"op":"stop",...}`), not Lavalink's numeric opcodes. The WS handshake needs `Authorization`, `User-Id`, and `Client-Name` headers. A test client must **not** create its own `@discordjs/voice` connection � send gateway op-4 yourself and forward the token/endpoint/sessionId to the node, or Discord closes the node's voice gateway with `4006 (Session is no longer valid)`.
