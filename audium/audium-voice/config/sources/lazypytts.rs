// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// LazyPy text-to-speech.
///
/// Off by default. Turns `lazypytts:service:voice:text` (or `lazytts:`) queries
/// into speech via the keyless LazyPy TTS endpoint. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct LazypyttsConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
    /// Service used when the query does not name one.
    pub default_service: Option<String>,
    /// Voice used when the query does not name one.
    pub default_voice: Option<String>,
    /// Maximum characters accepted per request.
    pub max_text_length: usize,
}
