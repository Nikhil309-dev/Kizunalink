// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Getyarn getyarn.io clip resolution.
///
/// Off by default. Resolves `getyarn.io/yarn-clip/<id>` clips to the deterministic `y.yarn.co/<id>.mp4` stream URL. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct GetyarnConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
