// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Clyp.it clip source.
//!
//! Resolves `clyp.it/<id>` clips by reading the public `https://api.clyp.it/<id>`
//! JSON document and using its `Mp3Url` as the stream. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use tracing::debug;

use crate::{
    common::http::read_json_capped,
    config::sources::ClypitConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

fn clip_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https?://(?:www\.)?clyp\.it/(?<id>[A-Za-z0-9]+)/?(?:\?[^#]*)?(?:#.*)?$")
            .expect("valid regex")
    })
}

/// The subset of the `api.clyp.it` payload we consume.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ClypClip {
    #[serde(default)]
    title: String,
    #[serde(default)]
    mp3_url: String,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    username: String,
    #[serde(default)]
    picture_url: Option<String>,
}

pub struct ClypitSource {
    http: Arc<reqwest::Client>,
}

impl ClypitSource {
    pub fn new(_config: ClypitConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    fn build_track(clip: ClypClip, id: &str, original_url: &str) -> Option<Track> {
        if clip.mp3_url.is_empty() {
            return None;
        }
        let mut track = Track::new(TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: if clip.username.is_empty() {
                "Unknown artist".to_string()
            } else {
                clip.username
            },
            length: (clip.duration * 1000.0).round() as u64,
            is_stream: false,
            position: 0,
            title: if clip.title.is_empty() {
                id.to_string()
            } else {
                clip.title
            },
            uri: Some(clip.mp3_url),
            artwork_url: clip.picture_url,
            isrc: None,
            source_name: "clypit".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        Some(track)
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = clip_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let id = &caps["id"];
        let response = match self
            .http
            .get(format!("https://api.clyp.it/{id}"))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                debug!("Clypit request failed: {}", e);
                return LoadResult::Empty {};
            }
        };
        if !response.status().is_success() {
            debug!("Clypit returned HTTP {}", response.status());
            return LoadResult::Empty {};
        }
        let clip: ClypClip = match read_json_capped(response).await {
            Ok(c) => c,
            Err(e) => {
                debug!("Clypit JSON parse failed: {}", e);
                return LoadResult::Empty {};
            }
        };
        match Self::build_track(clip, id, identifier) {
            Some(track) => LoadResult::Track(track),
            None => LoadResult::Empty {},
        }
    }
}

#[async_trait]
impl SourcePlugin for ClypitSource {
    fn name(&self) -> &str {
        "clypit"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        clip_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Clypit resolving: {}", identifier);
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

    fn source() -> ClypitSource {
        ClypitSource::new(ClypitConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_clyp_urls() {
        let s = source();
        assert!(s.can_handle("https://clyp.it/abcdef"));
        assert!(s.can_handle("http://www.clyp.it/xy12"));
        assert!(!s.can_handle("https://clyp.it/user/abcdef"));
        assert!(!s.can_handle("https://example.com/abcdef"));
    }

    #[test]
    fn builds_track_from_api_payload() {
        let json = r#"{
            "Title": "My Clip",
            "Mp3Url": "https://cdn.clyp.it/abcdef.mp3",
            "Duration": 42.5,
            "Username": "Somebody",
            "PictureUrl": "https://cdn.clyp.it/abcdef.png"
        }"#;
        let clip: ClypClip = serde_json::from_str(json).expect("payload");
        let track = ClypitSource::build_track(clip, "abcdef", "https://clyp.it/abcdef").unwrap();
        assert_eq!(track.info.source_name, "clypit");
        assert_eq!(track.info.length, 42_500);
        assert_eq!(track.info.title, "My Clip");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://cdn.clyp.it/abcdef.mp3")
        );
        assert_eq!(
            track.info.artwork_url.as_deref(),
            Some("https://cdn.clyp.it/abcdef.png")
        );
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let json = r#"{"Mp3Url": "https://cdn.clyp.it/z.mp3"}"#;
        let clip: ClypClip = serde_json::from_str(json).expect("payload");
        let track = ClypitSource::build_track(clip, "z", "https://clyp.it/z").unwrap();
        assert_eq!(track.info.title, "z");
        assert_eq!(track.info.author, "Unknown artist");
        assert_eq!(track.info.length, 0);
    }

    #[test]
    fn empty_mp3_url_is_not_playable() {
        let json = r#"{"Title": "x"}"#;
        let clip: ClypClip = serde_json::from_str(json).expect("payload");
        assert!(ClypitSource::build_track(clip, "x", "https://clyp.it/x").is_none());
    }
}
