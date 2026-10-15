// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Instagram post, reel and audio resolution.
///
/// Off by default. Resolves `/p/`, `/reel/` and `/reels/audio/` links. Public
/// content only; no login is performed.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct InstagramConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
