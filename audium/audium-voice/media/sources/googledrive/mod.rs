// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Google Drive source.
//!
//! Resolves publicly shared `drive.google.com/file/d/<id>` links by probing the
//! `uc?export=download` endpoint, which redirects to the real media host for
//! files small enough to skip the virus-scan interstitial. Shared folders
//! (`drive.google.com/drive/folders/<id>`) are expanded into a playlist by
//! reading the public embedded-folder listing. Only publicly shared content is
//! supported; no OAuth credentials are used.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    config::sources::GoogleDriveConfig,
    lavalink::protocol::tracks::{LoadResult, PlaylistData, PlaylistInfo, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const DEFAULT_MAX_FOLDER_ENTRIES: usize = 200;

fn file_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^https?://(?:docs|drive|drive\.usercontent)\.google\.com/(?:(?:uc|open|download)\?.*?id=|file/d/)([a-zA-Z0-9_-]{28,})",
        )
        .expect("valid regex")
    })
}

fn folder_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https?://(?:docs|drive)\.google\.com/drive/folders/([a-zA-Z0-9_-]{28,})")
            .expect("valid regex")
    })
}

pub struct GoogleDriveSource {
    http: Arc<reqwest::Client>,
    max_folder_entries: usize,
}

impl GoogleDriveSource {
    pub fn new(config: GoogleDriveConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self {
            http,
            max_folder_entries: match config.max_folder_entries {
                0 => DEFAULT_MAX_FOLDER_ENTRIES,
                n => n,
            },
        })
    }

    /// Probes the download endpoint and returns the resolved direct URL.
    ///
    /// `uc?export=download` answers with a 302 to the media host for files that
    /// need no confirmation token. Larger files return the HTML confirmation
    /// page, which is not usable without a session cookie, so those yield
    /// `None`.
    async fn resolve_file_url(&self, file_id: &str) -> Option<String> {
        let probe_url = format!("https://drive.google.com/uc?id={file_id}&export=download");
        let response = self
            .http
            .get(&probe_url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::REFERER, "https://drive.google.com/")
            .send()
            .await
            .ok()?;
        let status = response.status();
        let final_url = response.url().to_string();
        if status.is_success() && !final_url.contains("drive.google.com") {
            return Some(final_url);
        }

        if !status.is_success() {
            debug!("Google Drive probe returned HTTP {status}");
        }
        None
    }

    fn build_file_track(file_id: &str, uri: String, original_url: &str) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: file_id.to_string(),
            is_seekable: true,
            author: "Google Drive".to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title: format!("Google Drive File {file_id}"),
            uri: Some(uri),
            artwork_url: None,
            isrc: None,
            source_name: "googledrive".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        track
    }

    /// Reads the public embedded-folder listing into `(id, title)` pairs.
    fn parse_folder(html: &str, limit: usize) -> Vec<(String, String)> {
        let entry_re = Regex::new(
            r#"<div class="flip-entry" id="entry-([A-Za-z0-9_-]{20,})"[\s\S]*?<div class="flip-entry-title">([\s\S]*?)</div>"#,
        )
        .expect("valid regex");
        entry_re
            .captures_iter(html)
            .take(limit)
            .map(|caps| (caps[1].to_string(), decode_entities(caps[2].trim())))
            .collect()
    }

    async fn resolve_folder(&self, folder_id: &str) -> LoadResult {
        let url = format!("https://drive.google.com/embeddedfolderview?id={folder_id}");
        let response = match self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                debug!("Google Drive folder returned HTTP {}", r.status());
                return LoadResult::Empty {};
            }
            Err(_) => return LoadResult::Empty {},
        };
        let html = match crate::common::http::read_text_capped(response).await {
            Ok(h) => h,
            Err(_) => return LoadResult::Empty {},
        };
        let entries = Self::parse_folder(&html, self.max_folder_entries);
        if entries.is_empty() {
            return LoadResult::Empty {};
        }

        let name = Regex::new(r"(?is)<title>(.*?)</title>")
            .ok()
            .and_then(|re| re.captures(&html))
            .map(|c| decode_entities(c[1].trim()))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Google Drive Folder {folder_id}"));

        let tracks: Vec<Track> = entries
            .into_iter()
            .map(|(id, title)| {
                let mut track = Track::new(TrackInfo {
                    identifier: id.clone(),
                    is_seekable: true,
                    author: "Google Drive".to_string(),
                    length: 0,
                    is_stream: false,
                    position: 0,
                    title,
                    uri: Some(format!("https://drive.google.com/file/d/{id}/view")),
                    artwork_url: None,
                    isrc: None,
                    source_name: "googledrive".to_string(),
                });
                track.plugin_info = serde_json::json!({ "folderId": folder_id });
                track
            })
            .collect();

        LoadResult::Playlist(PlaylistData {
            info: PlaylistInfo {
                name,
                selected_track: 0,
            },
            plugin_info: serde_json::json!({ "folderId": folder_id }),
            tracks,
        })
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        if let Some(caps) = folder_pattern().captures(identifier) {
            return self.resolve_folder(&caps[1]).await;
        }
        let Some(caps) = file_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let file_id = caps[1].to_string();
        match self.resolve_file_url(&file_id).await {
            Some(uri) => LoadResult::Track(Self::build_file_track(&file_id, uri, identifier)),
            None => LoadResult::Empty {},
        }
    }
}

