// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

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

pub use audium::common;
pub use audium::discord;
pub use audium::engine as audio;
pub use audium::media::sources;

pub use audium::discord::gateway;
pub use audium::discord::player;
pub use audium::lavalink::protocol;
pub use audium::lavalink::routeplanner;
