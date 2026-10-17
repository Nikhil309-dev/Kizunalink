// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Songlink / Odesli cross-platform resolver.
///
/// Off by default. Given a song URL on any supported platform, asks the keyless
/// Odesli API for equivalent links elsewhere and re-dispatches the first one a
/// configured, enabled source can play. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct SonglinkConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
