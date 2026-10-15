// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// iHeartRadio live-station (radio) resolution.
///
/// Off by default. Only resolves *live* stations (non-seekable streams) via
/// iHeart's public catalog API — no account or key required. On-demand tracks
/// are intentionally not exposed because their protected stream formats are not
/// portable.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct IheartradioConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
    /// Search result limit for `ihsearch:` queries.
    pub search_limit: usize,
}
