// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Bluesky (AT Protocol) post source.
//!
//! Resolves a public Bluesky post into its embedded video via the
//! unauthenticated public AppView API (`public.api.bsky.app`). For a
//! `bsky.app` / `bsky.social` post URL we first resolve the handle to a DID with
//! the public `resolveHandle` endpoint, then fetch the post record and use the
//! `app.bsky.embed.video` blob's CDN URL as the track `uri` (decoded later by
//! the `http` source). No credentials, no HTML scraping.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::BlueskyConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const PUBLIC_APPVIEW: &str = "https://public.api.bsky.app/xrpc";
const VIDEO_CDN: &str = "https://video.bsky.app/watch";
const CDN_BASE: &str = "https://cdn.bsky.app/img/feed_thumbnail/plain";

fn post_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?bsky\.app/profile/([^/\s]+)/post/([a-z0-9]+)")
            .expect("valid regex")
    })
}

pub struct BlueskySource {
    http: Arc<reqwest::Client>,
}

impl BlueskySource {
    pub fn new(_config: BlueskyConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_json(&self, url: &str) -> Option<Value> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Bluesky API returned HTTP {}", response.status());
            return None;
        }
        response.json().await.ok()
    }

    async fn resolve_handle(&self, handle: &str) -> Option<String> {
        if handle.starts_with("did:") {
            return Some(handle.to_string());
        }
        let data = self
            .fetch_json(&format!(
                "{PUBLIC_APPVIEW}/com.atproto.identity.resolveHandle?handle={}",
                urlencoding::encode(handle)
            ))
            .await?;
        data.get("did").and_then(|v| v.as_str()).map(str::to_string)
    }

    async fn fetch_post(&self, at_uri: &str) -> Option<Value> {
        let url = format!(
            "{PUBLIC_APPVIEW}/com.atproto.repo.getRecord?repo={}&collection=app.bsky.feed.post&rkey={}",
            urlencoding::encode(at_uri.split('/').nth(2).unwrap_or_default()),
            urlencoding::encode(at_uri.rsplit('/').next().unwrap_or_default()),
        );
        self.fetch_json(&url).await
    }

    /// Build a track from a `getRecord` response containing a video embed.
    fn build_track(&self, record: &Value, original_url: &str) -> Option<Track> {
        let value = record.get("value")?;
        let embed = value.get("embed")?;
        // Support both the current lexicon shape and the older `$type` shape.
        let video = embed
            .get("video")
            .or_else(|| embed.get("$type").is_some().then_some(embed))?;
        let did = record.get("uri")?.as_str()?.split('/').nth(2)?;
        let cid = video
            .get("ref")
            .and_then(|r| r.get("$link"))
            .and_then(|v| v.as_str())
            .or_else(|| video.get("cid").and_then(|v| v.as_str()))?;

        // The video CDN URL requires the blob CID; the poster comes from the
        // public thumbnail CDN.
        let stream = format!("{VIDEO_CDN}/{did}/{cid}/playlist.m3u8");
        let poster = video
            .get("thumbnail")
            .and_then(|v| v.as_str())
            .map(|thumb| {
                if thumb.starts_with("http") {
                    thumb.to_string()
                } else {
                    format!("{CDN_BASE}/{did}/{thumb}")
                }
            });

        let text = value
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let title = if text.is_empty() {
            "Bluesky video".to_string()
        } else {
            text.lines()
                .next()
                .unwrap_or(text)
                .chars()
                .take(120)
                .collect()
        };

        Some(Track::new(TrackInfo {
            identifier: record
                .get("uri")
                .and_then(|v| v.as_str())
                .unwrap_or(original_url)
                .to_string(),
            is_seekable: true,
            author: did.to_string(),
            length: 0,
            is_stream: true,
            position: 0,
            title,
            uri: Some(stream),
            artwork_url: poster,
            isrc: None,
            source_name: "bluesky".to_string(),
        }))
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = post_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let handle = &caps[1];
        let rkey = &caps[2];

        let Some(did) = self.resolve_handle(handle).await else {
            return LoadResult::Empty {};
        };
        let at_uri = format!("at://{did}/app.bsky.feed.post/{rkey}");

        let Some(record) = self.fetch_post(&at_uri).await else {
            return LoadResult::Empty {};
        };
        match self.build_track(&record, identifier) {
            Some(track) => LoadResult::Track(track),
            None => LoadResult::Empty {},
        }
    }
}

#[async_trait]
impl SourcePlugin for BlueskySource {
    fn name(&self) -> &str {
        "bluesky"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        post_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Bluesky resolving post: {}", identifier);
        self.resolve(identifier).await
    }

    async fn get_track(
        &self,
        _identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<BoxedTrack> {
        // The resolved CDN stream is carried as `uri` and decoded by `http`.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source() -> BlueskySource {
        BlueskySource::new(BlueskyConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_post_urls() {
        let s = source();
        assert!(s.can_handle("https://bsky.app/profile/alice.bsky.social/post/3kabc"));
        assert!(s.can_handle("https://www.bsky.app/profile/did:plc:xyz/post/3kabc"));
        assert!(!s.can_handle("https://bsky.app/profile/alice.bsky.social"));
        assert!(!s.can_handle("https://example.com/post/3kabc"));
    }

    #[test]
    fn builds_track_from_video_embed() {
        let s = source();
        let record = json!({
            "uri": "at://did:plc:abc/app.bsky.feed.post/3kxyz",
            "value": {
                "text": "Check this out\nsecond line",
                "embed": {
                    "video": {
                        "ref": { "$link": "bafyblobcid" },
                        "thumbnail": "bafythumb"
                    }
                }
            }
        });
        let track = s
            .build_track(&record, "https://bsky.app/profile/x/post/3kxyz")
            .expect("track");
        assert_eq!(track.info.title, "Check this out");
        assert_eq!(track.info.author, "did:plc:abc");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://video.bsky.app/watch/did:plc:abc/bafyblobcid/playlist.m3u8")
        );
        assert_eq!(
            track.info.artwork_url.as_deref(),
            Some("https://cdn.bsky.app/img/feed_thumbnail/plain/did:plc:abc/bafythumb")
        );
        assert!(track.info.is_stream);
    }

    #[test]
    fn rejects_post_without_video_embed() {
        let s = source();
        let record = json!({
            "uri": "at://did:plc:abc/app.bsky.feed.post/3kxyz",
            "value": { "text": "just text" }
        });
        assert!(
            s.build_track(&record, "https://bsky.app/profile/x/post/3kxyz")
                .is_none()
        );
    }
}
