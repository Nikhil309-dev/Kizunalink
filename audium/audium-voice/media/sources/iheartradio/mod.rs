// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! iHeartRadio live-station source.
//!
//! Resolves publicly listed iHeart *live* radio stations (non-seekable streams)
//! through the keyless public catalog API. On-demand songs are deliberately not
//! exposed: their stream URLs are DRM/session protected and not portable. The
//! station's secure stream URL is stored as the track `uri`, so playback flows
//! through the `http` source.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::StreamExt;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::IheartradioConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const API_V1: &str = "https://api2.iheart.com/api/v1";
const API_V2: &str = "https://us.api.iheart.com/api/v2";
const DEFAULT_SEARCH_LIMIT: usize = 10;
const HARD_SEARCH_LIMIT: usize = 50;

fn url_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?iheart\.com/live/(?:[a-z0-9-]+-)?(\d+)/?$")
            .expect("valid regex")
    })
}

/// Stream keys in preference order. `secure_*` first, then the plain variants.
const STREAM_KEYS: &[&str] = &[
    "secure_shoutcast_stream",
    "secure_mp3_pls_stream",
    "secure_hls_stream",
    "shoutcast_stream",
    "mp3_pls_stream",
    "hls_stream",
];

pub struct IheartradioSource {
    http: Arc<reqwest::Client>,
    search_limit: usize,
}

impl IheartradioSource {
    pub fn new(config: IheartradioConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        let search_limit = if config.search_limit == 0 {
            DEFAULT_SEARCH_LIMIT
        } else {
            config.search_limit.min(HARD_SEARCH_LIMIT)
        };
        Ok(Self { http, search_limit })
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header(reqwest::header::ORIGIN, "https://www.iheart.com")
            .header(reqwest::header::REFERER, "https://www.iheart.com/")
            .header(reqwest::header::ACCEPT, "application/json")
    }

    async fn fetch_json(&self, url: &str) -> Option<Value> {
        let response = self.request(url).send().await.ok()?;
        if !response.status().is_success() {
            debug!("iHeart API returned HTTP {}", response.status());
            return None;
        }
        crate::common::http::read_json_capped::<serde_json::Value>(response)
            .await
            .ok()
    }

    /// Extract the live-station object from a v2 `liveStations/{id}` response.
    fn extract_station<'a>(&self, data: &'a Value) -> Option<&'a Value> {
        data.get("hits")
            .and_then(|h| h.as_array())
            .and_then(|a| a.first())
            .or_else(|| data.get("station"))
    }

    fn select_stream(station: &Value) -> Option<String> {
        let streams = station.get("streams")?.as_object()?;
        for key in STREAM_KEYS {
            if let Some(url) = streams.get(*key).and_then(|v| v.as_str())
                && !url.is_empty()
            {
                return Some(url.to_string());
            }
        }
        None
    }

    fn build_track(&self, station: &Value) -> Option<Track> {
        let id = station
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .or_else(|| {
                station
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })?;

        let stream = Self::select_stream(station)?;

        let name = station
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| station.get("callLetters").and_then(|v| v.as_str()))
            .unwrap_or("iHeartRadio Station")
            .to_string();
        let city = station
            .get("market")
            .and_then(|m| m.get("cityName"))
            .and_then(|v| v.as_str())
            .or_else(|| station.get("city").and_then(|v| v.as_str()));

        Some(Track::new(TrackInfo {
            identifier: id.clone(),
            is_seekable: false,
            author: city.unwrap_or("iHeartRadio").to_string(),
            length: 0,
            is_stream: true,
            position: 0,
            title: name,
            uri: Some(stream),
            artwork_url: station
                .get("logo")
                .and_then(|v| v.as_str())
                .or_else(|| station.get("newThumbnailUrl").and_then(|v| v.as_str()))
                .map(str::to_string),
            isrc: None,
            source_name: "iheartradio".to_string(),
        }))
    }

    async fn resolve_station(&self, station_id: &str) -> LoadResult {
        let Some(data) = self
            .fetch_json(&format!("{API_V2}/content/liveStations/{station_id}"))
            .await
        else {
            return LoadResult::Empty {};
        };
        match self
            .extract_station(&data)
            .and_then(|s| self.build_track(s))
        {
            Some(track) => LoadResult::Track(track),
            None => LoadResult::Empty {},
        }
    }

    async fn search(&self, query: &str) -> Vec<Track> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "{API_V1}/catalog/searchAll?keywords={encoded}&bestMatch=True\
             &queryStation=True&queryArtist=False&queryTrack=False&queryTalkShow=True\
             &startIndex=0&maxRows={}&queryFeaturedStation=True&queryBundle=False\
             &queryTalkTheme=False&amp_version=4.11.0",
            self.search_limit
        );
        let Some(data) = self.fetch_json(&url).await else {
            return Vec::new();
        };
        let Some(stations) = data.get("stations").and_then(|s| s.as_array()) else {
            return Vec::new();
        };

        // The search response omits stream URLs, so resolve each station's live
        // detail concurrently (bounded by the same search limit).
        let ids: Vec<String> = stations
            .iter()
            .take(self.search_limit)
            .filter_map(|s| {
                s.get("id")
                    .and_then(|v| v.as_i64())
                    .map(|v| v.to_string())
                    .or_else(|| s.get("id").and_then(|v| v.as_str()).map(str::to_string))
            })
            .collect();

        let fetches = futures::stream::iter(ids.into_iter().map(|id| async move {
            let data = self
                .fetch_json(&format!("{API_V2}/content/liveStations/{id}"))
                .await?;
            self.extract_station(&data)
                .and_then(|s| self.build_track(s))
        }))
        // Bound fan-out so a search cannot open dozens of sockets at once.
        .buffer_unordered(5);
        futures::StreamExt::collect::<Vec<Option<Track>>>(fetches)
            .await
            .into_iter()
            .flatten()
            .collect()
    }
}

