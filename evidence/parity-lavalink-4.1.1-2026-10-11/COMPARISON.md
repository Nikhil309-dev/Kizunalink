# KizunaLink vs Lavalink v4 — reality check (live side-by-side)

Date: 2026-10-11
Reference: **real Lavalink 4.1.1** (`Lavalink.jar`, Lavaplayer 2.2.3, JVM 21) on
`127.0.0.1:2444`, started from the official release jar — not the docs, not a
clone. KizunaLink: `target/release/kizuna-server` on `127.0.0.1:2333`, built
from `hardening/production-readiness`.

Both nodes were driven through the **same** REST calls and the **same** WS
client (identical `Authorization` / `User-Id` / `Client-Name` headers), so every
difference below is the implementation, not the harness.

## Method

- Lavalink source clone for route/schema cross-checks: `lavalink-devs/Lavalink`
  (`docs/api/rest.md`, `docs/api/websocket.md`).
- Sessions created the real way: connect `/v4/websocket`, read the `ready` op.
  (Neither node exposes `POST /v4/sessions` — Lavalink creates sessions on WS
  connect; the earlier "404 on POST /v4/sessions" was **not** a KizunaLink gap.)
- Scripts used: `compare_rest.py`, `compare_patch.py`, `compare_fields.py`,
  `compare_identifier.py`, `compare_ws.py`, `sweep.py`, `verify_fixes.py`.

## Endpoint coverage

Lavalink v4 REST surface (from `docs/api/rest.md`): `GET /version`,
`GET /v4/info`, `GET /v4/stats`, `GET /v4/loadtracks`, `GET /v4/decodetrack`,
`POST /v4/decodetracks`, `GET|PATCH|DELETE /v4/sessions/{id}/players/{guild}`,
`GET /v4/sessions/{id}/players`, `PATCH /v4/sessions/{id}`,
`GET /v4/routeplanner/status`, `POST /v4/routeplanner/free/{address,all}`.

KizunaLink implements **all** of these. Extras beyond the core spec (superset,
not a compatibility break): `GET /v4/loadsearch`, `GET /v4/lyrics`,
`GET /v4/youtube*`, and lyrics/sponsorblock player sub-routes.

## Live status parity (matched unless noted)

| Request | Lavalink 4.1.1 | KizunaLink | Verdict |
|---|---|---|---|
| `GET /v4/info` | 200 | 200 | match (see version below) |
| `GET /v4/stats` | 200 | 200 | match |
| `GET /v4/loadtracks` (no id) | 400 | 400 | match |
| `GET /v4/loadtracks?identifier=<garbage>` | 200 `empty` | 200 `empty` | match |
| `GET /v4/decodetrack` (none) | 400 | 400 | match |
| `PATCH player {track,encodedTrack}` | 400 | 400 | match (exact msg) |
| `PATCH player {encodedTrack,identifier}` | 400 | 400 | match (exact msg) |
| `PATCH player {endTime:0}` | 400 | 400 | match (exact msg) |
| `PATCH player {endTime:null}` | 200 | 200 | match |
| `PATCH player {volume:5000}` | 200 vol=1000 | 200 vol=1000 | match (clamp) |
| `PATCH player {volume:-10}` | 200 vol=0 | 200 vol=0 | match (clamp) |
| `PATCH player {identifier:<resolvable>}` | 200 track | 200 track | match |
| `PATCH player {identifier:<no match>}` | 400 "No matches found for identifier" | 400 same | match (exact msg) |
| `GET players` | 200 `[]` | 200 `[]` | match |
| `GET player` (absent) | 404 "Player not found" | 404 "Player not found for guild: …" | match (status) |
| `DELETE player` | 204 | 204 | match |
| `PATCH session {resuming}` | 200 | 200 | match |
| `GET session` | **405** (no GET) | 200 | superset (KizunaLink adds GET) |
| `GET routeplanner/status` (disabled) | 204 | 204 | match |
| `POST routeplanner/free/*` (disabled) | 500 | 500 | match (status) |

## Gaps found and fixed (commit `84ec517`)

1. **`GET /version` wrong value.** Lavalink returned `"4.1.1"`; KizunaLink
   returned `"1.1.0"` (crate semver). Clients gate on this. Fixed: `/version`
   and `/v4/info.version.semver` now share one `protocol_version()` helper.
2. **Missing `preRelease` / `frameStats` keys.** Lavalink emits
   `"preRelease": ""` and `"frameStats": null`; KizunaLink omitted both
   (`Option` + `skip_serializing_if`). Fixed: always serialized, `""`/`null`.
3. **WS handshake accepted no `User-Id`.** Lavalink rejects with HTTP 400 when
   `User-Id` is missing/unparseable; KizunaLink accepted the connection and
   deferred the failure. Fixed: handshake now returns 400.

Verified live after the fix (`verify_fixes.py`, `verify_userid.py`):

```
GET /version        REF 4.1.1     KIZ 4.1.0     (protocol version, agreement with /v4/info)
GET /v4/info        preRelease "" (both)
GET /v4/stats       frameStats null (both)
WS stats event      frameStats null (both)
WS no User-Id       REF 400        KIZ 400
WS with User-Id     REF ready      KIZ ready
```

## Known residual differences (documented, not bugs)

- **Malformed base64.** Lavalink throws `500` (uncaught Java
  `IllegalArgumentException`); KizunaLink returns `400 "Invalid track encoding"`.
  KizunaLink's behavior is the more correct one; this is a deliberate
  divergence.
- **`GET /v4/sessions/{id}`** is a KizunaLink extension (Lavalink 405).
- **`dave` key** on the player object is a KizunaLink DAVE extension; standard
  clients ignore unknown fields.
- **`/v4/info` fields**: `jvm`/`lavaplayer` are labels (`"Rust"`/`"symphonia"`)
  and `git.*`/`buildTime` are `unknown`/`0` unless the build exports
  `GIT_COMMIT`/`BUILD_TIME`. Schema matches; values are informational only.

## Size (informational)

| | files | LOC |
|---|---|---|
| KizunaLink (`kizunalink` + `kizuna-server`, `*.rs`) | 346 | 61,789 |
| Lavalink (`*.java` + `*.kt`, excl. `build/`) | 81 | 6,860 |

KizunaLink is ~9x the LOC because it reimplements the audio engine, sources,
DAVE/E2EE, and the Lavalink wire protocol natively in Rust; Lavalink delegates
playback to Lavaplayer and networking to JDA.
