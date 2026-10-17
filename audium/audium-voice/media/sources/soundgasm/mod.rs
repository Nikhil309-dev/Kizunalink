// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Soundgasm source.
//!
//! Resolves `soundgasm.net/u/<author>/<slug>` pages by scraping the embedded
//! `media.soundgasm.net/sounds/<id>.m4a` stream URL and the page title. No
//! credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    common::http::read_text_capped,
    config::sources::SoundgasmConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn page_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https?://soundgasm\.net/u/(?<author>[^/]+)/(?<slug>[^/?#]+)")
            .expect("valid regex")
    })
}

fn sound_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"m4a:\s*["']https://media\.soundgasm\.net/sounds/([^.]+)\.m4a["']"#)
            .expect("valid regex")
    })
}

fn title_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"<div class="jp-title"[^>]*>([^<]+)</div>"#).expect("valid regex")
    })
}

pub struct SoundgasmSource {
    http: Arc<reqwest::Client>,
}

impl SoundgasmSource {
    pub fn new(_config: SoundgasmConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
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
            debug!("Soundgasm returned HTTP {}", response.status());
            return None;
        }
        read_text_capped(response).await.ok()
    }

    /// Extracts the sound id and title from a rendered page.
    fn parse_page(html: &str) -> Option<(String, String)> {
        let id = sound_pattern().captures(html)?.get(1)?.as_str().to_string();
        let title = title_pattern()
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Unknown title".to_string());
        Some((id, title))
    }

    fn build_track(id: &str, title: String, author: &str, original_url: &str) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: author.to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title,
            uri: Some(format!("https://media.soundgasm.net/sounds/{id}.m4a")),
            artwork_url: None,
            isrc: None,
            source_name: "soundgasm".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        track
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = page_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let author = &caps["author"];
        let Some(html) = self.fetch_page(identifier).await else {
            return LoadResult::Empty {};
        };
        match Self::parse_page(&html) {
            Some((id, title)) => {
                LoadResult::Track(Self::build_track(&id, title, author, identifier))
            }
            None => LoadResult::Empty {},
        }
    }
}

#[async_trait]
impl SourcePlugin for SoundgasmSource {
    fn name(&self) -> &str {
        "soundgasm"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        page_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Soundgasm resolving: {}", identifier);
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

    fn source() -> SoundgasmSource {
        SoundgasmSource::new(SoundgasmConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_user_path_urls() {
        let s = source();
        assert!(s.can_handle("https://soundgasm.net/u/someone/my-clip"));
        assert!(s.can_handle("http://soundgasm.net/u/a/b?x=1"));
        assert!(!s.can_handle("https://soundgasm.net/"));
        assert!(!s.can_handle("https://example.net/u/a/b"));
    }

    #[test]
    fn extracts_sound_id_and_title() {
        let html = r#"
            <div class="jp-title" aria-label="title">Relaxing Rain</div>
            <script>var x = { m4a: "https://media.soundgasm.net/sounds/8b9c19db.m4a" };</script>"#;
        let (id, title) = SoundgasmSource::parse_page(html).expect("parsed");
        assert_eq!(id, "8b9c19db");
        assert_eq!(title, "Relaxing Rain");
    }

    #[test]
    fn falls_back_to_generic_title() {
        let html = r#"m4a: 'https://media.soundgasm.net/sounds/abc.m4a'"#;
        let (id, title) = SoundgasmSource::parse_page(html).expect("parsed");
        assert_eq!(id, "abc");
        assert_eq!(title, "Unknown title");
    }

    #[test]
    fn builds_stream_url() {
        let track = SoundgasmSource::build_track(
            "abc",
            "Clip".into(),
            "someone",
            "https://soundgasm.net/u/someone/clip",
        );
        assert_eq!(track.info.source_name, "soundgasm");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://media.soundgasm.net/sounds/abc.m4a")
        );
        assert_eq!(track.info.author, "someone");
    }

    #[test]
    fn page_without_audio_is_none() {
        assert!(SoundgasmSource::parse_page("<html>no media here</html>").is_none());
    }
}
