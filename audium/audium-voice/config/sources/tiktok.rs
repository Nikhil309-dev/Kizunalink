// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// TikTok video resolution.
///
/// Off by default. Resolves `tiktok.com/@user/video/<id>` and the `vm.tiktok.com`
/// short links through TikTok's public player API, which needs no credentials.
/// The extracted CDN MP4 URL is stored as the track `uri` and decoded by the
/// `http` source.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct TiktokConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
    /// Search result limit for `ttsearch:` queries.
    pub search_limit: usize,
}
