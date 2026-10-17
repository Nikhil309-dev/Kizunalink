// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// RSS / Atom podcast feed resolution.
///
/// Off by default. A feed URL is expanded into a playlist of its playable
/// enclosure items (audio/video media), which the [`crate::media::sources::http`]
/// source then decodes like any other remote media.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct RssConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
    /// Maximum number of feed items expanded into a playlist. Bounds the
    /// response for very large feeds. Defaults to 100 when unset.
    pub max_items: usize,
}
