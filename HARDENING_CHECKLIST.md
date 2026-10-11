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
- [x] PERF-001 same as SEC-003
- [x] ROBBUST-001 panic=unwind + run_guarded — RESOLVED
- [x] COMPAT-002/003 /v4/info version contract — RESOLVED, test-pinned
- [x] PERF-002 mixer re-enable after stop_all — RESOLVED, test-pinned
- [x] Tape pin on pause/resume mid-ramp — FIXED (8c6f674) + E2E harness

## Findings (open)
- [ ] LOW/DoS: `configureResuming.timeout` is client-controlled and was unbounded.
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
- [x] `cargo test --workspace` 255 pass; clippy `-D warnings` clean; fmt clean.
- [x] Live Discord voice/audio E2E PASS at HEAD `3b27f6b`: real bot Yuna joined
  voice, played `jssearch:Kesariya`, DAVE v1 READY, sustained encrypted frames
  (`speak_loop: 1500 ticks, frames_sent=1481`), and the 8/8 pause/resume listen
  gate passed with a live second member present. Evidence:
  `evidence/live-dave-e2e-2026-10-10-rerun/` (token/sessionId redacted).
  Remaining: a two-party **DAVE v2** (protocol ≥ 2) capture — v1 is the current
  Discord default and what was exercised; the vendored `davey` is pinned to the
  v1 MLS version.
