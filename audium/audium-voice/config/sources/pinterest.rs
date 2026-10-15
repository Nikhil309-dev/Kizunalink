// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Pinterest pin (video) resolution.
///
/// Off by default. Resolves `pinterest.<tld>/pin/<id>` videos through the
/// keyless `PinResource` endpoint. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct PinterestConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
