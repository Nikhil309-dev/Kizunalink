// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Soundgasm Soundgasm audio-page resolution.
///
/// Off by default. Resolves `soundgasm.net/u/<author>/<slug>` pages by scraping the embedded `media.soundgasm.net/sounds/<id>.m4a` stream. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct SoundgasmConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
