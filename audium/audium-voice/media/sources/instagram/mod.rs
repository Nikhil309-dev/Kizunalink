// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Instagram source.
//!
//! Resolves public posts (`/p/<code>`), reels (`/reel/<code>`) and reel audio
//! pages (`/reels/audio/<id>`). Media URLs are read from the post page's
//! embedded JSON (`video_versions`, `image_versions2`, `audio` blocks). No
//! login is performed, so only publicly visible content resolves.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::InstagramConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn post_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?instagram\.com/(?:p|reels?|reel)/([\w-]+)")
            .expect("valid regex")
    })
}

fn audio_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?instagram\.com/reels/audio/(\d+)")
            .expect("valid regex")
    })
}

pub struct InstagramSource {
    http: Arc<reqwest::Client>,
}

impl InstagramSource {
    pub fn new(_config: InstagramConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    /// Fetches a public page as HTML.
    async fn fetch_page(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "text/html,application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Instagram returned HTTP {}", response.status());
            return None;
        }
        crate::common::http::read_text_capped(response).await.ok()
    }

    /// Extracts a top-level JSON object assigned to `key` in a page body.
    ///
    /// Instagram embeds several `window.__X = {...};` payloads whose contents
    /// are not guaranteed to be valid JSON, so this scans for a balanced brace
    /// range rather than parsing the whole document.
    fn extract_json_after(html: &str, key: &str) -> Option<Value> {
        let start = html.find(key)? + key.len();
        let rest = &html[start..];
        let open = rest.find('{')?;
        let body = &rest[open..];
        let end = Self::balanced_object_end(body)?;
        serde_json::from_str(&body[..end]).ok()
    }

    /// Finds the matching closing brace for the object at the start of `s`.
    fn balanced_object_end(s: &str) -> Option<usize> {
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (i, ch) in s.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Reads the first `video_versions[].url` from an Instagram media node.
    fn video_url(node: &Value) -> Option<String> {
        node.get("video_versions")?
            .as_array()?
            .iter()
            .find_map(|v| v.get("url").and_then(|u| u.as_str()))
            .map(str::to_string)
    }

    fn thumbnail(node: &Value) -> Option<String> {
        node.get("image_versions2")?
            .get("candidates")?
            .as_array()?
            .first()?
            .get("url")?
            .as_str()
            .map(str::to_string)
    }

    async fn resolve_post(&self, code: &str, identifier: &str) -> LoadResult {
        let url = format!("https://www.instagram.com/p/{code}/");
        let Some(html) = self.fetch_page(&url).await else {
            return LoadResult::Empty {};
        };
        let Some(payload) = Self::extract_json_after(&html, "video_versions")
            .or_else(|| Self::extract_json_after(&html, "\"media\""))
        else {
            return LoadResult::Empty {};
        };

        let media = payload
            .get("media")
            .cloned()
            .unwrap_or_else(|| payload.clone());
        let Some(stream) = Self::video_url(&media) else {
            return LoadResult::Empty {};
        };

        let title = Self::extract_json_after(&html, "\"caption\"")
            .and_then(|c| c.get("text").and_then(|t| t.as_str()).map(str::to_string))
            .unwrap_or_else(|| "Instagram Video".to_string());
        let author = Self::extract_json_after(&html, "\"owner\"")
            .and_then(|o| {
                o.get("username")
                    .and_then(|u| u.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "User Unknown".to_string());
        let length = Self::extract_json_after(&html, "\"video_duration\"")
            .and_then(|d| d.as_f64())
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| (d * 1000.0).round() as u64)
            .unwrap_or(0);

        let mut track = Track::new(TrackInfo {
            identifier: code.to_string(),
            is_seekable: true,
            author,
            length,
            is_stream: false,
            position: 0,
            title,
            uri: Some(stream),
            artwork_url: Self::thumbnail(&media),
            isrc: None,
            source_name: "instagram".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": identifier });
        LoadResult::Track(track)
    }

    /// Resolves a reel-audio page, which exposes a progressive MP3.
    async fn resolve_audio(&self, audio_id: &str) -> LoadResult {
        let url = format!("https://www.instagram.com/reels/audio/{audio_id}/");
        let Some(html) = self.fetch_page(&url).await else {
            return LoadResult::Empty {};
        };
        let Some(payload) = Self::extract_json_after(&html, "progressive_download_url")
            .or_else(|| Self::extract_json_after(&html, "original_sound_info"))
        else {
            return LoadResult::Empty {};
        };
        let Some(stream) = payload
            .get("progressive_download_url")
            .and_then(|u| u.as_str())
            .map(str::to_string)
        else {
            return LoadResult::Empty {};
        };

        let mut track = Track::new(TrackInfo {
            identifier: audio_id.to_string(),
            is_seekable: true,
            author: "Instagram".to_string(),
            length: payload
                .get("duration_in_ms")
                .and_then(|d| d.as_u64())
                .unwrap_or(0),
            is_stream: false,
            position: 0,
            title: payload
                .get("original_audio_title")
                .and_then(|t| t.as_str())
                .unwrap_or("Instagram Audio")
                .to_string(),
            uri: Some(stream),
            artwork_url: payload
                .get("cover_artwork_thumbnail_uri")
                .and_then(|t| t.as_str())
                .map(str::to_string),
            isrc: None,
            source_name: "instagram".to_string(),
        });
        track.plugin_info = serde_json::json!({ "audioId": audio_id });
        LoadResult::Track(track)
    }
}

#[async_trait]
impl SourcePlugin for InstagramSource {
    fn name(&self) -> &str {
        "instagram"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        audio_pattern().is_match(identifier) || post_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Instagram resolving: {}", identifier);
        if let Some(caps) = audio_pattern().captures(identifier) {
            return self.resolve_audio(&caps[1]).await;
        }
        match post_pattern().captures(identifier) {
            Some(caps) => self.resolve_post(&caps[1], identifier).await,
            None => LoadResult::Empty {},
        }
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

    #[test]
    fn recognises_post_and_audio_urls() {
        assert!(post_pattern().is_match("https://www.instagram.com/p/Cxyz123/"));
        assert!(post_pattern().is_match("https://instagram.com/reel/Cxyz123/"));
        assert!(post_pattern().is_match("https://www.instagram.com/reels/Cxyz123/"));
        assert!(audio_pattern().is_match("https://www.instagram.com/reels/audio/123456/"));
        assert!(!post_pattern().is_match("https://www.instagram.com/explore/tags/foo/"));
    }

    #[test]
    fn extracts_balanced_json_object() {
        let html = r#"<script>window.__data = {"a":{"b":"}"},"c":1};</script>"#;
        let value = InstagramSource::extract_json_after(html, "__data = ").unwrap();
        assert_eq!(value["c"], 1);
        assert_eq!(value["a"]["b"], "}");
    }

    #[test]
    fn returns_none_when_key_absent() {
        assert!(InstagramSource::extract_json_after("<html></html>", "__data = ").is_none());
    }

    #[test]
    fn reads_first_video_version() {
        let node = serde_json::json!({
            "video_versions": [
                { "url": "https://cdn/first.mp4" },
                { "url": "https://cdn/second.mp4" }
            ]
        });
        assert_eq!(
            InstagramSource::video_url(&node).as_deref(),
            Some("https://cdn/first.mp4")
        );
    }

    #[test]
    fn reads_thumbnail_candidate() {
        let node = serde_json::json!({
            "image_versions2": { "candidates": [ { "url": "https://cdn/t.jpg" } ] }
        });
        assert_eq!(
            InstagramSource::thumbnail(&node).as_deref(),
            Some("https://cdn/t.jpg")
        );
    }
}
