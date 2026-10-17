// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Vimeo video resolution.
///
/// Off by default. Resolves public `vimeo.com/<id>` (and `player.vimeo.com`)
/// videos to a progressive MP4 or the HLS master playlist. No credentials
/// required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct VimeoConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
