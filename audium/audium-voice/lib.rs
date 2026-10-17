// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)
)]

pub mod common;
pub mod config;

pub mod discord;
pub mod engine;
pub mod lavalink;
pub mod media;
pub mod opus;
