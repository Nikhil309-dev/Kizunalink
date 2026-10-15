// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Bilibili video, bangumi and audio resolution.
///
/// Off by default. Resolves `bilibili.com/video/BV...` (also `av...`), `b23.tv`
/// short links, bangumi (`/bangumi/play/ep...`) and audio (`/audio/au...`).
/// Playback is the anonymous DASH audio stream; no account required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct BilibiliConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
    /// Optional `SESSDATA` cookie, sent as a bare value when provided.
    pub sessdata: Option<String>,
}
