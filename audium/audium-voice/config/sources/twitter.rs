// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Twitter / X video resolution.
///
/// Off by default. Resolves `twitter.com/<user>/status/<id>` and `x.com`
/// equivalents to the best MP4 or HLS variant. Public tweets only; no account
/// or API key required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct TwitterConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
