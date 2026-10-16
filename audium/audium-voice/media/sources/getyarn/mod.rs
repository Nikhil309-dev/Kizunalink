// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! getyarn.io clip source.
//!
//! Resolves `getyarn.io/yarn-clip/<id>` links to the deterministic
//! `y.yarn.co/<id>.mp4` stream URL. The clip id fully determines the media URL,
//! so no page fetch is required. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    config::sources::GetyarnConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

fn clip_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https?://(?:www\.)?getyarn\.io/yarn-clip/(?<id>[A-Za-z0-9_-]+)")
            .expect("valid regex")
    })
}

pub struct GetyarnSource;

impl GetyarnSource {
    pub fn new(_config: GetyarnConfig, _http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self)
    }

    fn build_track(id: &str, original_url: &str) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: "getyarn.io".to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title: id.to_string(),
            uri: Some(format!("https://y.yarn.co/{id}.mp4?v=0")),
            artwork_url: Some(format!("https://y.yarn.co/{id}_screenshot.jpg")),
            isrc: None,
            source_name: "getyarn".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        track
    }
}

#[async_trait]
impl SourcePlugin for GetyarnSource {
    fn name(&self) -> &str {
        "getyarn"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        clip_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Getyarn resolving: {}", identifier);
        let Some(caps) = clip_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let id = &caps["id"];
        LoadResult::Track(Self::build_track(id, identifier))
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

    fn source() -> GetyarnSource {
        GetyarnSource::new(GetyarnConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_yarn_clip_urls() {
        let s = source();
        assert!(s.can_handle("https://getyarn.io/yarn-clip/8f2c-1234"));
        assert!(s.can_handle("http://www.getyarn.io/yarn-clip/abc_def"));
        assert!(!s.can_handle("https://getyarn.io/yarn-find?text=hi"));
        assert!(!s.can_handle("https://example.com/yarn-clip/1"));
    }

    #[test]
    fn builds_deterministic_stream_url() {
        let track =
            GetyarnSource::build_track("8f2c-1234", "https://getyarn.io/yarn-clip/8f2c-1234");
        assert_eq!(track.info.source_name, "getyarn");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://y.yarn.co/8f2c-1234.mp4?v=0")
        );
        assert_eq!(
            track.info.artwork_url.as_deref(),
            Some("https://y.yarn.co/8f2c-1234_screenshot.jpg")
        );
        assert_eq!(track.info.identifier, "8f2c-1234");
    }
}
