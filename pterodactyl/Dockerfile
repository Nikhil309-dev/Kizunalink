# syntax=docker/dockerfile:1
#
# KizunaLink — Pterodactyl-compatible image.
#
# Pterodactyl does not run a server container's own ENTRYPOINT/USER/WORKDIR: the
# panel mounts the server volume at /home/container, drops to the `container`
# user, runs from /home/container, and invokes the egg's startup command. So this
# image only has to provide the runtime the egg needs (a Rust toolchain + the
# native build/runtime libraries), laid out the way the panel expects.
#
# Build (from the repo root):
#   docker build -f pterodactyl/Dockerfile -t your-registry/kizunalink-ptero:latest .
# Push:
#   docker push your-registry/kizunalink-ptero:latest
# Then reference the pushed tag as a docker image in the egg / server.
#
# NOTE: this image installs the toolchain under /usr/local (global), which the
# `container` user can read. The egg's startup still points CARGO_HOME at the
# server volume so build caches persist across restarts.

FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    LIBOPUS_STATIC=1 \
    OPUS_STATIC=1

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl git bash tar xz-utils \
        build-essential cmake pkg-config libclang-dev clang perl \
        libopus0 \
        tini \
        && rm -rf /var/lib/apt/lists/*

# Global Rust toolchain so the `container` user can build without owning rustup.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --no-modify-path \
    && chmod -R a+rx /usr/local/rustup /usr/local/cargo

# The panel creates and owns this user; match its uid/gid.
RUN useradd -m -d /home/container container

USER container
ENV USER=container HOME=/home/container
WORKDIR /home/container

# Pterodactyl overrides this with the egg's startup command; tini reaps zombies.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/bin/bash"]
