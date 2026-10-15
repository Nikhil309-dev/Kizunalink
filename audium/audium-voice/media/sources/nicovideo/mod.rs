// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! NicoVideo source.
//!
//! Resolves `nicovideo.jp/watch/<id>` and `nico.ms/<id>` links, plus keyword
//! search through the snapshot API. Metadata comes from the watch page's
//! embedded JSON-LD. Playback uses the anonymous DMC session API:
//!
//! 1. read `accessRightKey` and `watchTrackId` from the page payload,
//! 2. POST them to `access-rights/hls` (expects `201` and a `Set-Cookie`),
//! 3. pick the audio-only variant from the master playlist.
//!
//! The returned HLS URL is session-bound, so the `Set-Cookie` value is attached
//! to the track and replayed by the HTTP source on every manifest and segment
//! request. No account is required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Value, json};
use tracing::debug;

use crate::{
    config::sources::NicovideoConfig,
    lavalink::protocol::tracks::{LoadResult, SearchResult, Track, TrackInfo},
    media::sources::{
        http::HttpTrack,
        plugin::{BoxedTrack, SourcePlugin},
    },
};

const WATCH_BASE: &str = "https://www.nicovideo.jp/watch";
const SNAPSHOT_SEARCH: &str =
    "https://api.search.nicovideo.jp/api/v2/snapshot/video/contents/search";
const HLS_ACCESS: &str = "https://nvapi.nicovideo.jp/v1/watch";
/// Quality labels NicoVideo exposes, best first.
const VIDEO_QUALITIES: &[&str] = &["1080p", "720p", "480p", "360p", "144p"];

fn url_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https?://(?:www\.)?(?:nicovideo\.jp/watch/(\w+)|nico\.ms/(\w+))")
            .expect("valid regex")
    })
}

fn prefix_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(ncsearch|nicovideo):").expect("valid regex"))
}

fn iso_duration_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?").expect("valid regex")
    })
}

pub struct NicovideoSource {
    http: Arc<reqwest::Client>,
}

impl NicovideoSource {
    pub fn new(_config: NicovideoConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    fn video_id(identifier: &str) -> Option<String> {
        url_pattern()
            .captures(identifier)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| m.as_str().to_string())
    }

