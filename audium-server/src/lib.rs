// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

// N05: server library code logs via `tracing`, it never prints (the binary may: `main.rs`
// delegates to the banner module which is allow-listed inside the `audium` crate).
#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)
)]

pub mod api;
pub mod health;
pub mod lavalink;
pub mod monitoring;
pub mod server;
pub mod tls;

// Re-export audium submodules so server code can use `crate::` paths.
pub use audium::common;
pub use audium::discord;
pub use audium::engine as audio;
pub use audium::media::sources;

// Convenience re-exports matching how the server code references these.
pub use audium::discord::gateway;
pub use audium::discord::player;
pub use audium::lavalink::protocol;
pub use audium::lavalink::routeplanner;
