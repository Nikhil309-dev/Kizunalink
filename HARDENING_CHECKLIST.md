# Hardening checklist (autonomous mission)

Working notes for the production-hardening pass. Not a deliverable; kept in-repo
to preserve state across a long session.

## Baseline
- Branch: `hardening/production-readiness`
- Start SHA: `c1901ea`
- Baseline `cargo test --workspace`: PASS (all suites green at start)

## Verified against real source (not the stale audit doc)
- [x] COMPAT-001 `/players` bare array — RESOLVED, test-pinned
- [x] SEC-001 SSRF: connect-time resolver + redirect re-validation + IP pin — RESOLVED (tests in client.rs)
- [x] SEC-002 empty authorization is rejected — already in config validate()
- [x] SEC-003 blocking DNS moved to spawn_blocking — RESOLVED
- [x] SEC-004 crypto advisories (8 libcrux RUSTSEC ignores) — re-verified
  against the *compiled* graph: hpke-rs resolves with the `libcrux` backend
  feature OFF, so `hpke-rs-libcrux`/`libcrux-kem`/`libcrux-aesgcm`/
  `libcrux-chacha20poly1305` are never compiled or linked (0 fingerprints,
  0 binary symbols); DAVE uses `hpke-rs-rust-crypto`. `libcrux-sha3`/
  `libcrux-secrets` are compiled but the affected APIs are unused (one-shot
  `shake256::<N>`, no incremental XOF, no avx2). `cargo audit` fails with
  `"error: 8 vulnerabilities found!"` when the ignores are removed, proving the
  gate is meaningful. Guarded by `scripts/check_libcrux_backend.py` in CI.
- [x] SEC-005 CI `cargo audit` step ran without `set -o pipefail`, so a failing
  audit piped through `tee` was reported as success — FIXED, plus a regression
  guard (`scripts/check_ci_pipefail.py`) wired into CI.
- [x] PERF-001 same as SEC-003
- [x] ROBBUST-001 panic=unwind + run_guarded — RESOLVED
- [x] COMPAT-002/003 /v4/info version contract — RESOLVED, test-pinned
- [x] PERF-002 mixer re-enable after stop_all — RESOLVED, test-pinned
- [x] Tape pin on pause/resume mid-ramp — FIXED (8c6f674) + E2E harness

## Findings (all resolved)
- [x] LOW/DoS: `configureResuming.timeout` was client-controlled and unbounded.
  A single authenticated socket could request an arbitrarily long resume window,
  pinning the retained `Session` (players, mixer, decoder tasks) for that long.
  Upstream Lavalink clamps to ~1 h. **FIXED** (see below).

## Fixed
- [x] Bound client `configureResuming.timeout` to `MAX_RESUME_TIMEOUT_SECS` (3600).
  `clamp_resume_timeout` + unit test in `kizuna-server/src/api/ws/opcodes.rs`.
  Verified live: a `timeout=999999999` request logs `timeout=3600s`.

## Fresh live verification (HEAD rebuild, Debian + Rust 1.99)
- [x] `cargo build --release --workspace` green; node launches on :2333.
- [x] `/health` unauth 200; `/v4/*` unauth 401, wrong auth 403, correct auth 200.
- [x] `/v4/loadtracks` live YouTube + SoundCloud search return real results.
- [x] WS handshake + `ready`; `play` of an undecodable track yields
  `TrackExceptionEvent` + `TrackEndEvent` (no panic, no hang).
- [x] `stop`/`destroy` on an unknown guild are safe no-ops.
- [x] Session resumption round-trips (`resumed:true`, same session id).
- [x] `cargo test --workspace` **295 pass / 0 fail** (at `83019a9`); clippy `-D warnings`
  clean; fmt clean. (Earlier 255-count snapshot was taken before the delay-line and
  DAVE-version pin tests were added.)
- [x] Live Discord voice/audio E2E PASS at HEAD `3b27f6b`: real bot Yuna joined
  voice, played `jssearch:Kesariya`, DAVE v1 READY, sustained encrypted frames
  (`speak_loop: 1500 ticks, frames_sent=1481`), and the 8/8 pause/resume listen
  gate passed with a live second member present. Evidence:
  `evidence/live-dave-e2e-2026-10-10-rerun/` (token/sessionId redacted).
  Remaining: a two-party **DAVE v2** (protocol ≥ 2) capture — v1 is the current
  Discord default and what was exercised; the vendored `davey` is pinned to the
  v1 MLS version.

## DAVE v2 feasibility (investigated, not actionable yet)
- The official protocol whitepaper (`discord/dave-protocol`) is at *document
  revision* "protocol version 1.1" but still defines only **one wire protocol
  version, v1** - a single MLS parameter set (MLS 1.0, ciphersuite
  `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`, basic credentials). There is no
  specification of a version 2 ciphersuite, capability set, or frame format.
