// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Newgrounds audio source.
//!
//! Resolves `newgrounds.com/audio/listen/<id>` pages by reading the public
//! `NgAudioPlayer.fromListenPage({…})` metadata block and using the reported
//! `audio-download.ngfiles.com` URL as the track `uri`. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    config::sources::NewgroundsConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn audio_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?newgrounds\.com/audio/listen/(\d+)")
            .expect("valid regex")
    })
}

pub struct NewgroundsSource {
    http: Arc<reqwest::Client>,
}

/// Metadata parsed out of the listen page.
struct AudioPage {
    identifier: String,
    stream_url: String,
    duration: f64,
    title: String,
    author: String,
}

impl NewgroundsSource {
    pub fn new(_config: NewgroundsConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_page(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Newgrounds returned HTTP {}", response.status());
            return None;
        }
        crate::common::http::read_text_capped(response).await.ok()
    }

    /// Parses the `NgAudioPlayer.fromListenPage({…})` literal.
    fn parse_page(html: &str) -> Option<AudioPage> {
        let re =
            Regex::new(r"NgAudioPlayer\.fromListenPage\(\s*\{([\s\S]*?)\}\s*,\s*\d+\s*\)").ok()?;
        let block = re.captures(html)?.get(1)?.as_str();

        let identifier = json_number(block, "generic_id")?.to_string();
        let stream_url = json_string(block, "url")?;
        let stream_url = if let Some(stripped) = stream_url.strip_prefix("//") {
            format!("https://{stripped}")
        } else {
            stream_url
        };
        let duration = json_number(block, "duration").unwrap_or(0.0);
        let title = json_string(block, "title")?;
        let author = json_string(block, "author")?;

        Some(AudioPage {
            identifier,
            stream_url,
            duration,
            title,
            author,
        })
    }

    fn build_track(&self, page: AudioPage, original_url: &str) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: page.identifier,
            is_seekable: true,
            author: page.author,
            length: (page.duration * 1000.0).round() as u64,
            is_stream: false,
            position: 0,
            title: page.title,
            uri: Some(page.stream_url),
            artwork_url: None,
            isrc: None,
            source_name: "newgrounds".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        track
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        if !audio_pattern().is_match(identifier) {
            return LoadResult::Empty {};
        }
        let Some(html) = self.fetch_page(identifier).await else {
            return LoadResult::Empty {};
        };
        match Self::parse_page(&html) {
            Some(page) => LoadResult::Track(self.build_track(page, identifier)),
            None => LoadResult::Empty {},
        }
    }
}

/// Extracts a JSON string field from a JS object literal.
fn json_string(block: &str, field: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"['"]{f}['"]\s*:\s*"((?:\\.|[^"\\])*)""#,
        f = regex::escape(field)
    ))
    .ok()?;
    let raw = re.captures(block)?.get(1)?.as_str();
    Some(raw.replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Extracts a JSON number field from a JS object literal.
fn json_number(block: &str, field: &str) -> Option<f64> {
    let re = Regex::new(&format!(
        r#"['"]{f}['"]\s*:\s*(-?\d+(?:\.\d+)?)"#,
        f = regex::escape(field)
    ))
    .ok()?;
    re.captures(block)?.get(1)?.as_str().parse().ok()
}

#[async_trait]
impl SourcePlugin for NewgroundsSource {
    fn name(&self) -> &str {
        "newgrounds"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        audio_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Newgrounds resolving: {}", identifier);
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

    fn source() -> NewgroundsSource {
        NewgroundsSource::new(
            NewgroundsConfig::default(),
            Arc::new(reqwest::Client::new()),
        )
        .expect("valid source")
    }

    #[test]
    fn recognises_listen_urls() {
        let s = source();
        assert!(s.can_handle("https://www.newgrounds.com/audio/listen/123456"));
        assert!(s.can_handle("https://newgrounds.com/audio/listen/1"));
        assert!(!s.can_handle("https://www.newgrounds.com/portal/view/123456"));
        assert!(!s.can_handle("https://example.com/audio/listen/123456"));
    }

    #[test]
    fn parses_player_metadata() {
        let html = r#"
            <script>
            NgAudioPlayer.fromListenPage({
              "url": "//audio-download.ngfiles.com/123000/123456_Song.mp3",
              "generic_id": 123456,
              "version": 3,
              "duration": 125.5,
              "title": "My Song",
              "author": "Artist"
            }, 42);
            </script>"#;
        let page = NewgroundsSource::parse_page(html).expect("page");
        assert_eq!(page.identifier, "123456");
        assert_eq!(
            page.stream_url,
            "https://audio-download.ngfiles.com/123000/123456_Song.mp3"
        );
        assert_eq!(page.title, "My Song");
        assert_eq!(page.author, "Artist");
        assert_eq!(page.duration, 125.5);
    }

    #[test]
    fn builds_track_with_ms_length() {
        let s = source();
        let html = r#"NgAudioPlayer.fromListenPage({
            "url":"https://audio-download.ngfiles.com/1/x.mp3",
            "generic_id":123456,"version":1,"duration":125.5,
            "title":"T","author":"A"}, 0);"#;
        let page = NewgroundsSource::parse_page(html).unwrap();
        let track = s.build_track(page, "https://www.newgrounds.com/audio/listen/123456");
        assert_eq!(track.info.length, 125_500);
        assert_eq!(track.info.source_name, "newgrounds");
        assert!(!track.info.is_stream);
    }

    #[test]
    fn returns_none_without_player_block() {
        assert!(NewgroundsSource::parse_page("<html>nothing</html>").is_none());
    }
}
