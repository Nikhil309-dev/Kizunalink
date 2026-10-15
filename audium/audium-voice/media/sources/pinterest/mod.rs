// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Pinterest pin source.
//!
//! Resolves `pinterest.<tld>/pin/<id>` videos through the keyless
//! `resource/PinResource/get` endpoint. The best available variant (720p, then
//! 540p, 360p, then any) is stored as the track `uri`. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Value, json};
use tracing::debug;

use crate::{
    config::sources::PinterestConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

/// Any Pinterest country TLD, including the dotted `co.uk`-style ones.
fn pin_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^https?://(?:[\w-]+\.)?pinterest\.(?:com|fr|de|ch|jp|cl|ca|it|co\.uk|nz|ru|com\.au|at|pt|co\.kr|es|com\.mx|dk|ph|th|co|nl|info|kr|ie|vn|ec|mx|in|pe|hu|id|tw|be|uk|com\.bo)/pin/(?:[\w-]+--)?(\d+)",
        )
        .expect("valid regex")
    })
}

pub struct PinterestSource {
    http: Arc<reqwest::Client>,
}

impl PinterestSource {
    pub fn new(_config: PinterestConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    fn resource_url(pin_id: &str) -> String {
        let data = json!({
            "options": {
                "field_set_key": "unauth_react_main_pin",
                "id": pin_id,
            }
        });
        format!(
            "https://www.pinterest.com/resource/PinResource/get/?data={}",
            urlencoding::encode(&data.to_string())
        )
    }

    async fn fetch_pin(&self, pin_id: &str) -> Option<Value> {
        let response = self
            .http
            .get(Self::resource_url(pin_id))
            .header("X-Pinterest-PWS-Handler", "www/[username].js")
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Pinterest returned HTTP {}", response.status());
            return None;
        }
        let body: Value = response.json().await.ok()?;
        body.get("resource_response")?.get("data").cloned()
    }

    /// Finds the video variant list on a standard pin or a story pin.
    fn video_list(data: &Value) -> Option<&Value> {
        if let Some(list) = data.get("videos").and_then(|v| v.get("video_list"))
            && list.is_object()
        {
            return Some(list);
        }
        data.get("story_pin_data")
            .and_then(|s| s.get("pages"))
            .and_then(|p| p.as_array())
            .and_then(|p| p.first())
            .and_then(|p| p.get("blocks"))
            .and_then(|b| b.as_array())?
            .iter()
            .find_map(|b| b.get("video")?.get("video_list"))
    }

    /// Picks 720p, then 540p, 360p, then the first variant present.
    fn preferred_variant(list: &Value) -> Option<&Value> {
        for key in ["V_720P", "V_540P", "V_360P"] {
            if let Some(v) = list.get(key) {
                return Some(v);
            }
        }
        list.as_object()?.values().next()
    }

    fn build_track(pin_id: &str, data: &Value, variant: &Value) -> Option<Track> {
        let uri = variant.get("url").and_then(|u| u.as_str())?.to_string();
        let author = data
            .get("closeup_attribution")
            .and_then(|a| a.get("full_name"))
            .or_else(|| data.get("pinner").and_then(|p| p.get("full_name")))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Artist")
            .to_string();
        let title = data
            .get("title")
            .or_else(|| data.get("grid_title"))
            .and_then(|v| v.as_str())
            .unwrap_or("Pinterest Video")
            .to_string();
        let length = variant
            .get("duration")
            .and_then(|d| d.as_f64())
            .filter(|d| d.is_finite())
            .map(|d| d.round() as u64)
            .unwrap_or(0);

        Some(Track::new(TrackInfo {
            identifier: pin_id.to_string(),
            is_seekable: true,
            author,
            length,
            is_stream: false,
            position: 0,
            title,
            uri: Some(uri),
            artwork_url: None,
            isrc: None,
            source_name: "pinterest".to_string(),
        }))
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = pin_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let pin_id = caps[1].to_string();
        let Some(data) = self.fetch_pin(&pin_id).await else {
            return LoadResult::Empty {};
        };
        let Some(variant) = Self::video_list(&data).and_then(Self::preferred_variant) else {
            return LoadResult::Empty {};
        };
        match Self::build_track(&pin_id, &data, variant) {
            Some(track) => LoadResult::Track(track),
            None => LoadResult::Empty {},
        }
    }
}

#[async_trait]
impl SourcePlugin for PinterestSource {
    fn name(&self) -> &str {
        "pinterest"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        pin_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Pinterest resolving: {}", identifier);
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
    fn recognises_pin_urls_across_tlds() {
        assert!(pin_pattern().is_match("https://www.pinterest.com/pin/123456789/"));
        assert!(pin_pattern().is_match("https://pinterest.co.uk/pin/987654321/"));
        assert!(pin_pattern().is_match("https://de.pinterest.com/pin/my-title--123456789/"));
        assert!(!pin_pattern().is_match("https://www.pinterest.com/search/pins/"));
        assert!(!pin_pattern().is_match("https://example.com/pin/123/"));
    }

    #[test]
    fn selects_preferred_variant_order() {
        let list = json!({
            "V_360P": { "url": "https://cdn/360.mp4" },
            "V_720P": { "url": "https://cdn/720.mp4" },
            "V_540P": { "url": "https://cdn/540.mp4" }
        });
        let v = PinterestSource::preferred_variant(&list).unwrap();
        assert_eq!(v["url"], "https://cdn/720.mp4");

        let only_low = json!({ "V_360P": { "url": "https://cdn/360.mp4" } });
        assert_eq!(
            PinterestSource::preferred_variant(&only_low).unwrap()["url"],
            "https://cdn/360.mp4"
        );
    }

    #[test]
    fn finds_video_list_on_story_pin() {
        let data = json!({
            "story_pin_data": {
                "pages": [ { "blocks": [
                    { "type": "image" },
                    { "video": { "video_list": { "V_720P": { "url": "https://cdn/s.mp4" } } } }
                ] } ]
            }
        });
        let list = PinterestSource::video_list(&data).expect("list");
        assert_eq!(list["V_720P"]["url"], "https://cdn/s.mp4");
    }

    #[test]
    fn builds_track_with_duration_and_author() {
        let data = json!({
            "closeup_attribution": { "full_name": "Alice" },
            "title": "My Pin",
            "videos": { "video_list": { "V_720P": { "url": "https://cdn/a.mp4", "duration": 12500 } } }
        });
        let variant = PinterestSource::video_list(&data)
            .and_then(PinterestSource::preferred_variant)
            .unwrap();
        let track = PinterestSource::build_track("123", &data, variant).unwrap();
        assert_eq!(track.info.title, "My Pin");
        assert_eq!(track.info.author, "Alice");
        assert_eq!(track.info.length, 12_500);
        assert_eq!(track.info.uri.as_deref(), Some("https://cdn/a.mp4"));
        assert_eq!(track.info.source_name, "pinterest");
    }

    #[test]
    fn returns_none_without_variants() {
        assert!(PinterestSource::video_list(&json!({})).is_none());
    }
}
