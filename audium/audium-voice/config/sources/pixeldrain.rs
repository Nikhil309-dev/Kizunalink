// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Pixeldrain Pixeldrain file resolution.
///
/// Off by default. Resolves `pixeldrain.com/u/<id>` audio files via the public file API (`/api/file/<id>`), accepting only `music.song` pages. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct PixeldrainConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
