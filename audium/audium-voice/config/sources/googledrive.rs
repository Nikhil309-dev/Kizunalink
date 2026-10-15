// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::config::sources::HttpProxyConfig;

/// Google Drive file and folder resolution.
///
/// Off by default. Resolves public `drive.google.com/file/d/<id>` links to a
/// direct download URL and public folders to a playlist. No OAuth credentials
/// required (only publicly shared content works).
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct GoogleDriveConfig {
    pub enabled: bool,
    pub proxy: Option<HttpProxyConfig>,
    /// Maximum number of entries expanded from a shared folder.
    pub max_folder_entries: usize,
}
