// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Clypit Clyp.it audio-clip resolution.
///
/// Off by default. Resolves `clyp.it/<id>` clips via the public `api.clyp.it` JSON endpoint (`Mp3Url`). No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct ClypitConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
