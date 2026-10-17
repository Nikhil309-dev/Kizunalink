// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// OCRemix OC ReMix community remix resolution.
///
/// Off by default. Resolves `ocremix.org/remix/<id>` (and bare `OCR<digits>`) by reading the public `?view=xml` metadata and streaming `file_name` from a public OC ReMix mirror host. No credentials required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct OcremixConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
