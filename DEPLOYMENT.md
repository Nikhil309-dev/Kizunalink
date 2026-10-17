# Deploying a prebuilt Audium binary on Nexio Host (Pterodactyl)

This guide deploys a **precompiled** Audium executable. The server does not
compile Rust: no Cargo, no Rustup, no C++ compiler, and no CMake are needed on
the host. It is the path to use when the panel image lacks native build tools.

> The egg in `pterodactyl/egg-audium.json` builds from source (`cargo run`).
> That is a **different** deployment path and is what fails on the stock
> `ghcr.io/ptero-eggs/yolks:rust_latest` image (no `c++`/`cmake`). The steps
> below replace the startup command with one that runs the uploaded binary.

> **Automatic (recommended):** instead of uploading anything by hand, import
> [`pterodactyl/egg-audium-binary.json`](./pterodactyl/egg-audium-binary.json).
> Its installer detects the host architecture, downloads **only** the matching
> release binary, verifies the SHA-256, and installs the version-matched
> `config.toml`. There is no `start.sh`: the binary reads the panel's `AUDIUM_*`
> Startup Variables straight from the environment. The rest of this guide is the
> **manual upload** path, useful when you want to pin a specific binary or the
> panel cannot reach GitHub at install time.

## 1. Which asset to download

From the repository's **Releases** page, download a binary asset, **not** the
"Source code" ZIP (a source ZIP is not a compiled binary).

| Asset | When to use |
|---|---|
| **`kizunalink-linux-x86_64-musl`** | **Recommended.** Fully static — no shared libraries, no glibc version requirement. Works on every x86_64 Linux container, including Pterodactyl's Debian 10 images. |
| `kizunalink-linux-amd64` | Standard glibc build. Smaller to link but requires **glibc ≥ 2.34** (Debian 12+/Ubuntu 22.04+). Use only if you know the container is recent. |
| `kizunalink-linux-arm64` | ARM64 hosts (`aarch64`). Same glibc notes as `amd64`. |

Each asset ships with a sibling `.sha256` file. Verify the download:

```bash
sha256sum -c kizunalink-linux-x86_64-musl.sha256
```

Check the actual executable name and linkage:

```bash
file kizunalink-linux-x86_64-musl      # ELF 64-bit LSB, x86-64, statically linked
ldd kizunalink-linux-x86_64-musl       # "statically linked" / "not a dynamic executable"
```

The process that runs is named **`kizunalink-linux-x86_64-musl`** (the binary was
renamed from the crate binary `audium-server`). Use that exact name in the
startup command.

## 2. Upload the executable

In the panel: **File Manager** → navigate to `/home/container/` (the server
volume root) → **Upload** the downloaded file.

Put it directly in `/home/container/` so Pterodactyl's default working directory
(`/home/container`) matches where the process starts.

### Set the executable bit (if needed)

Uploaded files usually keep their permissions, but if the panel strips them the
process will fail with "Permission denied". Enable **SFTP** access in the panel
and fix it from a terminal:

```bash
cd /home/container
chmod +x kizunalink-linux-x86_64-musl
```

If SFTP is unavailable, the egg's install script (or a shell step you control)
can run the same `chmod`.

## 3. Configuration

Audium reads `config.toml` from its working directory, so create
`/home/container/config.toml`. Minimal contents:

```toml
[server]
address = "0.0.0.0"          # must be 0.0.0.0 so the panel can route the port
port = 2333                  # must equal the server's allocation port
authorization = "CHANGE-ME"  # REQUIRED; no default, placeholders rejected

[logging]
level = "info"               # trace | debug | info | warn | error | off
```

Logs always go to the **Console** tab: `INFO`/`DEBUG` on stdout and
`WARN`/`ERROR` on stderr. You do **not** need a `[logging.file]` section — omit
it and Audium logs to the console only. To *also* mirror logs to a rotating file,
add (all fields optional except `path`):

```toml
[logging.file]
enabled = true               # default true when the section exists
path = "/home/container/logs/audium.log"
max_lines = 10000
max_files = 7
```

If you keep the file section, create the directory once: `mkdir -p
/home/container/logs` (via SFTP, or run from a directory whose `logs/` exists).
File logging never replaces console output.

**Do not put the real secret in a file that is committed or shared.** Prefer the
panel variable below; environment variables always override the file.

## 4. Startup command

The egg sets the Startup Command to:

```bash
./audium
```

That is the whole thing — **there is no `start.sh`.** The released binary reads
`AUDIUM_*` environment variables directly, and Pterodactyl puts the panel's
Startup Variables into the process environment, so the panel settings reach the
server with no wrapper script and no file rewriting. Environment variables
override `config.toml`, so the secret and port always win.

