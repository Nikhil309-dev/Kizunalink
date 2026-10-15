// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Songlink / Odesli cross-platform resolver.
//!
//! Given a song URL, asks the keyless Odesli API (`api.song.link`) for
//! equivalent links on other platforms, then hands the platform URLs to the
//! source manager via [`SourcePlugin::translate`]. The manager re-dispatches
//! them in preference order, so a Spotify link can be played through Deezer or
//! Apple Music when only those sources are configured. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::SonglinkConfig,
    lavalink::protocol::tracks::LoadResult,
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const API_BASE: &str = "https://api.song.link/v1-alpha.1/links";

/// Platforms Odesli reports, mapped to the source that owns them, in the order
/// they should be tried. Native Audium sources come first; anything requiring a
/// source Audium does not ship is simply skipped by the manager.
const PLATFORM_SOURCES: &[(&str, &str)] = &[
    ("spotify", "spotify"),
    ("appleMusic", "applemusic"),
    ("deezer", "deezer"),
    ("tidal", "tidal"),
    ("soundcloud", "soundcloud"),
    ("youtube", "youtube"),
    ("youtubeMusic", "youtube"),
    ("amazonMusic", "amazonmusic"),
    ("pandora", "pandora"),
    ("yandex", "yandexmusic"),
    ("vk", "vkmusic"),
    ("audius", "audius"),
];

/// Song URLs the resolver will forward to Odesli.
fn song_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^https?://(?:www\.)?(?:open\.spotify\.com|music\.apple\.com|deezer\.com|tidal\.com|soundcloud\.com|music\.youtube\.com|youtube\.com|music\.amazon\.|www\.amazon\.|pandora\.com|music\.yandex\.|audius\.co)/",
        )
        .expect("valid regex")
    })
}

pub struct SonglinkSource {
    http: Arc<reqwest::Client>,
}

impl SonglinkSource {
    pub fn new(_config: SonglinkConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_links(&self, url: &str) -> Option<Value> {
        let request_url = format!("{API_BASE}?url={}&userCountry=US", urlencoding::encode(url));
        let response = self
            .http
            .get(&request_url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Odesli returned HTTP {}", response.status());
            return None;
        }
        crate::common::http::read_json_capped::<serde_json::Value>(response)
            .await
            .ok()
    }

    /// Builds the ordered list of equivalent platform URLs.
    fn candidate_urls(body: &Value) -> Vec<String> {
        let Some(links) = body.get("linksByPlatform").and_then(|l| l.as_object()) else {
            return Vec::new();
        };
        PLATFORM_SOURCES
            .iter()
            .filter_map(|(platform, _)| {
                links
                    .get(*platform)
                    .and_then(|p| p.get("url"))
                    .and_then(|u| u.as_str())
                    .map(str::to_string)
            })
            .collect()
    }
}

#[async_trait]
impl SourcePlugin for SonglinkSource {
    fn name(&self) -> &str {
        "songlink"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        song_pattern().is_match(identifier)
    }

    /// A mirror-style resolver: it never serves media itself.
    fn is_mirror(&self) -> bool {
        true
    }

    async fn translate(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Vec<String> {
        debug!("Songlink resolving: {}", identifier);
        match self.fetch_links(identifier).await {
            Some(body) => Self::candidate_urls(&body),
            None => Vec::new(),
        }
    }

    async fn load(
        &self,
        _identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        LoadResult::Empty {}
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

    #[test]
    fn recognises_supported_song_urls() {
        assert!(song_pattern().is_match("https://open.spotify.com/track/abc"));
        assert!(song_pattern().is_match("https://music.apple.com/us/album/x/1"));
        assert!(song_pattern().is_match("https://www.deezer.com/track/123"));
        assert!(!song_pattern().is_match("https://example.com/track/abc"));
    }

    #[test]
    fn orders_candidates_by_platform_preference() {
        let body = json!({
            "linksByPlatform": {
                "youtube": { "url": "https://youtube.com/watch?v=1" },
                "deezer": { "url": "https://deezer.com/track/2" },
                "spotify": { "url": "https://open.spotify.com/track/3" }
            }
        });
        let urls = SonglinkSource::candidate_urls(&body);
        assert_eq!(urls[0], "https://open.spotify.com/track/3");
        assert_eq!(urls[1], "https://deezer.com/track/2");
        assert_eq!(urls[2], "https://youtube.com/watch?v=1");
    }

    #[test]
    fn skips_platforms_without_a_url() {
        let body = json!({
            "linksByPlatform": {
                "spotify": { "id": "x" },
                "deezer": { "url": "https://deezer.com/track/2" }
            }
        });
        assert_eq!(
            SonglinkSource::candidate_urls(&body),
            vec!["https://deezer.com/track/2".to_string()]
        );
    }

    #[test]
    fn handles_missing_links_by_platform() {
        assert!(SonglinkSource::candidate_urls(&json!({})).is_empty());
    }
}
