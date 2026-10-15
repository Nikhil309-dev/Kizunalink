// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Bilibili source.
//!
//! Resolves `bilibili.com/video/BV…` (and legacy `av…`), `b23.tv` short links,
//! and `bilibili.com/audio/au…` audio pages. Metadata comes from
//! `x/web-interface/view`; the playable URL is the anonymous DASH audio stream
//! from `x/player/wbi/playurl`, which requires a WBI signature. WBI keys are
//! fetched once from `x/web-interface/nav` and cached for an hour.
//!
//! The DASH stream host rejects requests without a `Referer`, so the resolved
//! URL is wrapped in an [`HttpTrack`] carrying the required headers. No account
//! is required unless a `sessdata` cookie is configured for higher quality.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use md5::{Digest, Md5};
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::BilibiliConfig,
    lavalink::protocol::tracks::{LoadResult, SearchResult, Track, TrackInfo},
    media::sources::{
        http::HttpTrack,
        plugin::{BoxedTrack, SourcePlugin},
    },
};

/// WBI mixin-key permutation table.
const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const WBI_TTL: Duration = Duration::from_secs(3600);

fn video_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?bilibili\.com/video/(BV[a-zA-Z0-9]+|av\d+)")
            .expect("valid regex")
    })
}

fn audio_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:www\.)?bilibili\.com/audio/au(\d+)").expect("valid regex")
    })
}

fn short_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^https?://b23\.tv/(\w+)").expect("valid regex"))
}

fn prefix_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(bilisearch|bilibili):").expect("valid regex"))
}

struct WbiCache {
    key: String,
    expires: Instant,
}

pub struct BilibiliSource {
    http: Arc<reqwest::Client>,
    sessdata: Option<String>,
    wbi: Mutex<Option<WbiCache>>,
}