If you manage `config.toml` by hand and want to start the binary yourself:

```bash
cd /home/container && ./audium
```

No `cargo`, no `PATH` mangling, and no `AUDIUM_CONFIG_PATH` needed when the
config sits in `/home/container` (the working directory).

> If the egg's startup command is **locked** (greyed out / not editable), you
> must edit the egg itself: **Admin → Nests → Audium → Startup Command**.
> Change the default from the `cargo run ...` line to the command above, then
> restart the server. A locked startup cannot be changed per-server; clone the
> nest or edit the egg as an admin.

### Panel "done" detection

The egg watches console output for `Server listening on`. Audium prints
`Audium Server listening on ...` (older prebuilt binaries print
`KizunaLink Server listening on ...`), so the substring matches both and the
server shows as **running** automatically. If your egg's done-string differs,
set it to `Server listening on`.

## 5. Port and environment variables

### Port

Pterodactyl routes only the server's **primary allocation** port into the
container. Audium must bind that same port.

- Set the egg variable **Port** / `AUDIUM_PORT` to the allocation port.
- Keep `AUDIUM_ADDRESS=0.0.0.0`.

The `AUDIUM_*` environment variables override `config.toml`, so the panel
variables always win:

| Variable | Required | Notes |
|---|---|---|
| `AUDIUM_AUTHORIZATION` | yes | Lavalink password. No default; weak/placeholder values are rejected. |
| `AUDIUM_ADDRESS` | yes | `0.0.0.0`. |
| `AUDIUM_PORT` | yes | Must equal the allocation port. |
| `AUDIUM_LOG_LEVEL` | no | `trace`..`off`. |
| `AUDIUM_RATE_LIMIT_PER_MINUTE` | no | `0` disables. |
| `AUDIUM_METRICS_ENABLED` | no | Prometheus endpoint. |
| `AUDIUM_HTTP_BUF_BYTES` | no | Per-playing-track HTTP prefetch cap, in bytes. Default `524288` (512 KiB). Clamped to `262144`–`67108864`. |
| `AUDIUM_CONFIG_PATH` | no | Absolute path if `config.toml` is not in the working directory. |

Set `AUDIUM_AUTHORIZATION` in the server's Startup variables — the panel injects
it into the process environment at launch. It is never written to a file.

### `AUDIUM_HTTP_BUF_BYTES` (memory tuning)

This is the single biggest lever on memory per concurrent stream, because one
buffer is held for **every playing track**.

The value counts **compressed** HTTP body bytes, not decoded PCM. At the 512 KiB
default and a typical 128 kbit/s stream that is roughly 30 seconds of readahead,
which is ample for ordinary network jitter.

Measured steady-state RSS at 100 concurrent players:

| Value | RSS | Per player |
|---|---|---|
| `2097152` (2 MiB) | 482 MB | 4.52 MB |
| `1048576` (1 MiB) | 377 MB | 3.47 MB |
| **`524288` (512 KiB, default)** | **300 MB** | **2.70 MB** |
| `262144` (256 KiB, minimum) | 298 MB | 2.68 MB |

The curve flattens at 512 KiB — going below it saves nothing measurable, which is
why that is the default and the minimum. Raise the value only if you serve
unusually slow origins and see buffering; it is clamped at 64 MiB so a typo
cannot allocate unbounded memory.


## 6. Missing shared libraries

With the **musl static** binary there is nothing to install; `ldd` reports
`statically linked`.

With the **glibc** `amd64` binary, the only runtime libraries are `libgcc_s.so.1`,
`libm.so.6` and `libc.so.6` — all part of the base OS. If startup fails with:

