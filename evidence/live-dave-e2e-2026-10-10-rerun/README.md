# Live Discord voice E2E — re-run at HEAD `3b27f6b`

Captured (UTC): see `captured_at_utc.txt`
Tested commit: see `tested_sha.txt` (`3b27f6bf2fd8de25a7bf90909025dc418c7d5b44`)
Result: **PASS — 8/8** on the real pause/resume listen gate, with a live
DAVE v1 encrypted voice session and sustained frame emission.

## What this proves

A real bot (Yuna, id `1528943321974571098`) joined a real voice channel via
discord.py + wavelink, against this KizunaLink node over the real Lavalink v4
wire protocol, and streamed audio while a second human member
(`kai._.dev`, id `912362112620331029`) was present, so Discord formed the DAVE
(MLS) group.

## Gate (from `harness_transcript.sanitized.log`)

```
PASS :: discord.guild+channel.resolve :: guild='Audium Devlopment Orginisation' channel='Testing playback'
PASS :: lavalink.node.connect :: session_id=qea1zv0rhnq6g0ez
PASS :: discord.voice.connect :: channel='Testing playback'
PASS :: track.play :: identifier=jssearch:Kesariya track='Kesariya'
PASS :: baseline.position_advancing :: 600 -> 2600
PASS :: burst.pause_resume :: paused_ok=True resumed_ok=True
PASS :: post_burst.still_playing :: paused=False track='Kesariya'
PASS :: post_burst.position_advancing :: 4960 -> 7960  (pinned tape = no advance)
RESULT: 8 PASS / 0 FAIL / 0 SKIP
```

The burst hammers `pause(); resume()` with a 120 ms gap so the resume lands
inside the 500 ms tape stop-ramp — the exact race that used to pin `TapeEffect`
at its 0.01 floor (silent playback while the player reported PLAYING). Position
kept advancing afterwards, so the tape recovered.

## Node-side evidence (`node_dave_excerpt.sanitized.log`)

- `Ready: ssrc=2703, mode=aead_aes256_gcm_rtpsize`
- `DAVE adding users: [912362112620331029]`
- `DAVE session setup (v1)` / `DAVE setup context: protocol_version=1, mls_group_id=0`
- `DAVE commit processed (tid 0)` → `DAVE session (v1) is READY`
- Sustained encrypted media: `speak_loop: 1500 ticks, frames_sent=1481 frames_nulled=6`

## Live state (`player_state.sanitized.json`)

`{"state":{"connected":true,"position":41260,"ping":276},"dave":{"protocolVersion":1,"privacyCode":"<redacted>"},"paused":false,"track":"Kesariya"}`

## Sanitization

The voice `token` and `sessionId` are redacted. The bot token is **not** present
in any file here. Guild/channel/user ids and track metadata are retained as
non-secret test fixtures.

## Scope / limitation

This is a **DAVE v1** (protocol version 1) session — Discord's current default.
A two-party **DAVE v2** (protocol ≥ 2) capture remains outstanding; the vendored
`davey` crate is pinned to the v1 MLS version and `setup_session` rejects other
versions by design.