impl BilibiliSource {
    pub fn new(config: BilibiliConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self {
            http,
            sessdata: config.sessdata.filter(|s| !s.is_empty()),
            wbi: Mutex::new(None),
        })
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static(USER_AGENT),
        );
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static("https://www.bilibili.com/"),
        );
        if let Some(sessdata) = &self.sessdata
            && let Ok(value) =
                reqwest::header::HeaderValue::from_str(&format!("SESSDATA={sessdata}"))
        {
            headers.insert(reqwest::header::COOKIE, value);
        }
        headers
    }

    /// Derives the WBI mixin key from the two key image URLs.
    fn mixin_key(img_key: &str, sub_key: &str) -> String {
        let raw = format!("{img_key}{sub_key}");
        let raw = raw.as_bytes();
        MIXIN_KEY_ENC_TAB
            .iter()
            .filter_map(|&i| raw.get(i).copied())
            .map(char::from)
            .take(32)
            .collect()
    }

    /// Signs query parameters with the mixin key, appending `wts` and `w_rid`.
    fn sign_params(params: &[(&str, String)], mixin_key: &str, timestamp: u64) -> String {
        let mut all: Vec<(&str, String)> = params.to_vec();
        all.push(("wts", timestamp.to_string()));
        all.sort_by(|a, b| a.0.cmp(b.0));

        let query = all
            .iter()
            .map(|(k, v)| {
                let value = v.replace(['!', '\'', '(', ')', '*'], "");
                format!("{}={}", urlencoding::encode(k), urlencoding::encode(&value))
            })
            .collect::<Vec<_>>()
            .join("&");

        let mut hasher = Md5::new();
        hasher.update(format!("{query}{mixin_key}").as_bytes());
        let w_rid = format!("{:x}", hasher.finalize());
        format!("{query}&w_rid={w_rid}")
    }

    /// Extracts the key filename stem from a WBI image URL.
    fn key_stem(url: &str) -> String {
        url.rsplit('/')
            .next()
            .and_then(|f| f.split('.').next())
            .unwrap_or_default()
            .to_string()
    }

    async fn wbi_key(&self) -> Option<String> {
        if let Ok(guard) = self.wbi.lock()
            && let Some(cache) = guard.as_ref()
            && cache.expires > Instant::now()
        {
            return Some(cache.key.clone());
        }

        let response = self
            .http
            .get("https://api.bilibili.com/x/web-interface/nav")
            .headers(self.headers())
            .send()
            .await
            .ok()?;
        let body: Value = crate::common::http::read_json_capped::<Value>(response)
            .await
            .ok()?;
        let img = body.get("data")?.get("wbi_img")?;
        let img_key = Self::key_stem(img.get("img_url")?.as_str()?);
        let sub_key = Self::key_stem(img.get("sub_url")?.as_str()?);
        let key = Self::mixin_key(&img_key, &sub_key);

        if let Ok(mut guard) = self.wbi.lock() {
            *guard = Some(WbiCache {
                key: key.clone(),
                expires: Instant::now() + WBI_TTL,
            });
        }
        Some(key)
    }

    async fn api_get(&self, url: &str) -> Option<Value> {
        let response = self
            .http
            .get(url)
            .headers(self.headers())
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Bilibili API returned HTTP {}", response.status());
            return None;
        }
        crate::common::http::read_json_capped::<Value>(response)
            .await
            .ok()
    }

    /// Resolves a `b23.tv` short link to its final URL.
    async fn expand_short(&self, identifier: &str) -> Option<String> {
        let response = self
            .http
            .get(identifier)
            .headers(self.headers())
            .send()
            .await
            .ok()?;
        Some(response.url().to_string())
    }

    /// Normalises a video URL into `(bvid_or_aid, cid)`.
    async fn video_ids(&self, identifier: &str) -> Option<(String, u64)> {
        let caps = video_pattern().captures(identifier)?;
        let id = caps[1].to_string();
        let query = if id.starts_with("BV") {
            format!("bvid={id}")
        } else {
            format!("aid={}", id.trim_start_matches("av"))
        };
        let body = self
            .api_get(&format!(
                "https://api.bilibili.com/x/web-interface/view?{query}"
            ))
            .await?;
        if body.get("code").and_then(|c| c.as_i64()) != Some(0) {
            return None;
        }
        let cid = body.get("data")?.get("cid")?.as_u64()?;
        Some((id, cid))
    }

    /// Fetches the anonymous DASH audio stream for a video.
    async fn playurl(&self, id: &str, cid: u64) -> Option<String> {
        let mixin = self.wbi_key().await?;
        let key = if id.starts_with("BV") { "bvid" } else { "aid" };
        let value = if id.starts_with("BV") {
            id.to_string()
        } else {
            id.trim_start_matches("av").to_string()
        };
        let params = vec![
            (key, value),
            ("cid", cid.to_string()),
            ("qn", "120".to_string()),
            ("fnval", "16".to_string()),
        ];
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let query = Self::sign_params(&params, &mixin, timestamp);

        let body = self
            .api_get(&format!(
                "https://api.bilibili.com/x/player/wbi/playurl?{query}"
            ))
            .await?;
        if body.get("code").and_then(|c| c.as_i64()) != Some(0) {
            return None;
        }
        let data = body.get("data")?;
        if let Some(url) = data
            .get("dash")
            .and_then(|d| d.get("audio"))
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|a| a.get("baseUrl"))
            .and_then(|u| u.as_str())
        {
            return Some(url.to_string());
        }
        // Some content only offers a merged `durl` MP4.
        data.get("durl")
            .and_then(|d| d.as_array())
            .and_then(|d| d.first())
            .and_then(|d| d.get("url"))
            .and_then(|u| u.as_str())
            .map(str::to_string)
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let url = if short_pattern().is_match(identifier) {
            match self.expand_short(identifier).await {
                Some(u) => u,
                None => return LoadResult::Empty {},
            }
        } else {
            identifier.to_string()
        };

        if audio_pattern().is_match(&url) {
            return self.resolve_audio(&url).await;
        }
        let Some((id, cid)) = self.video_ids(&url).await else {
            return LoadResult::Empty {};
        };
        let Some(stream) = self.playurl(&id, cid).await else {
            return LoadResult::Empty {};
        };
        LoadResult::Track(Self::build_track(&id, &url, stream))
    }

    /// Audio pages (`/audio/au…`) expose a single `x/player/playurl` result.
    async fn resolve_audio(&self, url: &str) -> LoadResult {
        let Some(caps) = audio_pattern().captures(url) else {
            return LoadResult::Empty {};
        };
        let au_id = caps[1].to_string();
        let body = self
            .api_get(&format!(
                "https://www.bilibili.com/audio/music-service-c/web/song/info?sid={au_id}"
            ))
            .await;
        let title = body
            .as_ref()
            .and_then(|b| b.get("data"))
            .and_then(|d| d.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("Bilibili Audio")
            .to_string();
        let body = self
            .api_get(&format!(
                "https://www.bilibili.com/audio/music-service-c/web/url?sid={au_id}"
            ))
            .await;
        let Some(stream) = body
            .as_ref()
            .and_then(|b| b.get("data"))
            .and_then(|d| d.get("cdns"))
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|u| u.as_str())
            .map(str::to_string)
        else {
            return LoadResult::Empty {};
        };

        let mut track = Track::new(TrackInfo {
            identifier: format!("au{au_id}"),
            is_seekable: true,
            author: "Bilibili".to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title,
            uri: Some(stream),
            artwork_url: None,
            isrc: None,
            source_name: "bilibili".to_string(),
        });
        track.plugin_info = serde_json::json!({ "auId": au_id });
        LoadResult::Track(track)
    }

    fn build_track(id: &str, url: &str, stream: String) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: "Bilibili".to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title: format!("Bilibili {id}"),
            uri: Some(stream),
            artwork_url: None,
            isrc: None,
            source_name: "bilibili".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": url });
        track
    }

    async fn search(&self, query: &str) -> Option<SearchResult> {
        let url = format!(
            "https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={}",
            urlencoding::encode(query)
        );
        let body = self.api_get(&url).await?;
        if body.get("code").and_then(|c| c.as_i64()) != Some(0) {
            return None;
        }
        let items = body.get("data")?.get("result")?.as_array()?;
        let tracks: Vec<Track> = items
            .iter()
            .filter_map(|item| {
                let bvid = item.get("bvid")?.as_str()?.to_string();
                Some(Track::new(TrackInfo {
                    identifier: bvid.clone(),
                    is_seekable: true,
                    author: item
                        .get("author")
                        .and_then(|a| a.as_str())
                        .unwrap_or("Unknown Artist")
                        .to_string(),
                    length: item
                        .get("duration")
                        .and_then(|d| d.as_str())
                        .map(|d| {
                            d.split(':')
                                .fold(0u64, |acc, p| acc * 60 + p.parse::<u64>().unwrap_or(0))
                        })
                        .unwrap_or(0)
                        .saturating_mul(1000),
                    is_stream: false,
                    position: 0,
                    title: item
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Unknown Title")
                        .replace("<em class=\"keyword\">", "")
                        .replace("</em>", ""),
                    uri: Some(format!("https://www.bilibili.com/video/{bvid}")),
                    artwork_url: item
                        .get("pic")
                        .and_then(|p| p.as_str())
                        .map(|p| p.replace("//", "https://")),
                    isrc: None,
                    source_name: "bilibili".to_string(),
                }))
            })
            .collect();
        if tracks.is_empty() {
            return None;
        }
        Some(SearchResult::from_tracks(tracks))
    }
}

