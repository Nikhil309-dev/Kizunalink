// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Bluesky (AT Protocol) post video resolution.
///
/// Off by default. Reads a public post's video embeds through the unauthenticated
/// public AppView API — no credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct BlueskyConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
