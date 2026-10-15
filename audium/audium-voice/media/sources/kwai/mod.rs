// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Kwai (Kuaishou) video source.
//!
//! Resolves `kwai.com/video/<id>` by requesting the page with
//! `?responseType=json`, which makes Kwai return its server-rendered state
//! payload instead of HTML. The `main_mv_urls` entry holds the direct MP4 URL,
//! which becomes the track `uri` and is decoded by the `http` source. No
//! credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    config::sources::KwaiConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36";

fn video_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^https?://(?:www\.|m\.)?kwai\.com/(?:@[\w.\-]+/)?(?:video|photo|fw)/?(\d+)",
        )
        .expect("valid regex")
    })
}

pub struct KwaiSource {
    http: Arc<reqwest::Client>,
}

struct KwaiVideo {
    url: String,
    author: String,
    title: String,
    length: u64,
    thumbnail: Option<String>,
}

impl KwaiSource {
    pub fn new(_config: KwaiConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_body(&self, video_id: &str) -> Option<String> {
        let url = format!("https://www.kwai.com/video/{video_id}?responseType=json");
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "*/*")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Kwai returned HTTP {}", response.status());
            return None;
        }
        crate::common::http::read_text_capped(response).await.ok()
    }

    fn parse(body: &str, video_id: &str) -> Option<KwaiVideo> {
        let marker = format!("photo_id_str:\"{video_id}\"");
        let photo_index = body.find(&marker)?;
        let entry_start = body[..photo_index].rfind("]={")?;
        let entry_end = body[photo_index..].find("};").map(|i| i + photo_index)?;
        let block = &body[entry_start + 3..entry_end];

        let url = decode_escapes(js_string(block, "main_mv_urls", "url")?);
        if url.is_empty() {
            return None;
        }
        let author = decode_escapes(js_string(block, "", "user_name").unwrap_or_default());
        let author = if author.is_empty() {
            "Unknown".to_string()
        } else {
            author
        };
        let title = decode_escapes(js_string(block, "", "caption").unwrap_or_default());
        let title = if title.is_empty() {
            if author == "Unknown" {
                "Kwai Video".to_string()
            } else {
                format!("Kwai - {author}")
            }
        } else {
            title
        };
        // `sound:<n>` inside `ext_params` is the duration in seconds.
        let length = Regex::new(r"ext_params:\{[^}]*sound:(\d+)")
            .ok()
            .and_then(|re| re.captures(block))
            .and_then(|c| c[1].parse::<u64>().ok())
            .map(|s| s.saturating_mul(1000))
            .unwrap_or(0);
        let thumbnail = Regex::new(r#"cover_thumbnail_urls:\[\{[^}]*url:"((?:\\.|[^"])*)""#)
            .ok()
            .and_then(|re| re.captures(block))
            .map(|c| decode_escapes(c[1].to_string()));

        Some(KwaiVideo {
            url,
            author,
            title,
            length,
            thumbnail,
        })
    }

    fn build_track(&self, v: KwaiVideo, video_id: &str, original_url: &str) -> Track {
        let mut track = Track::new(TrackInfo {
            identifier: video_id.to_string(),
            is_seekable: true,
            author: v.author,
            length: v.length,
            is_stream: false,
            position: 0,
            title: v.title,
            uri: Some(v.url),
            artwork_url: v.thumbnail,
            isrc: None,
            source_name: "kwai".to_string(),
        });
        track.plugin_info = serde_json::json!({ "originalUrl": original_url });
        track
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = video_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let video_id = caps[1].to_string();
        let Some(body) = self.fetch_body(&video_id).await else {
            return LoadResult::Empty {};
        };
        match Self::parse(&body, &video_id) {
            Some(v) => LoadResult::Track(self.build_track(v, &video_id, identifier)),
            None => LoadResult::Empty {},
        }
    }
}

/// Reads `key:"value"` or, when `outer` is non-empty, the first `url:"…"`
/// inside the `outer:[{…}]` list.
fn js_string(block: &str, outer: &str, key: &str) -> Option<String> {
    let pattern = if outer.is_empty() {
        format!(r#"{k}:"((?:\\.|[^"])*)""#, k = regex::escape(key))
    } else {
        format!(
            r#"{o}:\[[^\]]*{k}:"((?:\\.|[^"])*)""#,
            o = regex::escape(outer),
            k = regex::escape(key)
        )
    };
    let re = Regex::new(&pattern).ok()?;
    re.captures(block).map(|c| c[1].to_string())
}

/// Decodes the `\uXXXX` escapes Kwai emits inside its JS string literals.
fn decode_escapes(s: String) -> String {
    let re = Regex::new(r"\\u([0-9a-fA-F]{4})").expect("valid regex");
    re.replace_all(&s, |caps: &regex::Captures| {
        u32::from_str_radix(&caps[1], 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string())
            .unwrap_or_default()
    })
    .replace("\\/", "/")
    .replace("\\\"", "\"")
}

#[async_trait]
impl SourcePlugin for KwaiSource {
    fn name(&self) -> &str {
        "kwai"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        video_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Kwai resolving: {}", identifier);
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

    fn source() -> KwaiSource {
        KwaiSource::new(KwaiConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_video_urls() {
        let s = source();
        assert!(s.can_handle("https://www.kwai.com/video/1234567890"));
        assert!(s.can_handle("https://m.kwai.com/photo/1234567890"));
        assert!(!s.can_handle("https://www.kwai.com/profile/abc"));
        assert!(!s.can_handle("https://example.com/video/123"));
    }

    #[test]
    fn parses_server_payload() {
        // Real payloads are `__APOLLO_STATE__={"k":]={…entry…};` — the parser
        // locates the entry by its `]={` opening after the matching id.
        let body = r#"window.__APOLLO_STATE__={"x":]={photo_id_str:"1234567890",user_name:"\u5f20\u4e09",caption:"My \u89c6\u9891",main_mv_urls:[{url:"https:\/\/cdn.kwai.com\/a.mp4"}],cover_thumbnail_urls:[{url:"https:\/\/cdn.kwai.com\/t.jpg"}],ext_params:{sound:42}};"#;
        let v = KwaiSource::parse(body, "1234567890").expect("video");
        assert_eq!(v.url, "https://cdn.kwai.com/a.mp4");
        assert_eq!(v.author, "张三");
        assert_eq!(v.title, "My 视频");
        assert_eq!(v.length, 42_000);
        assert_eq!(v.thumbnail.as_deref(), Some("https://cdn.kwai.com/t.jpg"));
    }

    #[test]
    fn falls_back_to_author_title() {
        let body = r#"]={photo_id_str:"9",user_name:"Bob",main_mv_urls:[{url:"https://c/x.mp4"}],ext_params:{sound:5}};"#;
        let v = KwaiSource::parse(body, "9").expect("video");
        assert_eq!(v.title, "Kwai - Bob");
        assert_eq!(v.length, 5_000);
    }

    #[test]
    fn returns_none_when_id_absent() {
        assert!(KwaiSource::parse("no such video", "123").is_none());
    }
}