#[async_trait]
impl SourcePlugin for BilibiliSource {
    fn name(&self) -> &str {
        "bilibili"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        video_pattern().is_match(identifier)
            || audio_pattern().is_match(identifier)
            || short_pattern().is_match(identifier)
            || prefix_pattern().is_match(identifier)
    }

    fn search_prefixes(&self) -> Vec<&str> {
        vec!["bilisearch:", "bilibili:"]
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Bilibili resolving: {}", identifier);
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

    async fn get_track(
        &self,
        identifier: &str,
        routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<BoxedTrack> {
        Some(Box::new(HttpTrack {
            url: identifier.to_string(),
            local_addr: routeplanner.and_then(|rp| rp.get_address()),
            proxy: None,
            headers: Some(self.headers()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> BilibiliSource {
        BilibiliSource::new(BilibiliConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_video_audio_and_short_urls() {
        let s = source();
        assert!(s.can_handle("https://www.bilibili.com/video/BV1xx411c7mD"));
        assert!(s.can_handle("https://bilibili.com/video/av170001"));
        assert!(s.can_handle("https://www.bilibili.com/audio/au12345"));
        assert!(s.can_handle("https://b23.tv/abcdefg"));
        assert!(!s.can_handle("https://www.bilibili.com/read/cv123"));
    }

    #[test]
    fn extracts_video_id_and_cid_source() {
        let caps = video_pattern()
            .captures("https://www.bilibili.com/video/BV1xx411c7mD?p=2")
            .unwrap();
        assert_eq!(&caps[1], "BV1xx411c7mD");
        let caps = video_pattern()
            .captures("https://bilibili.com/video/av170001")
            .unwrap();
        assert_eq!(&caps[1], "av170001");
    }

    #[test]
    fn derives_mixin_key_from_key_stems() {
        // The table takes the first 32 permuted characters of img_key+sub_key.
        let key = BilibiliSource::mixin_key(
            "abcdefghijklmnopqrstuvwxyz012345",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ678901",
        );
        assert_eq!(key.len(), 32);
        assert_eq!(key, "OPscVixApSk56dND1LfRBjKt32oHmGJn");
    }

    #[test]
    fn signs_params_deterministically() {
        let params = vec![("bvid", "BV1xx".to_string()), ("cid", "123".to_string())];
        let signed = BilibiliSource::sign_params(&params, "mixin", 1_700_000_000);
        assert!(signed.starts_with("bvid=BV1xx&cid=123&wts=1700000000"));
        assert!(signed.contains("&w_rid="));
        // Same inputs must produce the same signature.
        assert_eq!(
            signed,
            BilibiliSource::sign_params(&params, "mixin", 1_700_000_000)
        );
    }

    #[test]
    fn extracts_key_stem_from_url() {
        assert_eq!(
            BilibiliSource::key_stem(
                "https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"
            ),
            "7cd084941338484aae1ad9425b84077c"
        );
    }

    #[test]
    fn reads_dash_audio_base_url() {
        let body: Value = serde_json::json!({
            "code": 0,
            "data": { "dash": { "audio": [ { "baseUrl": "https://cdn/audio.m4s" } ] } }
        });
        let url = body["data"]["dash"]["audio"][0]["baseUrl"].as_str();
        assert_eq!(url, Some("https://cdn/audio.m4s"));
    }
}
