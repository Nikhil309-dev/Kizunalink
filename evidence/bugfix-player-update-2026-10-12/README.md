# KizunaLink — bug hunt & fixes (2026-10-12)

Session artifacts for the player-update cadence fix and the soak-harness fix.
Branch: `fix/player-update-cadence-and-soak`. Commit: `964b2b8`.

## Bug 1 — `playerUpdate` cadence (HIGH, FIXED)

**Symptom.** Clients (the yuna bot running `lavalink-client`) saw the player
position / progress bar frozen at `0:00`.

**Root cause.** `monitor_loop` ticks every **500 ms**, but `start.rs` divided the
configured interval by **20 ms** (the old mixer period):

```rust
let interval_ms = config.update_interval.as_millis().max(20);
let update_every_n = interval_ms.div_ceil(20) as u64;   // 250 for the default 5s
```

`update_every_n` counts *monitor ticks*, so the default 5 s interval produced one
`playerUpdate` every `250 * 500 ms = 125 s`. No config value could get below 25 s
(a 1 s config rounded to `50 * 500 ms`). Clients treating a >5 s gap as a stall
show a frozen bar.

**Fix** — `kizunalink/kizuna-voice/discord/player/manager/start.rs`

```rust
let interval_ms = config.update_interval.as_millis().max(20);
let tick_ms = super::monitor::MONITOR_TICK.as_millis().max(1);
let update_every_n = interval_ms.div_ceil(tick_ms) as u64;
```

`MONITOR_TICK` (`= 500 ms`) is now defined in `manager/monitor.rs` and used both
by the loop's `tokio::time::interval` and the divisor, so the two can no longer
drift apart.

**Regression test.** `update_cadence_matches_configured_interval` pins the
cadence for 1/5/10/30 s.

**Live verification** (local release node, http-source track, WS client):

```
recv op=event type=TrackStartEvent
t+ 4.5  playerUpdate pos=0 conn=False
t+ 9.5  playerUpdate pos=0 conn=False
t+14.5  playerUpdate pos=0 conn=False
t+19.5  playerUpdate pos=0 conn=False
t+24.5  playerUpdate pos=0 conn=False
```

`playerUpdate` now arrives every ~5 s (was: never within 30 s). Position stays
`0` without a voice connection because the mixer is not driven — expected; the
cadence is what this fix addresses.

## Bug 2 — stale soak harness (MEDIUM, FIXED)

`kizuna-server/src/soak_tests.rs::ws_connect` opened the WebSocket without the
`User-Id` header. The upgrade rejects that with `400 Missing or invalid User-Id
header`, so both `#[ignore]` soak tests panicked during setup and never exercised
anything.

**Fix.** Send `User-Id` (and `Client-Name`), matching `api/ws/tests.rs`.

**Verification.**

```
soak ok: 40 cycles x 48 create/destroy, sessions reaped to 0
soak ok: 8 sessions x 16 players, 71808 REST updates in 5.008645949s
test result: ok. 2 passed; 0 failed
```

## Investigated, NOT bugs

- **WS `play` op** is a v3-style extension. Lavalink v4 removed client->server
  `play` in favour of REST `PATCH /v4/sessions/{id}/players/{guild}`; KizunaLink
  retains it for compatibility. `lavalink-client` uses the REST path.
- **REST `PATCH` play** was initially reported as "emitting nothing"; re-tested
  with a sequential client it emits `TrackStartEvent` + `playerUpdate` correctly.
  The earlier miss was an artifact of the ad-hoc probe blocking its own reader.
- **`connected=false` / position `0`** without a voice connection is expected.
- **DAVE E2EE**: a lone bot in a channel never forms the MLS group, so it emits
  no audio; a second member is required (matches the repo's documented gotcha).

## Remaining (informational, from the production audit)

- `TEST-001` (HIGH): no automated end-to-end real-playback / voice / DAVE test in
  CI. Manual evidence only; an integration harness is the recommended next step.

## Gates

- `cargo fmt --all -- --check` — clean
- `cargo clippy --workspace --all-targets` — 0 errors
- `cargo test --workspace` — 263 lib + 41 server pass, 0 fail
- `cargo test -p kizuna-server --release -- --ignored soak` — 2 pass
