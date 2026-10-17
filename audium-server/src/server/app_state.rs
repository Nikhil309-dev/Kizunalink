// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use dashmap::DashMap;
use sysinfo::System;

use crate::{
    common::types::SessionId,
    lavalink::routeplanner::RoutePlanner,
    monitoring::sysstat::ProcessStat,
    server::session::Session,
    sources::{SourceManager, youtube::YoutubeStreamContext},
};

pub type SessionMap = DashMap<SessionId, Arc<Session>>;

pub struct AppState {
    pub start_time: std::time::Instant,
    pub sessions: SessionMap,
    pub resumable_sessions: SessionMap,
    pub routeplanner: Option<Arc<dyn RoutePlanner>>,
    pub source_manager: Arc<SourceManager>,
    pub lyrics_manager: Arc<audium::media::lyrics::LyricsManager>,
    pub config: audium::config::AppConfig,
    pub youtube: Option<Arc<YoutubeStreamContext>>,
    pub system_state: parking_lot::Mutex<System>,
    pub last_system_refresh: parking_lot::Mutex<std::time::Instant>,
    pub process_stat: parking_lot::Mutex<ProcessStat>,
    /// Per-IP token bucket guarding the public HTTP/WS surface (S06).
    pub rate_limiter: Arc<crate::api::RateLimiter>,
    /// Bounds simultaneously-open HTTP/WS connections. `None` = unlimited.
    pub connection_limit: Option<Arc<tokio::sync::Semaphore>>,
    /// NodeLink-parity `/v4/stats` extras (see `monitoring::detailed`).
    pub detailed_counters: Arc<crate::monitoring::detailed::DetailedCounters>,
}

impl audium::common::server_hooks::ServerContext for AppState {}
