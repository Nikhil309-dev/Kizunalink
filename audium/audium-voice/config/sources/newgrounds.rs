// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Newgrounds audio-portal resolution.
///
/// Off by default. Resolves `newgrounds.com/audio/listen/<id>` pages by reading
/// the public audio-page metadata and the `audio-download.ngfiles.com` URL. No
/// credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct NewgroundsConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
