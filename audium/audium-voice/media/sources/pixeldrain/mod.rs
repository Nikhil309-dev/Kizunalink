// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Pixeldrain source.
//!
//! Resolves `pixeldrain.com/u/<id>` audio files. The public page exposes the
//! title via `og:title` and the type via `og:type`; only `music.song` items are
//! accepted. The stream is the public file API `pixeldrain.com/api/file/<id>`.
//! No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    common::http::read_text_capped,
    config::sources::PixeldrainConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn file_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https?://pixeldrain\.com/u/(?<id>[A-Za-z0-9]+)").expect("valid regex")
    })
}

fn meta_pattern(property: &str) -> Regex {
    Regex::new(&format!(
        r#"<meta\s+property="{p}"\s+content="([^"]*)""#,
        p = regex::escape(property)
    ))
    .expect("valid escaped regex")
}

pub struct PixeldrainSource {
    http: Arc<reqwest::Client>,
}

impl PixeldrainSource {
    pub fn new(_config: PixeldrainConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_page(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Pixeldrain returned HTTP {}", response.status());
            return None;
        }
        read_text_capped(response).await.ok()
    }

    /// Returns `Some((title, is_song))` parsed from the page's OpenGraph tags.
    fn parse_page(html: &str, id: &str) -> Option<(String, bool)> {
        let title = meta_pattern("og:title")
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| id.to_string());
        let kind = meta_pattern("og:type")
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        Some((title, kind == "music.song"))
    }

    fn build_track(id: &str, title: String, original_url: &str) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: "Unknown artist".to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title,
            uri: Some(format!("https://pixeldrain.com/api/file/{id}")),
            artwork_url: Some(format!("https://pixeldrain.com/api/file/{id}/thumbnail")),
            isrc: None,
            source_name: "pixeldrain".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        track
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = file_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let id = &caps["id"];
        let Some(html) = self.fetch_page(identifier).await else {
            return LoadResult::Empty {};
        };
        match Self::parse_page(&html, id) {
            Some((title, true)) => LoadResult::Track(Self::build_track(id, title, identifier)),
            // A well-formed but non-audio page is a "no match", not an error.
            Some((_, false)) => LoadResult::Empty {},
            None => LoadResult::Empty {},
        }
    }
}

#[async_trait]
impl SourcePlugin for PixeldrainSource {
    fn name(&self) -> &str {
        "pixeldrain"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        file_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Pixeldrain resolving: {}", identifier);
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

    fn source() -> PixeldrainSource {
        PixeldrainSource::new(
            PixeldrainConfig::default(),
            Arc::new(reqwest::Client::new()),
        )
        .expect("valid source")
    }

    #[test]
    fn recognises_file_urls() {
        let s = source();
        assert!(s.can_handle("https://pixeldrain.com/u/abcd1234"));
        assert!(s.can_handle("http://pixeldrain.com/u/XyZ"));
        assert!(!s.can_handle("https://pixeldrain.com/l/abcd1234"));
        assert!(!s.can_handle("https://example.com/u/abcd1234"));
    }

    #[test]
    fn accepts_music_song_pages() {
        let html = r#"<html><head>
            <meta property="og:title" content="My Song" />
            <meta property="og:type" content="music.song" />
            </head></html>"#;
        let (title, is_song) = PixeldrainSource::parse_page(html, "abcd1234").expect("parsed");
        assert_eq!(title, "My Song");
        assert!(is_song);
    }

    #[test]
    fn rejects_non_song_pages() {
        let html = r#"<meta property="og:title" content="archive.zip" />
            <meta property="og:type" content="application/octet-stream" />"#;
        let (_, is_song) = PixeldrainSource::parse_page(html, "abcd1234").expect("parsed");
        assert!(!is_song);
    }

    #[test]
    fn builds_playable_urls() {
        let track = PixeldrainSource::build_track(
            "abcd1234",
            "My Song".into(),
            "https://pixeldrain.com/u/abcd1234",
        );
        assert_eq!(track.info.source_name, "pixeldrain");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://pixeldrain.com/api/file/abcd1234")
        );
        assert_eq!(
            track.info.artwork_url.as_deref(),
            Some("https://pixeldrain.com/api/file/abcd1234/thumbnail")
        );
    }
}
