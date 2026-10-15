// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Vimeo video source.
//!
//! Resolves public `vimeo.com/<id>` (optionally with the `?h=<hash>` privacy
//! parameter) and `player.vimeo.com/video/<id>` links. Metadata comes from the
//! keyless oEmbed endpoint; the playable URL is taken from the page's
//! `window.playerConfig` payload, preferring a progressive MP4 and falling back
//! to the HLS master playlist. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::VimeoConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const BASE: &str = "https://vimeo.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn video_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.|player\.)?vimeo\.com/(?:video/)?(\d+)")
            .expect("valid regex")
    })
}

fn hash_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[?&]h=([0-9a-f]+)").expect("valid regex"))
}

pub struct VimeoSource {
    http: Arc<reqwest::Client>,
}

struct Metadata {
    title: String,
    author: String,
    duration_ms: u64,
    artwork: Option<String>,
}

impl VimeoSource {
    pub fn new(_config: VimeoConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    fn page_url(video_id: &str, hash: Option<&str>) -> String {
        match hash {
            Some(h) => format!("{BASE}/{video_id}?h={h}"),
            None => format!("{BASE}/{video_id}"),
        }
    }

    async fn fetch_metadata(&self, video_id: &str, hash: Option<&str>) -> Option<Metadata> {
        let target = Self::page_url(video_id, hash);
        let oembed = format!(
            "{BASE}/api/oembed.json?url={}",
            urlencoding::encode(&target)
        );
        let response = self
            .http
            .get(&oembed)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Vimeo oEmbed returned HTTP {}", response.status());
            return None;
        }
        let body: Value = crate::common::http::read_json_capped::<serde_json::Value>(response)
            .await
            .ok()?;
        let title = body.get("title")?.as_str()?.to_string();
        Some(Metadata {
            title,
            author: body
                .get("author_name")
                .and_then(|a| a.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            duration_ms: body
                .get("duration")
                .and_then(|d| d.as_u64())
                .unwrap_or(0)
                .saturating_mul(1000),
            artwork: body
                .get("thumbnail_url")
                .and_then(|t| t.as_str())
                .map(str::to_string),
        })
    }

    /// Extracts the first progressive MP4 (or the HLS master) from the page.
    async fn fetch_stream_url(&self, video_id: &str, hash: Option<&str>) -> Option<String> {
        let url = Self::page_url(video_id, hash);
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Vimeo page returned HTTP {}", response.status());
            return None;
        }
        let html = crate::common::http::read_text_capped(response).await.ok()?;

        let progressive = Regex::new(r#""progressive":\s*\[\s*\{[^}]*"url":"([^"]+)""#).ok()?;
        if let Some(caps) = progressive.captures(&html) {
            return Some(decode_slashes(&caps[1]));
        }
        let hls = Regex::new(
            r#""hls":\s*\{\s*"default_cdn":"[^"]*",\s*"cdns":\s*\{[^}]*"url":"([^"]+)""#,
        )
        .ok()?;
        hls.captures(&html).map(|caps| decode_slashes(&caps[1]))
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = video_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let video_id = caps[1].to_string();
        let hash = hash_pattern()
            .captures(identifier)
            .map(|c| c[1].to_string());
        let hash = hash.as_deref();

        let Some(meta) = self.fetch_metadata(&video_id, hash).await else {
            return LoadResult::Empty {};
        };
        let Some(stream) = self.fetch_stream_url(&video_id, hash).await else {
            return LoadResult::Empty {};
        };

        let mut track = Track::new(TrackInfo {
            identifier: video_id.clone(),
            is_seekable: true,
            author: meta.author,
            length: meta.duration_ms,
            is_stream: false,
            position: 0,
            title: meta.title,
            uri: Some(stream),
            artwork_url: meta.artwork,
            isrc: None,
            source_name: "vimeo".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": identifier });
        LoadResult::Track(track)
    }
}

/// Vimeo escapes `/` inside JSON string URLs.
fn decode_slashes(s: &str) -> String {
    s.replace("\\/", "/")
}

#[async_trait]
impl SourcePlugin for VimeoSource {
    fn name(&self) -> &str {
        "vimeo"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        video_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Vimeo resolving: {}", identifier);
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

    #[test]
    fn recognises_video_urls() {
        assert!(video_pattern().is_match("https://vimeo.com/123456789"));
        assert!(video_pattern().is_match("https://www.vimeo.com/123456789?h=abcdef"));
        assert!(video_pattern().is_match("https://player.vimeo.com/video/123456789"));
        assert!(!video_pattern().is_match("https://vimeo.com/channels/staffpicks"));
    }

    #[test]
    fn extracts_privacy_hash() {
        let caps = hash_pattern()
            .captures("https://vimeo.com/123?h=1a2b3c")
            .unwrap();
        assert_eq!(&caps[1], "1a2b3c");
    }

    #[test]
    fn builds_page_url_with_hash() {
        assert_eq!(
            VimeoSource::page_url("123", Some("abc")),
            "https://vimeo.com/123?h=abc"
        );
        assert_eq!(VimeoSource::page_url("123", None), "https://vimeo.com/123");
    }

    #[test]
    fn decodes_escaped_slashes() {
        assert_eq!(
            decode_slashes("https:\\/\\/cdn.vimeo.com\\/v.mp4"),
            "https://cdn.vimeo.com/v.mp4"
        );
    }

    #[test]
    fn finds_progressive_url_in_page() {
        let html = r#"window.playerConfig = {"request":{"files":{"progressive":[{"url":"https:\/\/cdn.vimeo.com\/a.mp4","quality":"720p"}]}}};"#;
        let re = Regex::new(r#""progressive":\s*\[\s*\{[^}]*"url":"([^"]+)""#).unwrap();
        let caps = re.captures(html).unwrap();
        assert_eq!(decode_slashes(&caps[1]), "https://cdn.vimeo.com/a.mp4");
    }
}
