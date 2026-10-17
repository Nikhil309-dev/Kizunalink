// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Telegram public-message resolution.
///
/// Off by default. Resolves `t.me/<channel>/<id>` public posts through the
/// keyless `?embed=1` widget page and plays the message's video/audio
/// attachment(s). No account or bot token required.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
}