    /// Parses an ISO-8601 duration (`PT4M5S`) into milliseconds.
    fn parse_iso_duration(value: &str) -> u64 {
        let Some(caps) = iso_duration_pattern().captures(value) else {
            return 0;
        };
        let num = |idx: usize| -> f64 {
            caps.get(idx)
                .and_then(|m| m.as_str().parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let seconds = num(1) * 3600.0 + num(2) * 60.0 + num(3);
        (seconds * 1000.0).round() as u64
    }

    fn build_headers() -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("Audium/NicoVideo"),
        );
        headers.insert(
            "X-Request-With",
            reqwest::header::HeaderValue::from_static("https://www.nicovideo.jp"),
        );
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static("https://www.nicovideo.jp/"),
        );
        headers.insert(
            "X-Frontend-Id",
            reqwest::header::HeaderValue::from_static("6"),
        );
        headers.insert(
            "X-Frontend-Version",
            reqwest::header::HeaderValue::from_static("0"),
        );
        headers
    }

    /// Reads the watch page with `responseType=json`, which returns the
    /// embedded `data` payload without HTML parsing.
    async fn fetch_watch_payload(&self, video_id: &str) -> Option<Value> {
        let url = format!("{WATCH_BASE}/{video_id}?responseType=json");
        let response = self
            .http
            .get(&url)
            .headers(Self::build_headers())
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("NicoVideo watch page returned HTTP {}", response.status());
            return None;
        }
        let body: Value = crate::common::http::read_json_capped::<serde_json::Value>(response)
            .await
            .ok()?;
        body.get("data").cloned()
    }

    fn json_ld(data: &Value) -> Option<&Value> {
        data.get("metadata")?
            .get("jsonLds")?
            .as_array()?
            .iter()
            .find(|entry| entry.get("@type").and_then(|t| t.as_str()) == Some("VideoObject"))
    }

    /// Builds the `outputs` pairs the access-rights endpoint expects: one entry
    /// per available video quality, each paired with the best audio track.
    fn build_outputs(dmc: &Value) -> Vec<Value> {
        let audios = dmc.get("audios").and_then(|a| a.as_array());
        let Some(audios) = audios else {
            return Vec::new();
        };

        let best_audio = audios
            .iter()
            .filter(|a| a.get("isAvailable").and_then(|v| v.as_bool()) == Some(true))
            .max_by_key(|a| a.get("qualityLevel").and_then(|q| q.as_i64()).unwrap_or(0))
            .and_then(|a| a.get("id").and_then(|i| i.as_str()));
        let Some(best_audio) = best_audio else {
            return Vec::new();
        };

        dmc.get("videos")
            .and_then(|v| v.as_array())
            .map(|videos| {
                videos
                    .iter()
                    .filter(|v| {
                        v.get("isAvailable").and_then(|x| x.as_bool()) == Some(true)
                            && v.get("label")
                                .and_then(|l| l.as_str())
                                .is_some_and(|l| VIDEO_QUALITIES.contains(&l))
                    })
                    .map(|v| {
                        json!([
                            v.get("id").and_then(|i| i.as_str()).unwrap_or(""),
                            best_audio
                        ])
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extracts the `Set-Cookie` value from a response.
    fn extract_cookie(response: &reqwest::Response) -> Option<String> {
        response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_string())
            .collect::<Vec<_>>()
            .join("; ")
            .into()
    }

    /// Picks the audio-only rendition from a master playlist, or the master
    /// itself when there is no `TYPE=AUDIO` entry.
    fn pick_audio_variant(master: &str, master_url: &str) -> String {
        let audio_uri = master
            .lines()
            .find(|l| l.starts_with("#EXT-X-MEDIA") && l.contains("TYPE=AUDIO"))
            .and_then(|l| Regex::new(r#"URI="([^"]+)""#).ok()?.captures(l))
            .map(|c| c[1].to_string());

        match audio_uri {
            Some(uri) if uri.starts_with("http") => uri,
            Some(uri) => format!(
                "{}/{}",
                master_url
                    .trim_end_matches('/')
                    .rsplit_once('/')
                    .map(|(b, _)| b)
                    .unwrap_or(master_url),
                uri.trim_start_matches('/')
            ),
            None => master_url.to_string(),
        }
    }

    /// Resolves the session-bound HLS URL for a video id.
    async fn resolve_stream(&self, video_id: &str) -> Option<(String, Option<String>)> {
        let data = self.fetch_watch_payload(video_id).await?;
        let response = data.get("response")?;
        let dmc = response.get("media")?.get("domand")?;
        let watch_track_id = response.get("client")?.get("watchTrackId")?.as_str()?;
        let access_right_key = dmc.get("accessRightKey")?.as_str()?;

        let mut headers = Self::build_headers();
        headers.insert(
            "x-access-right-key",
            reqwest::header::HeaderValue::from_str(access_right_key).ok()?,
        );

        let access_url = format!(
            "{HLS_ACCESS}/{video_id}/access-rights/hls?actionTrackId={}&__retry=1",
            urlencoding::encode(watch_track_id)
        );
        let response = self
            .http
            .post(&access_url)
            .headers(headers)
            .json(&json!({ "outputs": Self::build_outputs(dmc) }))
            .send()
            .await
            .ok()?;

        // The endpoint answers 201 Created with the session cookie in headers.
        if response.status().as_u16() != 201 {
            debug!(
                "NicoVideo access-rights returned HTTP {}",
                response.status()
            );
            return None;
        }
        let cookie = Self::extract_cookie(&response);
        let body: Value = crate::common::http::read_json_capped::<serde_json::Value>(response)
            .await
            .ok()?;
        let content_url = body.get("data")?.get("contentUrl")?.as_str()?.to_string();

        // The master playlist lists the audio-only rendition.
        let master = self
            .http
            .get(&content_url)
            .header(reqwest::header::COOKIE, cookie.clone().unwrap_or_default())
            .send()
            .await
            .ok()?;
        let master_text = crate::common::http::read_text_capped(master).await.ok()?;
        Some((Self::pick_audio_variant(&master_text, &content_url), cookie))
    }

    fn build_track(video_id: &str, json_ld: &Value) -> Track {
        let author = json_ld
            .get("author")
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown Artist")
            .to_string();
        let title = json_ld
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown Title")
            .to_string();
        let length = json_ld
            .get("duration")
            .and_then(|d| d.as_str())
            .map(Self::parse_iso_duration)
            .unwrap_or(0);
        let artwork = json_ld
            .get("thumbnailUrl")
            .and_then(|t| t.as_array())
            .and_then(|t| t.first())
            .and_then(|t| t.as_str())
            .map(str::to_string);

        let mut track = Track::new(TrackInfo {
            identifier: video_id.to_string(),
            is_seekable: true,
            author,
            length,
            is_stream: false,
            position: 0,
            title,
            uri: Some(format!("{WATCH_BASE}/{video_id}")),
            artwork_url: artwork,
            isrc: None,
            source_name: "nicovideo".to_string(),
        });
        track.plugin_info = json!({ "videoId": video_id });
        track
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(video_id) = Self::video_id(identifier) else {
            return LoadResult::Empty {};
        };
        let Some(data) = self.fetch_watch_payload(&video_id).await else {
            return LoadResult::Empty {};
        };
        let Some(json_ld) = Self::json_ld(&data) else {
            return LoadResult::Empty {};
        };
        LoadResult::Track(Self::build_track(&video_id, json_ld))
    }

    async fn search(&self, query: &str) -> Option<SearchResult> {
        let url = format!(
            "{SNAPSHOT_SEARCH}?q={}&targets=title,tags&fields=contentId,title,owner,thumbnailUrl,duration&_sort=-viewCounter&_context=Audium&_limit=25",
            urlencoding::encode(query)
        );
        let response = self.http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            debug!("NicoVideo search returned HTTP {}", response.status());
            return None;
        }
        let body: Value = crate::common::http::read_json_capped::<serde_json::Value>(response)
            .await
            .ok()?;
        let items = body.get("data")?.as_array()?;
        let tracks: Vec<Track> = items
            .iter()
            .filter_map(|item| {
                let id = item.get("contentId")?.as_str()?.to_string();
                let track = Track::new(TrackInfo {
                    identifier: id.clone(),
                    is_seekable: true,
                    author: item
                        .get("owner")
                        .and_then(|o| o.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("Unknown Artist")
                        .to_string(),
                    length: item
                        .get("duration")
                        .and_then(|d| d.as_u64())
                        .unwrap_or(0)
                        .saturating_mul(1000),
                    is_stream: false,
                    position: 0,
                    title: item
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Unknown Title")
                        .to_string(),
                    uri: Some(format!("{WATCH_BASE}/{id}")),
                    artwork_url: item
                        .get("thumbnailUrl")
                        .and_then(|t| t.as_str())
                        .map(str::to_string),
                    isrc: None,
                    source_name: "nicovideo".to_string(),
                });
                Some(track)
            })
            .collect();
        if tracks.is_empty() {
            return None;
        }
        Some(SearchResult::from_tracks(tracks))
    }
}

#[async_trait]
impl SourcePlugin for NicovideoSource {
    fn name(&self) -> &str {
        "nicovideo"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        url_pattern().is_match(identifier) || prefix_pattern().is_match(identifier)
    }

    fn search_prefixes(&self) -> Vec<&str> {
        vec!["ncsearch:", "nicovideo:"]
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("NicoVideo resolving: {}", identifier);
        self.resolve(identifier).await
    }

    async fn load_search(
        &self,
        query: &str,
        _types: &[String],
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<SearchResult> {
        let term = prefix_pattern().replace(query, "").trim().to_string();
        if term.is_empty() {
            return None;
        }
        self.search(&term).await
    }

    /// Establishes an anonymous DMC session and returns a header-bearing HLS
    /// track. Sessions are short-lived, so this runs at playback time rather
    /// than during `load`, matching NodeLink's `getTrackUrl`/`loadStream` split.
    async fn get_track(
        &self,
        identifier: &str,
        routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<BoxedTrack> {
        let video_id = Self::video_id(identifier)?;
        let (url, cookie) = self.resolve_stream(&video_id).await?;

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("Audium/NicoVideo"),
        );
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static("https://www.nicovideo.jp/"),
        );
        if let Some(cookie) = cookie {
            headers.insert(
                reqwest::header::COOKIE,
                reqwest::header::HeaderValue::from_str(&cookie).ok()?,
            );
        }

        Some(Box::new(HttpTrack {
            url,
            local_addr: routeplanner.and_then(|rp| rp.get_address()),
            proxy: None,
            headers: Some(headers),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_watch_and_short_urls() {
        assert!(url_pattern().is_match("https://www.nicovideo.jp/watch/sm9"));
        assert!(url_pattern().is_match("https://nico.ms/sm9"));
        assert!(!url_pattern().is_match("https://www.nicovideo.jp/user/123"));
    }

    #[test]
    fn extracts_video_id_from_both_forms() {
        assert_eq!(
            NicovideoSource::video_id("https://www.nicovideo.jp/watch/sm9").as_deref(),
            Some("sm9")
        );
        assert_eq!(
            NicovideoSource::video_id("https://nico.ms/sm9").as_deref(),
            Some("sm9")
        );
        assert_eq!(NicovideoSource::video_id("https://example.com/x"), None);
    }

    #[test]
    fn parses_iso_durations() {
        assert_eq!(NicovideoSource::parse_iso_duration("PT4M5S"), 245_000);
        assert_eq!(NicovideoSource::parse_iso_duration("PT1H2M3S"), 3_723_000);
        assert_eq!(NicovideoSource::parse_iso_duration("PT30S"), 30_000);
        assert_eq!(NicovideoSource::parse_iso_duration("nonsense"), 0);
    }

    #[test]
    fn picks_best_audio_and_available_qualities() {
        let dmc = json!({
            "audios": [
                { "id": "audio-low", "isAvailable": true, "qualityLevel": 1 },
                { "id": "audio-high", "isAvailable": true, "qualityLevel": 3 },
                { "id": "audio-off", "isAvailable": false, "qualityLevel": 9 }
            ],
            "videos": [
                { "id": "v720", "label": "720p", "isAvailable": true },
                { "id": "v1080", "label": "1080p", "isAvailable": false },
                { "id": "v-weird", "label": "dolby", "isAvailable": true }
            ]
        });
        let outputs = NicovideoSource::build_outputs(&dmc);
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], json!(["v720", "audio-high"]));
    }

    #[test]
    fn returns_no_outputs_without_audio() {
        let dmc = json!({ "audios": [], "videos": [{ "id": "v", "label": "720p", "isAvailable": true }] });
        assert!(NicovideoSource::build_outputs(&dmc).is_empty());
    }

    #[test]
    fn selects_audio_only_variant() {
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",URI=\"audio/playlist.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8\n";
        assert_eq!(
            NicovideoSource::pick_audio_variant(master, "https://cdn.example/master.m3u8"),
            "https://cdn.example/audio/playlist.m3u8"
        );
    }

    #[test]
    fn falls_back_to_master_without_audio_rendition() {
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo.m3u8\n";
        assert_eq!(
            NicovideoSource::pick_audio_variant(master, "https://cdn.example/master.m3u8"),
            "https://cdn.example/master.m3u8"
        );
    }

    #[test]
    fn finds_video_object_json_ld() {
        let data = json!({
            "metadata": { "jsonLds": [
                { "@type": "BreadcrumbList" },
                { "@type": "VideoObject", "name": "Song", "author": { "name": "Alice" }, "duration": "PT4M5S" }
            ] }
        });
        let ld = NicovideoSource::json_ld(&data).unwrap();
        let track = NicovideoSource::build_track("sm9", ld);
        assert_eq!(track.info.title, "Song");
        assert_eq!(track.info.author, "Alice");
        assert_eq!(track.info.length, 245_000);
    }
}
