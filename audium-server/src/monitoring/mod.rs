// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

pub mod detailed;
pub mod prometheus;
pub mod stats_collector;
pub mod sysstat;

pub use stats_collector::collect_stats;
