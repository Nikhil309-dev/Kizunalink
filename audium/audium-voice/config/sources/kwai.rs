// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Kwai (Kuaishou) video resolution.
///
/// Off by default. Resolves `kwai.com/video/<id>` pages from the public
/// server-rendered payload. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct KwaiConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
