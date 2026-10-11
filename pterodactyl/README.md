# Running KizunaLink on Pterodactyl

Pterodactyl runs each server in a container it controls: the panel mounts the
server volume at `/home/container`, drops to the `container` user, runs from
`/home/container`, and executes the egg's **startup command**. It does **not**
use a server image's own `ENTRYPOINT`/`USER`/`WORKDIR`. Two things follow:

- The egg's startup command must `cd` into the repo and put `cargo` on `PATH`.
- The container image must already contain a Rust toolchain and the native libs.

This directory ships:

| File | Purpose |
|---|---|
| `egg-kizunalink.json` | Importable egg (builds from source on first install). |
| `Dockerfile` | Pterodactyl-compatible image (Rust + native deps). |

## The one hard rule: port

Pterodactyl routes a server's **primary allocation** port to the container. A
process listening on a different port is unreachable. So the port KizunaLink
binds **must equal** the allocation port the panel shows.

KizunaLink supports `KIZUNA_PORT` / `KIZUNA_ADDRESS` env overrides, which is how
you do that without editing files:

```
KIZUNA_ADDRESS=0.0.0.0
KIZUNA_PORT=<the allocation port>
```

Set `KIZUNA_PORT` in the server's **Startup** variables to match the allocation.

## Option A — build from source (no image build required)

Uses stock Pterodactyl images. Slow first boot (Rust compiles the whole tree).

1. **Admin → Nests → Import Egg** → `pterodactyl/egg-kizunalink.json`.
2. Create a server with that egg. Give it an allocation (e.g. `2333`).
3. Set the **Authorization Secret** startup variable (`openssl rand -hex 32`)
   and set **Port** to the allocation port.
4. Let the installer run. It clones the repo, installs Rust into the volume, and
   pre-builds the binary.
5. Start the server. The console shows `KizunaLink Server listening on ...`.

## Option B — prebuilt image (fast restarts)

1. Build and push the image:

   ```bash
   docker build -f pterodactyl/Dockerfile -t your-registry/kizunalink-ptero:latest .
   docker push your-registry/kizunalink-ptero:latest
   ```

2. In the egg, set `docker_images` to your tag (the egg ships with Pterodactyl
   yolks as a fallback; yolks:debian has no Rust, so install it in the installer
   step or use this image).
3. Import the egg, create the server, set `KIZUNA_AUTHORIZATION` and
   `KIZUNA_PORT`, start.

## Egg startup command

The egg runs:

```bash
cd /home/container/repo && \
  export CARGO_HOME=/home/container/.cargo RUSTUP_HOME=/home/container/.rustup \
         PATH=/home/container/.cargo/bin:$PATH && \
  cargo run --release --locked --bin kizuna-server
```

`CARGO_HOME`/`RUSTUP_HOME` point at the server volume so the toolchain and build
cache survive restarts. On the image from `Dockerfile` you can drop those exports
(the toolchain is global) and run the binary directly:

```bash
cd /home/container/repo && ./target/release/kizuna-server
```

## Startup variables

| Variable | Required | Notes |
|---|---|---|
| `KIZUNA_AUTHORIZATION` | yes | Lavalink password. No default; weak/placeholder values are rejected. |
| `KIZUNA_ADDRESS` | yes | `0.0.0.0` so the panel can route the allocation. |
| `KIZUNA_PORT` | yes | Must equal the allocation port. |
| `KIZUNA_LOG_LEVEL` | no | `trace`..`off`. |
| `KIZUNA_RATE_LIMIT_PER_MINUTE` | no | `0` disables. |
| `KIZUNA_METRICS_ENABLED` | no | Prometheus endpoint. |

## Gotchas

- **Config location.** The process reads `config.toml` from its working
  directory, i.e. `/home/container/repo/config.toml` in this layout. The installer
  writes the minimal file to `/mnt/server/config.toml`; if you prefer it at the
  repo root, either move it or set `KIZUNA_CONFIG_PATH`. Env vars override the
  file either way, so the secret/port always win.
- **Missing config falls back to `config.example.toml`** with a warning. That
  file has `address = "0.0.0.0"` and a placeholder secret; the env override makes
  the secret real, but keep an explicit `config.toml` to avoid surprises.
- **`logs/` must be writable.** The default log path is `./logs/kizunalink.log`.
  The installer creates `/mnt/server/logs`; make sure the repo-relative `logs/`
  exists too if you keep the default path with the repo as CWD.
- **First build is heavy** (native `libopus`/`symphonia` + the whole tree). Give
  the container real CPU and RAM; a 1-core/512 MB allocation may OOM the linker.
- **libopus.** The stock binary links `libopus.so.0`. The installer sets
  `LIBOPUS_STATIC=1`/`OPUS_STATIC=1` to link it statically, avoiding a runtime
  dependency. If you use a prebuilt binary instead, install `libopus0`.
- **Stop button.** KizunaLink handles SIGTERM and drains sessions cleanly; the
  egg's `^^C` stop also works. Prefer the panel's normal stop over kill.
- **No Discord token here.** Like Lavalink, the node does not log into Discord.
  Your bot connects to it with the authorization secret and pushes voice
  credentials. `KIZUNA_*` has no bot-token variable by design.
