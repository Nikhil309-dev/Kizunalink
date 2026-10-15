// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// NicoVideo (nicovideo.jp / nico.ms) resolution.
///
/// Off by default. Resolves watch pages and short links, with optional keyword
/// search. Playback uses the anonymous DMC session API; no account required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct NicovideoConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