- `identify` advertises `max_dave_protocol_version = DAVE_INITIAL_VERSION = 1`
  (`constants.rs`), matching `davey::DAVE_PROTOCOL_VERSION`. A regression test
  (`advertised_dave_version_is_implementable`) now fails loudly if these drift,
  so the node can never advertise a version its crypto cannot implement.
- `dave_protocol_version_to_ciphersuite` / `..._to_capabilities` in the vendored
  `davey` accept **only** version 1 and reject everything else with
  `UnsupportedProtocolVersion`. Upstream `davey` master is identical (verified
  against the raw `session.rs`): no version 2 yet.
- The node's `DaveHandler` already handles protocol-version *transitions* and
  rejects `> DAVE_INITIAL_VERSION` (`setup_session`, `prepare_transition`,
  `prepare_epoch`), so it degrades safely rather than mis-encrypting.
- Conclusion: supporting DAVE v2 requires a v2 specification **and** a `davey`
  that maps it to an MLS ciphersuite/capabilities set. Until both exist there is
  no safe implementation to write; added a pin test instead of speculative code.

## Additional hardening (this pass)
- [x] `DelayLine::new(0)` no longer panics on `% 0` / underflows on `size - 1`;
  constructor clamps to >= 1 slot (defensive; all current call sites pass
  positive sizes). Unit tests added.
- [x] Advertised DAVE protocol version pinned to the davey-supported version.

## Upstream parity (Lavalink v4 `PlayerRestHandler`) — this pass
Ported the PATCH `/v4/sessions/{id}/players/{guildId}` preconditions and the
`identifier` / `userData` semantics that upstream Lavalink enforces. Diffed
against `lavalink-devs/Lavalink` (`PlayerRestHandler.kt`, protocol `v4` Kotlin).
- [x] **400 on mutually exclusive track fields** — `track` together with the
  deprecated `encodedTrack`/`identifier`, and `encodedTrack` together with
  `identifier` (also inside `track`), now return `400 Bad Request` instead of
  silently preferring one field.
- [x] **400 on non-positive `endTime`** — `endTime: 0` is rejected
  (`End time must be greater than 0`); `endTime: null` still resets it.
- [x] **`identifier` playback implemented** — `identifier` (top-level or inside
  `track`) is resolved to a single playable track via the source manager; a
  playlist, search result, empty result, or resolution error is answered with
  `400` (`Cannot play a playlist or search result` / `No matches found for
  identifier`). Previously the raw identifier was fed into base64 track decoding
  and always failed with a `TrackException`, so `identifier`-based playback never
  worked. Resolution is bounded by a 30 s timeout.
- [x] **`userData` echoed** — a client-supplied `userData` (on a new track or a
  userData-only update) now appears in the echoed `Track` object and in
  `TrackStart`/`TrackEnd` events, matching upstream `newTrack.userData`.
- [x] Regression tests added (`api::rest::tests`): mutual-exclusion, `endTime`
  bounds, unresolvable identifier. `cargo test --workspace` **299 pass / 0 fail**;
  clippy `-D warnings` clean; fmt clean.

## Live Lavalink 4.1.1 parity pass — this pass
Ran a real Lavalink 4.1.1 jar (Lavaplayer 2.2.3) side-by-side with the running
KizunaLink binary, driving both through identical REST + WS flows. Full evidence
in `evidence/parity-lavalink-4.1.1-2026-10-11/` (`COMPARISON.md` + raw logs).
- [x] **`GET /version`** now returns the v4 protocol version (was the crate
  semver `1.1.0`); it and `/v4/info.version.semver` share one `protocol_version()`
  helper and are asserted equal in a test.
- [x] **`preRelease` / `frameStats`** are always serialized (`""` / `null`),
  matching official nodes; previously the keys were omitted.
- [x] **WS handshake requires `User-Id`** — missing/unparseable now returns 400
  like upstream, instead of accepting a session that can never do a voice update.
- [x] Confirmed exact-match parity on the PATCH preconditions, `identifier`
  resolution, volume clamping, and session/player/routeplanner status codes.
- [x] `cargo test --workspace` **303 pass / 0 fail**; clippy `-D warnings` clean;
  fmt clean.


## Performance sanity — this pass
Idle control-plane latency (n=300/endpoint, same host) vs a real Lavalink 4.1.1:
- `GET /v4/info`: KizunaLink 3024 req/s, p99 0.41 ms vs Lavalink 998 req/s, p99 2.78 ms.
- `GET /v4/stats`: KizunaLink 1847 req/s, p99 0.65 ms vs Lavalink 1230 req/s, p99 2.59 ms.
- Idle RSS: KizunaLink ~46 MB vs Lavalink JVM ~344 MB (~7.5x less).
- Raw logs + method in `evidence/parity-lavalink-4.1.1-2026-10-11/` (`raw_perf.txt`).
  Control-plane only; does not exercise the audio/Discord path.
