// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Twitter / X source.
//!
//! Resolves `twitter.com/<user>/status/<id>` and `x.com/...` links to the best
//! MP4 variant, falling back to the HLS playlist. Resolution goes through the
//! public GraphQL `TweetResultByRestId` operation, which requires a guest
//! token; the token is fetched once and refreshed on expiry. No account or API
//! key is required.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use tracing::debug;

use crate::{
    config::sources::TwitterConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::SourcePlugin,
};

/// Public bearer token embedded in Twitter's web client.
const BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const GRAPHQL_OPERATION: &str = "2ICDjqPd81tulZcYrtpTuQ/TweetResultByRestId";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TOKEN_TTL: Duration = Duration::from_secs(3 * 3600);

fn status_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:(?:www|m(?:obile)?)\.)?(?:twitter|x)\.com/[^/]+/status/(\d+)")
            .expect("valid regex")
    })
}

struct GuestToken {
    value: String,
    expires: Instant,
}

pub struct TwitterSource {
    http: Arc<reqwest::Client>,
    token: Mutex<Option<GuestToken>>,
}

impl TwitterSource {
    pub fn new(_config: TwitterConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self {
            http,
            token: Mutex::new(None),
        })
    }

    async fn guest_token(&self) -> Option<String> {
        if let Ok(guard) = self.token.lock()
            && let Some(token) = guard.as_ref()
            && token.expires > Instant::now()
        {
            return Some(token.value.clone());
        }

        let response = self
            .http
            .post("https://api.twitter.com/1.1/guest/activate.json")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {BEARER}"))
            .send()
            .await
            .ok()?;
        let body: Value = response.json().await.ok()?;
        let value = body.get("guest_token")?.as_str()?.to_string();

        if let Ok(mut guard) = self.token.lock() {
            *guard = Some(GuestToken {
                value: value.clone(),
                expires: Instant::now() + TOKEN_TTL,
            });
        }
        Some(value)
    }

    /// Calls `TweetResultByRestId` and returns the tweet's `legacy` object.
    async fn fetch_tweet(&self, tweet_id: &str) -> Option<Value> {
        let token = self.guest_token().await?;
        let variables = json!({
            "tweetId": tweet_id,
            "withCommunity": false,
            "includePromotedContent": false,
            "withVoice": true
        });
        let features = json!({
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true
        });
        let url = format!(
            "https://twitter.com/i/api/graphql/{GRAPHQL_OPERATION}?variables={}&features={}",
            urlencoding::encode(&variables.to_string()),
            urlencoding::encode(&features.to_string())
        );

        let response = self
            .http
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {BEARER}"))
            .header("x-guest-token", token)
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-client-language", "en")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::REFERER, "https://twitter.com/")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Twitter GraphQL returned HTTP {}", response.status());
            return None;
        }
        let body: Value = response.json().await.ok()?;
        let result = body.get("data")?.get("tweetResult")?.get("result")?;
        // A tombstone/limited tweet wraps the real one under `tweet`.
        let result = result.get("tweet").unwrap_or(result);
        result.get("legacy").cloned()
    }

    /// Chooses the highest-bitrate MP4, else the HLS playlist.
    fn select_variant(media: &Value) -> Option<(String, bool)> {
        let variants = media
            .get("video_info")?
            .get("variants")?
            .as_array()?;

        let mut best_mp4: Option<(u64, String)> = None;
        let mut hls: Option<String> = None;
        for variant in variants {
            let Some(url) = variant.get("url").and_then(|u| u.as_str()) else {
                continue;
            };
            match variant.get("content_type").and_then(|c| c.as_str()) {
                Some("video/mp4") => {
                    let bitrate = variant
                        .get("bitrate")
                        .and_then(|b| b.as_u64())
                        .unwrap_or(0);
                    if best_mp4.as_ref().is_none_or(|(b, _)| bitrate > *b) {
                        best_mp4 = Some((bitrate, url.to_string()));
                    }
                }
                Some("application/x-mpegURL") => {
                    hls.get_or_insert_with(|| url.to_string());
                }
                _ => {}
            }
        }

        best_mp4
            .map(|(_, url)| (url, false))
            .or_else(|| hls.map(|url| (url, true)))
    }

    /// Picks the media entity that carries video info.
    fn video_media(legacy: &Value) -> Option<&Value> {
        legacy
            .get("extended_entities")?
            .get("media")?
            .as_array()?
            .iter()
            .find(|m| m.get("video_info").is_some())
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {

        let Some(caps) = status_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let tweet_id = caps[1].to_string();
        let Some(legacy) = self.fetch_tweet(&tweet_id).await else {
            return LoadResult::Empty {};
        };
        let Some(media) = Self::video_media(&legacy) else {
            return LoadResult::Empty {};
        };
        let Some((stream, is_hls)) = Self::select_variant(media) else {
            return LoadResult::Empty {};
        };

        let mut track = Track::new(TrackInfo {
            identifier: tweet_id.clone(),
            is_seekable: true,
            author: legacy
                .get("user")
                .and_then(|u| u.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("Twitter User")
                .to_string(),
            length: media
                .get("video_info")
                .and_then(|v| v.get("duration_millis"))
                .and_then(|d| d.as_u64())
                .unwrap_or(0),
            is_stream: is_hls,
            position: 0,
            title: legacy
                .get("full_text")
                .and_then(|t| t.as_str())
                .map(|t| t.lines().next().unwrap_or(t).to_string())
                .unwrap_or_else(|| "Twitter Video".to_string()),
            uri: Some(stream.clone()),
            artwork_url: media
                .get("media_url_https")
                .and_then(|u| u.as_str())
                .map(str::to_string),
            isrc: None,
            source_name: "twitter".to_string(),
        });
        track.plugin_info = json!({ "originalUrl": identifier, "isHLS": is_hls });
        LoadResult::Track(track)
    }
}

#[async_trait]
impl SourcePlugin for TwitterSource {
    fn name(&self) -> &str {
        "twitter"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        status_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Twitter resolving: {}", identifier);
        self.resolve(identifier).await
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_twitter_and_x_status_urls() {
        assert!(status_pattern().is_match("https://twitter.com/user/status/1234567890"));
        assert!(status_pattern().is_match("https://x.com/user/status/1234567890"));
        assert!(status_pattern().is_match("https://mobile.twitter.com/u/status/42"));
        assert!(!status_pattern().is_match("https://twitter.com/user"));
    }

    #[test]
    fn prefers_highest_bitrate_mp4() {
        let media = serde_json::json!({
            "video_info": { "variants": [
                { "content_type": "video/mp4", "bitrate": 256000, "url": "https://cdn/low.mp4" },
                { "content_type": "video/mp4", "bitrate": 2176000, "url": "https://cdn/high.mp4" },
                { "content_type": "application/x-mpegURL", "url": "https://cdn/playlist.m3u8" }
            ] }
        });
        let (url, is_hls) = TwitterSource::select_variant(&media).unwrap();
        assert_eq!(url, "https://cdn/high.mp4");
        assert!(!is_hls);
    }

    #[test]
    fn falls_back_to_hls_when_no_mp4() {
        let media = serde_json::json!({
            "video_info": { "variants": [
                { "content_type": "application/x-mpegURL", "url": "https://cdn/playlist.m3u8" }
            ] }
        });
        let (url, is_hls) = TwitterSource::select_variant(&media).unwrap();
        assert_eq!(url, "https://cdn/playlist.m3u8");
        assert!(is_hls);
    }

    #[test]
    fn returns_none_without_video_variants() {
        let media = serde_json::json!({ "video_info": { "variants": [] } });
        assert!(TwitterSource::select_variant(&media).is_none());
    }

    #[test]
    fn finds_video_media_entity() {
        let legacy = serde_json::json!({
            "extended_entities": { "media": [
                { "type": "photo" },
                { "type": "video", "video_info": { "variants": [] } }
            ] }
        });
        assert!(TwitterSource::video_media(&legacy).is_some());
        assert!(TwitterSource::video_media(&serde_json::json!({})).is_none());
    }
}