```
error while loading shared libraries: libc.so.6: version `GLIBC_2.34' not found
```

the container's glibc is too old (Pterodactyl's own `ghcr.io/pterodactyl/yolks:debian`
is Debian 10, glibc 2.28). The egg avoids this by shipping
`ghcr.io/parkervcp/yolks:debian` (glibc 2.41). If you switched to the stock
image, fix it by using `kizunalink-linux-x86_64-musl` instead, or by pointing the
server at a newer image (see §8).

If a non-base library is ever reported missing, install it in the egg's install
script (which runs as root) or via the panel's package manager — but the static
build avoids this entirely.

## 7. Verifying it works

From the panel console you should see:

```
Audium Server starting...
Audium Server listening on http://0.0.0.0:<port>
```

Because the allocation port is usually not published to your own machine, use
the panel's **Console** and the node's internal network. Locally (or over SFTP +
a tunnel) you can check:

```bash
curl http://127.0.0.1:<port>/health          # {"status":"ok",...}
curl -H "Authorization: <secret>" http://127.0.0.1:<port>/v4/info
```

`/health` is unauthenticated; `/v4/*` and `/metrics` require the `Authorization`
header (401 = missing, 403 = wrong).

Your Discord bot then connects to `host:<port>` with the same
`authorization` secret. The node itself does not log into Discord.

## 8. If the container image is old

The startup command runs whatever binary you upload, so the image mainly matters
for the C runtime. Two options:

- **Use the musl static binary.** Recommended; no image change needed.
- **Change the egg's `docker_images`.** Pterodactyl's own
  `ghcr.io/pterodactyl/yolks:debian` is glibc 2.28 and cannot run the ARM64
  binary; the egg therefore ships `ghcr.io/parkervcp/yolks:debian` (glibc 2.41),
  which satisfies both architectures. Apply any change in
  **Admin → Nests → Audium → Docker Images**, then restart.

## 9. Updating the binary

1. Download the new asset (and its `.sha256`) from the Releases page.
2. Verify: `sha256sum -c kizunalink-linux-x86_64-musl.sha256`.
3. Stop the server (prefer the panel's normal Stop; the node drains sessions on
   SIGTERM before exiting).
4. Upload the new file to `/home/container/`, overwriting the old one.
5. `chmod +x` again if the panel reset permissions.
6. Start the server.

Because the deployment never compiled on the host, no `cargo clean`, no toolchain
upgrade and no rebuild are involved — a binary swap is the whole update.

## YouTube PoToken provider (bgutil)

### Why it is needed

YouTube enforces a Proof-of-Origin token (PoToken) on `googlevideo.com` format
URLs. Datacenter and cloud egress IPs (such as VPS providers, container hosts, or
cloud panels) routinely receive `HTTP 403` errors ("Sign in to confirm you're not
a bot") when requesting video streams without a token. Running an external
bgutil provider mints these integrity tokens so format streams can be fetched
reliably without HTTP 403 blocks.

> **Note:** The provider only affects GVS tokens for the `WEB`, `ANDROID`, and
> `IOS` clients. Alternative clients such as `ANDROID_VR` and muxed format
> `itag 18` do not require a PoToken.

### Standalone Docker deployment

To run the provider as a standalone container on the host:

```bash
docker run --name bgutil-pot -d -p 4416:4416 brainicism/bgutil-ytdlp-pot-provider
```

### Docker Compose sidecar usage

In `docker-compose.yml`, run the provider alongside `audium` as a hardened sidecar
service:

```yaml
services:
  audium:
    build: .
    depends_on:
      - bgutil-pot
    # ...

  bgutil-pot:
    image: brainicism/bgutil-ytdlp-pot-provider
    restart: unless-stopped
    # ports:
    #   - "127.0.0.1:4416:4416"
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
```

### Configuring `config.toml`

Point `sources.youtube.pot.url` to your provider:

- **Docker Compose:** `http://bgutil-pot:4416` (reachable across the internal compose network)
- **Standalone:** `http://127.0.0.1:4416` (reachable on the host loopback interface)

```toml
[sources.youtube.pot]
enabled = true
url = "http://bgutil-pot:4416"    # Use "http://127.0.0.1:4416" for standalone host deployments
bypass_cache = false
```

### Proxy interaction

When `sources.youtube.proxy` is configured:

- A **loopback** provider URL (`127.0.0.1` or `localhost`) is automatically bypassed
  (`no_proxy`) so Audium connects directly to the local provider without routing
  through the outbound proxy.
- A **non-loopback** compose hostname (such as `http://bgutil-pot:4416`) is **not**
  treated as loopback and will be sent through the configured proxy. If that proxy
  cannot resolve Docker-internal network hostnames, requests to the provider will
  fail.

Therefore, when using a YouTube proxy, either keep the provider on loopback (e.g.
publish `127.0.0.1:4416:4416` and set `url = "http://127.0.0.1:4416"`) or leave
`sources.youtube.proxy` unset.

## Known limitations

- **Discord playback / DAVE E2EE were not exercised by this deployment guide.**
  The static link does not change those code paths, but voice E2EE only reaches
  readiness when the channel has another member; a bot alone in a channel stays
  silent (see the project README/AGENTS notes). Verify with a second client.
- **The release binary is built from the tagged commit.** Confirm the tag matches
  the behavior you expect; the asset's build metadata (`/v4/info` → `git.commit`)
  records the source commit.
- **Source-install path still requires build tools.** This guide replaces it; if
  you keep the original egg startup, the `rust_latest` image still needs
  `c++`/`cmake` added by the egg install script.