/// Decodes the handful of HTML entities that appear in Drive folder titles.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[async_trait]
impl SourcePlugin for GoogleDriveSource {
    fn name(&self) -> &str {
        "googledrive"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        file_pattern().is_match(identifier) || folder_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Google Drive resolving: {}", identifier);
        self.resolve(identifier).await
    }

    async fn get_track(
        &self,
        _identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<BoxedTrack> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> GoogleDriveSource {
        GoogleDriveSource::new(
            GoogleDriveConfig::default(),
            Arc::new(reqwest::Client::new()),
        )
        .expect("valid source")
    }

    const ID: &str = "1A2b3C4d5E6f7G8h9I0jK1l2M3n4O5p6Q";

    #[test]
    fn recognises_file_and_folder_urls() {
        let s = source();
        assert!(s.can_handle(&format!("https://drive.google.com/file/d/{ID}/view")));
        assert!(s.can_handle(&format!(
            "https://drive.google.com/uc?id={ID}&export=download"
        )));
        assert!(s.can_handle(&format!("https://docs.google.com/drive/folders/{ID}")));
        assert!(!s.can_handle("https://drive.google.com/drive/my-drive"));
        assert!(!s.can_handle("https://example.com/file/d/short"));
    }

    #[test]
    fn parses_folder_entries() {
        let html = format!(
            r#"<div class="flip-entry" id="entry-{ID}"><a href="https://drive.google.com/file/d/{ID}/view"></a>
               <div class="flip-entry-title">Song &amp; Dance.mp3</div></div>
               <div class="flip-entry" id="entry-2B3c4D5e6F7g8H9i0J1k2L3m4N5o6P7q"><a></a>
               <div class="flip-entry-title">Other.flac</div></div>"#
        );
        let entries = GoogleDriveSource::parse_folder(&html, 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, "Song & Dance.mp3");
        assert_eq!(entries[1].1, "Other.flac");
    }

    #[test]
    fn caps_folder_entries() {
        let mut html = String::new();
        for i in 0..5 {
            html.push_str(&format!(
                r#"<div class="flip-entry" id="entry-{ID}{i}"><div class="flip-entry-title">T{i}</div></div>"#
            ));
        }
        assert_eq!(GoogleDriveSource::parse_folder(&html, 3).len(), 3);
    }

    #[test]
    fn builds_file_track() {
        let track = GoogleDriveSource::build_file_track(
            ID,
            "https://r5---sn-x.googlevideo.com/videoplayback".to_string(),
            "https://drive.google.com/file/d/x/view",
        );
        assert_eq!(track.info.identifier, ID);
        assert_eq!(track.info.source_name, "googledrive");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://r5---sn-x.googlevideo.com/videoplayback")
        );
    }

    #[test]
    fn empty_folder_yields_no_entries() {
        assert!(GoogleDriveSource::parse_folder("<html></html>", 10).is_empty());
    }
}
