// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! TikTok video source.
//!
//! Resolves `tiktok.com/@user/video/<id>`, `vm.tiktok.com/<code>` and
//! `m.tiktok.com/<code>` links through the public player API
//! (`www.tiktok.com/player/api/v1/items`), which requires no account or key.
//! Short links are followed once to recover the numeric video id. The selected
//! CDN MP4 URL is stored as the track `uri`; decoding is handled by the `http`
//! source.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::TiktokConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const API_BASE: &str = "https://www.tiktok.com/player/api/v1/items";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

fn video_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.|m\.)?tiktok\.com/(?:@[\w.\-]+/video|v)/(\d{15,})")
            .expect("valid regex")
    })
}

fn short_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:vm|vt)\.tiktok\.com/([\w-]+)").expect("valid regex")
    })
}

pub struct TiktokSource {
    http: Arc<reqwest::Client>,
}

impl TiktokSource {
    pub fn new(_config: TiktokConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    /// Follows a `vm.tiktok.com` short link once and extracts the numeric id.
    async fn expand_short_link(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .ok()?;
        let final_url = response.url().to_string();
        if let Some(caps) = video_pattern().captures(&final_url) {
            return Some(caps[1].to_string());
        }
        // The id can also live in a redirect body or a `share/video` path.
        let body = response.text().await.ok()?;
        let re = Regex::new(r"/video/(\d{15,})").ok()?;
        re.captures(&body).map(|c| c[1].to_string())
    }

    async fn fetch_item(&self, video_id: &str) -> Option<Value> {
        let url = format!(
            "{API_BASE}?item_ids={video_id}&language=en&aid=1284&app_name=tiktok_web&device_platform=web_pc"
        );
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(
                reqwest::header::REFERER,
                format!("https://www.tiktok.com/player/v1/{video_id}"),
            )
            .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("TikTok API returned HTTP {}", response.status());
            return None;
        }
        let value: Value = response.json().await.ok()?;
        value.get("items")?.as_array()?.first().cloned()
    }

    fn build_track(&self, item: &Value, video_id: &str, original_url: &str) -> Option<Track> {
        let video_info = item.get("video_info")?;
        let play_url = first_url(
            video_info
                .get("profiles")
                .and_then(|p| p.as_array())
                .and_then(|p| p.first())
                .and_then(|p| p.get("play_addr"))
                .and_then(|p| p.get("url_list")),
        )
        .or_else(|| first_url(video_info.get("url_list")))?;

        let author = item
            .get("author_info")
            .and_then(|a| a.get("nickname"))
            .and_then(|v| v.as_str())
            .unwrap_or("TikTok User")
            .to_string();
        let desc = item.get("desc").and_then(|v| v.as_str()).unwrap_or("");
        let title = desc.split('#').next().unwrap_or(desc).trim().to_string();
        let title = if title.is_empty() {
            "TikTok Video".to_string()
        } else {
            title
        };

        // TikTok reports duration in seconds for web payloads; normalise to ms.
        let raw = video_info
            .get("meta")
            .and_then(|m| m.get("duration"))
            .and_then(|d| d.as_u64())
            .unwrap_or(0);
        let length = match raw {
            0 => 0,
            n if n < 1000 => n * 1000,
            n => n,
        };

        let artwork = first_url(video_info.get("cover").and_then(|c| c.get("url_list")));

        let mut track = Track::new(TrackInfo {
            identifier: video_id.to_string(),
            is_seekable: true,
            author,
            length,
            is_stream: false,
            position: 0,
            title,
            uri: Some(play_url),
            artwork_url: artwork,
            isrc: None,
            source_name: "tiktok".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        Some(track)
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let video_id = if let Some(caps) = video_pattern().captures(identifier) {
            caps[1].to_string()
        } else if short_pattern().is_match(identifier) {
            match self.expand_short_link(identifier).await {
                Some(id) => id,
                None => return LoadResult::Empty {},
            }
        } else {
            return LoadResult::Empty {};
        };

        let Some(item) = self.fetch_item(&video_id).await else {
            return LoadResult::Empty {};
        };
        match self.build_track(&item, &video_id, identifier) {
            Some(track) => LoadResult::Track(track),
            None => LoadResult::Empty {},
        }
    }
}

/// Returns the first non-empty string from a JSON `url_list` array.
fn first_url(list: Option<&Value>) -> Option<String> {
    list?
        .as_array()?
        .iter()
        .find_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
}

#[async_trait]
impl SourcePlugin for TiktokSource {
    fn name(&self) -> &str {
        "tiktok"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        video_pattern().is_match(identifier) || short_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("TikTok resolving: {}", identifier);
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
    use serde_json::json;

    fn source() -> TiktokSource {
        TiktokSource::new(TiktokConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_video_urls() {
        let s = source();
        assert!(s.can_handle("https://www.tiktok.com/@alice/video/7300000000000000000"));
        assert!(s.can_handle("https://tiktok.com/@a.b-c/video/7300000000000000000"));
        assert!(s.can_handle("https://vm.tiktok.com/ZMabcDEF/"));
        assert!(!s.can_handle("https://www.tiktok.com/@alice"));
        assert!(!s.can_handle("https://example.com/video/7300000000000000000"));
    }

    #[test]
    fn builds_track_from_player_api_item() {
        let s = source();
        let item = json!({
            "author_info": { "nickname": "Alice" },
            "desc": "cool clip #fyp #viral",
            "video_info": {
                "meta": { "duration": 15 },
                "cover": { "url_list": ["https://cdn/cover.jpg"] },
                "profiles": [{ "play_addr": { "url_list": ["https://cdn/play.mp4"] } }]
            }
        });
        let track = s
            .build_track(
                &item,
                "7300000000000000000",
                "https://www.tiktok.com/@alice/video/7300000000000000000",
            )
            .expect("track");
        assert_eq!(track.info.title, "cool clip");
        assert_eq!(track.info.author, "Alice");
        assert_eq!(track.info.length, 15_000);
        assert_eq!(track.info.uri.as_deref(), Some("https://cdn/play.mp4"));
        assert_eq!(
            track.info.artwork_url.as_deref(),
            Some("https://cdn/cover.jpg")
        );
        assert!(!track.info.is_stream);
        assert_eq!(track.info.source_name, "tiktok");
    }

    #[test]
    fn prefers_profile_play_addr_and_normalises_ms() {
        let s = source();
        let item = json!({
            "author_info": { "nickname": "Bob" },
            "desc": "no tags here",
            "video_info": {
                "meta": { "duration": 45000 },
                "url_list": ["https://cdn/fallback.mp4"],
                "profiles": [{ "play_addr": { "url_list": ["https://cdn/preferred.mp4"] } }]
            }
        });
        let track = s
            .build_track(&item, "123", "https://tiktok.com/@b/video/123")
            .unwrap();
        assert_eq!(track.info.uri.as_deref(), Some("https://cdn/preferred.mp4"));
        assert_eq!(track.info.length, 45_000);
        assert_eq!(track.info.title, "no tags here");
    }

    #[test]
    fn rejects_item_without_play_url() {
        let s = source();
        let item = json!({ "author_info": {}, "desc": "x", "video_info": {} });
        assert!(s.build_track(&item, "1", "u").is_none());
    }
}
