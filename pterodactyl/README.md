# Running Audium on Pterodactyl

> **Fastest path — prebuilt static binary, downloaded automatically.**
> Import [`egg-audium-binary.json`](./egg-audium-binary.json) and start. The
> installer runs automatically at install time: it picks the right asset for the
> host architecture, downloads it from GitHub Releases, verifies its SHA-256, and
> writes a `start.sh` wrapper that injects the panel variables into `config.toml`
> and runs the binary. There is **nothing to build and nothing to upload by
> hand** — no `cargo`, Rustup, CMake or `c++`. All runtime logs stream to the
> **Console** tab (`INFO`/`DEBUG` on stdout, `WARN`/`ERROR` on stderr); file
> logging is opt-in. See [`../DEPLOYMENT.md`](../DEPLOYMENT.md) for
> the full walkthrough. The sections below cover the build-from-source variants.

Pterodactyl runs each server in a container it controls: the panel mounts the
server volume at `/home/container`, drops to the `container` user, runs from
`/home/container`, and executes the egg's **startup command**. It does **not**
use a server image's own `ENTRYPOINT`/`USER`/`WORKDIR`. Two things follow:

- The egg's startup command must `cd` into the repo and put `cargo` on `PATH`.
- The container image must already contain a Rust toolchain and the native libs.

This directory ships:

| File | Purpose |
|---|---|
| `egg-audium-binary.json` | Importable egg that **downloads the prebuilt binary at install time** and runs it (**recommended**; no build tools, no manual upload). |
| `egg-audium.json` | Importable egg (builds from source on first install). |
| `Dockerfile` | Pterodactyl-compatible image (Rust + native deps, for the source path). |

## The one hard rule: port

Pterodactyl routes a server's **primary allocation** port to the container. A
process listening on a different port is unreachable. So the port Audium
binds **must equal** the allocation port the panel shows.

Audium supports `AUDIUM_PORT` / `AUDIUM_ADDRESS` env overrides, which is how
you do that without editing files:

```
AUDIUM_ADDRESS=0.0.0.0
AUDIUM_PORT=<the allocation port>
```

Set `AUDIUM_PORT` in the server's **Startup** variables to match the allocation.

## Option A — build from source (no image build required)

Uses stock Pterodactyl images. Slow first boot (Rust compiles the whole tree).

1. **Admin → Nests → Import Egg** → `pterodactyl/egg-audium.json`.
2. Create a server with that egg. Give it an allocation (e.g. `2333`).
3. Set the **Authorization Secret** startup variable (`openssl rand -hex 32`)
   and set **Port** to the allocation port.
4. Let the installer run. It clones the repo, installs Rust into the volume, and
   pre-builds the binary.
5. Start the server. The console shows `Audium Server listening on ...`.

## Option B — prebuilt image (fast restarts)

1. Build and push the image:

   ```bash
   docker build -f pterodactyl/Dockerfile -t your-registry/audium-ptero:latest .
   docker push your-registry/audium-ptero:latest
   ```

2. In the egg, set `docker_images` to your tag (the egg ships with Pterodactyl
   yolks as a fallback; yolks:debian has no Rust, so install it in the installer
   step or use this image).
3. Import the egg, create the server, set `AUDIUM_AUTHORIZATION` and
   `AUDIUM_PORT`, start.

## Egg startup command

The egg runs:

```bash
cd /home/container/repo && \
  export PATH=/home/container/.cargo/bin:/usr/local/cargo/bin:$PATH \
         AUDIUM_CONFIG_PATH=/home/container/config.toml && \
  cargo run --release --locked --bin audium-server
```

`PATH` is ordered so a toolchain installed into the volume (`/home/container/.cargo/bin`)
wins, falling back to a global one from the image (`/usr/local/cargo/bin`). This one
command works for both options above.

Two things this deliberately avoids:

- **Do not set `RUSTUP_HOME` to the volume** unless you installed rustup there. A
  global image toolchain needs its real `RUSTUP_HOME` (e.g. `/usr/local/rustup`), or
  `rustc` cannot find its sysroot.
- **Do not set `CARGO_HOME` to the volume** on the global-toolchain image; `cargo`
  needs a writable registry there (the `Dockerfile` makes `/usr/local/cargo`
  writable for exactly this reason).

`AUDIUM_CONFIG_PATH` points the process at the config on the volume root, so the
working directory can be the repo without the config silently falling back to
`config.example.toml`.

## Startup variables

| Variable | Required | Notes |
|---|---|---|
| `AUDIUM_AUTHORIZATION` | yes | Lavalink password. No default; weak/placeholder values are rejected. |
| `AUDIUM_ADDRESS` | yes | `0.0.0.0` so the panel can route the allocation. |
| `AUDIUM_PORT` | yes | Must equal the allocation port. |
| `AUDIUM_LOG_LEVEL` | no | `trace`..`off`. |
| `AUDIUM_RATE_LIMIT_PER_MINUTE` | no | `0` disables. |
| `AUDIUM_METRICS_ENABLED` | no | Prometheus endpoint. |

## Gotchas

- **Config location.** The egg sets `AUDIUM_CONFIG_PATH=/home/container/config.toml`
  and writes that file, so it is read regardless of the working directory. Env vars
  override the file, so the secret/port always win.
- **Missing config falls back to `config.example.toml`** with a warning. That file
  has `address = "0.0.0.0"` and a placeholder secret; the env override makes the
  secret real, but keep an explicit `config.toml` to avoid surprises.
- **Console logging is the default.** `INFO`/`DEBUG` go to stdout and
  `WARN`/`ERROR` to stderr, so the panel's **Console** tab shows startup, API,
  player and error logs without any file. A `[logging.file]` section is optional
  and only *mirrors* logs to a file; if you use it, `logs/` must be writable
  (the installer creates `/home/container/logs`).
- **First build is heavy** (native `libopus`/`symphonia` + the whole tree). Give
  the container real CPU and RAM; a 1-core/512 MB allocation may OOM the linker.
- **libopus.** The stock binary links `libopus.so.0`. The installer sets
  `LIBOPUS_STATIC=1`/`OPUS_STATIC=1` to link it statically, avoiding a runtime
  dependency. If you use a prebuilt binary instead, install `libopus0`.
- **Stop button.** Audium handles SIGTERM and drains sessions cleanly; the
  egg's `^^C` stop also works. Prefer the panel's normal stop over kill.
- **No Discord token here.** Like Lavalink, the node does not log into Discord.
  Your bot connects to it with the authorization secret and pushes voice
  credentials. `AUDIUM_*` has no bot-token variable by design.
