// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Tumblr post resolution.
///
/// Off by default. Extracts a post's embedded video/audio URL (or a delegated
/// YouTube/Vimeo link) from the public post page. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct TumblrConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
