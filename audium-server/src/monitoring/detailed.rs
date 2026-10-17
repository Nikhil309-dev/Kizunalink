// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! NodeLink-style `detailedStats` block for `/v4/stats`.
//!
//! Lavalink's `/v4/stats` schema is fixed and Audium stays byte-compatible with
//! it by keeping these counters in a *separate, additive* `detailedStats` key
//! (exactly how NodeLink exposes its extras). The block adds process/thread
//! counts, per-source resolve outcomes, and playback event totals — the
//! observability NodeLink users get for free, but as an opt-in extra object so
//! strict Lavalink clients never see an unexpected field.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

/// Global, monotonically-increasing counters. Cheap enough to bump on every
/// request/resolve/event without measurable overhead.
#[derive(Debug, Default)]
pub struct DetailedCounters {
    pub requests_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub tracks_played: AtomicU64,
    pub track_exceptions: AtomicU64,
    pub track_starts: AtomicU64,
    pub track_ends: AtomicU64,
}

/// Wire shape of the `detailedStats` object (NodeLink-compatible naming).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailedStats {
    pub version: &'static str,
    pub total_requests: u64,
    pub total_errors: u64,
    pub process: ProcessStats,
    pub sources: SourceStats,
    pub playback: PlaybackStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStats {
    pub process_id: u32,
    pub threads: usize,
    pub cpu_cores: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SourceStats {
    /// Number of source managers registered in the registry.
    pub loaded: usize,
    /// Source managers enabled and eligible to resolve at runtime.
    pub enabled: usize,
    /// Configured name of the source that ultimately served the most resolves.
    pub top: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStats {
    pub tracks_started: u64,
    pub tracks_ended: u64,
    pub exceptions: u64,
}

impl DetailedCounters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn inc_requests(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_errors(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_track_start(&self) {
        self.track_starts.fetch_add(1, Ordering::Relaxed);
        self.tracks_played.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_track_end(&self) {
        self.track_ends.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_exception(&self) {
        self.track_exceptions.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the counters plus live process facts into the wire struct.
    pub fn snapshot(
        &self,
        loaded: usize,
        enabled: usize,
        top: Option<String>,
        cpu_cores: usize,
    ) -> DetailedStats {
        DetailedStats {
            version: env!("CARGO_PKG_VERSION"),
            total_requests: self.requests_total.load(Ordering::Relaxed),
            total_errors: self.errors_total.load(Ordering::Relaxed),
            process: ProcessStats {
                process_id: std::process::id(),
                threads: thread_count(),
                cpu_cores,
            },
            sources: SourceStats {
                loaded,
                enabled,
                top,
            },
            playback: PlaybackStats {
                tracks_started: self.track_starts.load(Ordering::Relaxed),
                tracks_ended: self.track_ends.load(Ordering::Relaxed),
                exceptions: self.track_exceptions.load(Ordering::Relaxed),
            },
        }
    }
}

/// Number of OS threads in this process. `/proc/self/status` on Linux, falling
/// back to 0 elsewhere (the field is decorative).
fn thread_count() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Threads:").map(|v| v.trim().to_string()))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_snapshot_reflects_increments() {
        let c = DetailedCounters::new();
        c.inc_requests();
        c.inc_requests();
        c.inc_errors();
        c.inc_track_start();
        c.inc_track_end();
        c.inc_exception();

        let s = c.snapshot(27, 3, Some("youtube".into()), 8);
        assert_eq!(s.total_requests, 2);
        assert_eq!(s.total_errors, 1);
        assert_eq!(s.sources.loaded, 27);
        assert_eq!(s.sources.enabled, 3);
        assert_eq!(s.sources.top.as_deref(), Some("youtube"));
        assert_eq!(s.playback.tracks_started, 1);
        assert_eq!(s.playback.tracks_ended, 1);
        assert_eq!(s.playback.exceptions, 1);
        assert_eq!(s.process.cpu_cores, 8);
    }

    #[test]
    fn snapshot_serializes_with_camel_case_keys() {
        let c = DetailedCounters::new();
        let json = serde_json::to_value(c.snapshot(1, 1, None, 1)).unwrap();
        assert!(json.get("totalRequests").is_some());
        assert!(json.get("totalErrors").is_some());
        assert!(json.get("playback").unwrap().get("tracksStarted").is_some());
        assert!(json["sources"]["loaded"].is_number());
    }
}
