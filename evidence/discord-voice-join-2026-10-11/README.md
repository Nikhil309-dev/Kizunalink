# Discord voice-join proof — 2026-10-11

Proves a **real Discord bot** connects to a real voice channel and KizunaLink
plays audio into it (the node's own audio path, not just a REST `200`).

## Setup

- Bot: `Yuna#6520` (`1528943321974571098`), discord.py 2.7.1 + wavelink 3.5.2 +
  PyNaCl 1.6.2.
- Guild: `Audium Devlopment Orginisation` (`1558468405604913235`).
- Channel: `Testing playback` (`1558469631898034246`), with a second human
  member present (`Nik ♪`, `912362112620331029`).
- Node: `kizuna-server` release build on `127.0.0.1:2333`.

The second human matters: Discord only forms the DAVE (E2EE) MLS group when a
non-bot member is in the channel, and the node gates its RTP send loop on DAVE
readiness. With the bot alone the audio path stays silent by design.

## Result 1 — repo e2e (`scripts/e2e_pause_resume.py`)

`8 PASS / 0 FAIL / 0 SKIP`. Log: `e2e_pause_resume.txt`.

```
PASS discord.guild+channel.resolve
PASS lavalink.node.connect            session_id=elj63ahiwhhwt4n6
PASS discord.voice.connect            channel='Testing playback'
PASS track.play                       identifier=jssearch:Kesariya
PASS baseline.position_advancing      920 -> 2920
PASS burst.pause_resume               paused_ok=True resumed_ok=True
PASS post_burst.still_playing         paused=False track='Kesariya'
PASS post_burst.position_advancing    5280 -> 8280
```

`position_advancing` is the real gate: a pinned/stalled tape keeps position fixed
and emits nothing.

## Result 2 — voice path capture (`voice_proof.json`)

`connected=true` and `dave_ready=true` (privacy code present) for every sample;
position advances ~1x realtime; voice ping drops 536 → 282 ms.

| t (s) | position (ms) | ping | dave_ready |
|---|---|---|---|
| 0 | 580 | 536 | true |
| 5 | 10600 | 536 | true |
| 11 | 22620 | 282 | true |

## Note on `frameStats`

`GET /v4/stats` returns `frameStats: null` even while playing. This is **correct
parity**, not a bug: upstream Lavalink's `retrieveStats(context = null)` (the REST
path) also leaves it null and only computes it for the WS stats event, which
carries a socket context. KizunaLink mirrors this exactly.