#[async_trait]
impl SourcePlugin for IheartradioSource {
    fn name(&self) -> &str {
        "iheartradio"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        identifier.starts_with("ihsearch:") || url_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        if let Some(query) = identifier.strip_prefix("ihsearch:") {
            let query = query.trim();
            if query.is_empty() {
                return LoadResult::Empty {};
            }
            let tracks = self.search(query).await;
            return match tracks.len() {
                0 => LoadResult::Empty {},
                1 => LoadResult::Track(tracks.into_iter().next().expect("len == 1")),
                _ => LoadResult::Search(tracks),
            };
        }

        let Some(caps) = url_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let station_id = &caps[1];
        debug!("iHeartRadio resolving station {station_id}");
        self.resolve_station(station_id).await
    }

    async fn load_search(
        &self,
        query: &str,
        _types: &[String],
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<crate::lavalink::protocol::tracks::SearchResult> {
        let q = query.strip_prefix("ihsearch:")?.trim();
        if q.is_empty() {
            return None;
        }
        let tracks = self.search(q).await;
        Some(crate::lavalink::protocol::tracks::SearchResult {
            tracks,
            albums: Vec::new(),
            artists: Vec::new(),
            playlists: Vec::new(),
            texts: Vec::new(),
            plugin: serde_json::json!({}),
        })
    }

    async fn get_track(
        &self,
        _identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<BoxedTrack> {
        // Live stations carry their stream URL as `uri`; resolution flows through
        // the `http` source.
        None
    }

    fn search_prefixes(&self) -> Vec<&str> {
        vec!["ihsearch:"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source() -> IheartradioSource {
        IheartradioSource::new(
            IheartradioConfig::default(),
            Arc::new(reqwest::Client::new()),
        )
        .expect("valid source")
    }

    fn station(streams: Value) -> Value {
        json!({
            "id": 8517,
            "name": "LoFi - Relax",
            "callLetters": "ACIRDS41-FL",
            "logo": "https://i.iheart.com/logo.png",
            "market": { "cityName": "Monterrey" },
            "streams": streams
        })
    }

    #[test]
    fn recognises_station_urls_only() {
        let s = source();
        assert!(s.can_handle("https://www.iheart.com/live/8517/"));
        assert!(s.can_handle("https://iheart.com/live/lofi-relax-8517"));
        // A bare site URL must not be claimed as a station.
        assert!(!s.can_handle("https://www.iheart.com/"));
        // The search prefix is claimed so `load` can dispatch it.
        assert!(s.can_handle("ihsearch:lofi"));
    }

    #[test]
    fn prefers_secure_stream_keys() {
        let streams = json!({
            "hls_stream": "http://insecure/hls.m3u8",
            "secure_shoutcast_stream": "https://secure/stream.aac"
        });
        let s = source();
        let track = s.build_track(&station(streams)).expect("track");
        assert_eq!(track.info.uri.as_deref(), Some("https://secure/stream.aac"));
        assert!(track.info.is_stream);
        assert!(!track.info.is_seekable);
        assert_eq!(track.info.author, "Monterrey");
        assert_eq!(track.info.source_name, "iheartradio");
    }

    #[test]
    fn skips_station_without_streams() {
        let s = source();
        assert!(s.build_track(&station(json!({}))).is_none());
    }
}
