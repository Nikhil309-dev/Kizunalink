// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! OC ReMix (ocremix.org) source.
//!
//! Resolves `ocremix.org/remix/<id>` links and bare `OCR<digits>` identifiers by
//! reading the public `?view=xml` metadata document, then streaming the reported
//! `file_name` from a public OC ReMix mirror host. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    common::http::read_text_capped,
    config::sources::OcremixConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Public OC ReMix mirrors that serve the same `/files/music/...` tree.
/// The first host is canonical; the rest are fallbacks.
const MIRROR_HOSTS: &[&str] = &["iterations.org", "ocrmirror.org", "ocr.blueblue.fr"];

fn remix_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:https?://(?:www\.)?ocremix\.org/remix/)?(?<id>OCR\d+)")
            .expect("valid regex")
    })
}

pub struct OcremixSource {
    http: Arc<reqwest::Client>,
}

/// Fields parsed out of the `<remix …>` element.
struct RemixMeta {
    id: String,
    name: String,
    file_name: String,
    track_length_ms: u64,
    remixers: String,
}

impl OcremixSource {
    pub fn new(_config: OcremixConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_xml(&self, id: &str) -> Option<String> {
        let url = format!("https://ocremix.org/remix/{id}?view=xml");
        let response = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/xml,text/xml")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("OCRemix returned HTTP {}", response.status());
            return None;
        }
        read_text_capped(response).await.ok()
    }

    /// Parses the `<remix>` element and the `<remixers>` list from the XML.
    fn parse_xml(xml: &str) -> Option<RemixMeta> {
        let remix_re = Regex::new(r"<remix\b([^>]*)>").ok()?;
        let attrs = remix_re.captures(xml)?.get(1)?.as_str();

        let id = attr(attrs, "id")?;
        let name = attr(attrs, "name").unwrap_or_else(|| id.clone());
        let file_name = attr(attrs, "file_name")?;
        let seconds: f64 = attr(attrs, "track_length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let remixers = parse_remixers(xml).unwrap_or_default();

        Some(RemixMeta {
            id,
            name,
            file_name,
            track_length_ms: (seconds * 1000.0).round() as u64,
            remixers,
        })
    }

    fn build_track(meta: RemixMeta, original_url: &str) -> Track {
        let stream = format!("https://{}{}", MIRROR_HOSTS[0], meta.file_name);
        let mut track = Track::new(TrackInfo {
            identifier: meta.id.clone(),
            is_seekable: true,
            author: if meta.remixers.is_empty() {
                "OC ReMix".to_string()
            } else {
                meta.remixers
            },
            length: meta.track_length_ms,
            is_stream: false,
            position: 0,
            title: meta.name,
            uri: Some(stream),
            artwork_url: Some("https://ocremix.org/favicon.ico".to_string()),
            isrc: None,
            source_name: "ocremix".to_string(),
        });
        track.plugin_info = serde_json::json!({
            "originalUrl": original_url,
            "streamHosts": MIRROR_HOSTS,
        });
        track
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = remix_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let id = &caps["id"];
        let Some(xml) = self.fetch_xml(id).await else {
            return LoadResult::Empty {};
        };
        match Self::parse_xml(&xml) {
            Some(meta) => LoadResult::Track(Self::build_track(meta, identifier)),
            None => LoadResult::Empty {},
        }
    }
}

/// Reads one attribute value out of an element's attribute string.
fn attr(attrs: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"{}\s*=\s*"([^"]*)""#, regex::escape(key))).ok()?;
    Some(unescape(re.captures(attrs)?.get(1)?.as_str()))
}

/// Collects the `name` attributes of every `<remixer …/>` child, joined by ", ".
fn parse_remixers(xml: &str) -> Option<String> {
    let block_re = Regex::new(r"(?s)<remixers>(.*?)</remixers>").ok()?;
    let block = block_re.captures(xml)?.get(1)?.as_str();
    let remixer_re = Regex::new(r#"<remixer\b[^>]*\bname\s*=\s*"([^"]*)""#).ok()?;
    let names: Vec<String> = remixer_re
        .captures_iter(block)
        .filter_map(|c| c.get(1).map(|m| unescape(m.as_str())))
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

/// Decodes the handful of XML entities that appear in OC ReMix metadata.
fn unescape(raw: &str) -> String {
    raw.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[async_trait]
impl SourcePlugin for OcremixSource {
    fn name(&self) -> &str {
        "ocremix"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        remix_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("OCRemix resolving: {}", identifier);
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

    fn source() -> OcremixSource {
        OcremixSource::new(OcremixConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_urls_and_bare_ids() {
        let s = source();
        assert!(s.can_handle("https://ocremix.org/remix/OCR04499"));
        assert!(s.can_handle("http://www.ocremix.org/remix/OCR01234"));
        assert!(s.can_handle("OCR04499"));
        assert!(s.can_handle("ocr04499"));
        assert!(!s.can_handle("https://ocremix.org/game/123/x"));
        assert!(!s.can_handle("https://example.com/remix/OCR1"));
    }

    #[test]
    fn parses_remix_element_and_remixers() {
        // Trimmed real payload from ocremix.org/remix/OCR04499?view=xml.
        let xml = r#"<?xml version="1.0"?>
<view><metadata id="708"/>
<remix id="OCR04499" name="Blue Magic" postdate="2023-11-02"
  file_name="/files/music/remixes/Minecraft_Blue_Magic_OC_ReMix.mp3"
  file_size="4299257" track_length="152" author_artist_name="The Orichalcon">
  <writeup><p>text &amp; more</p></writeup>
</remix>
<remixers><remixer name="NoTuX" id="4656" urlname="notux"/></remixers>
</view>"#;
        let meta = OcremixSource::parse_xml(xml).expect("meta");
        assert_eq!(meta.id, "OCR04499");
        assert_eq!(meta.name, "Blue Magic");
        assert_eq!(
            meta.file_name,
            "/files/music/remixes/Minecraft_Blue_Magic_OC_ReMix.mp3"
        );
        assert_eq!(meta.track_length_ms, 152_000);
        assert_eq!(meta.remixers, "NoTuX");
    }

    #[test]
    fn joins_multiple_remixers_and_unescapes() {
        let xml = r#"<view>
<remix id="OCR1" name="Rock &amp; Roll" file_name="/files/music/remixes/x.mp3" track_length="10"/>
<remixers><remixer name="Alice"/><remixer name="Bob"/></remixers>
</view>"#;
        let meta = OcremixSource::parse_xml(xml).expect("meta");
        assert_eq!(meta.name, "Rock & Roll");
        assert_eq!(meta.remixers, "Alice, Bob");
        assert_eq!(meta.track_length_ms, 10_000);
    }

    #[test]
    fn builds_track_on_primary_mirror() {
        let xml = r#"<view><remix id="OCR04499" name="Blue Magic"
            file_name="/files/music/remixes/x.mp3" track_length="152"/></view>"#;
        let meta = OcremixSource::parse_xml(xml).unwrap();
        let track = OcremixSource::build_track(meta, "https://ocremix.org/remix/OCR04499");
        assert_eq!(track.info.source_name, "ocremix");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://iterations.org/files/music/remixes/x.mp3")
        );
        assert_eq!(track.info.length, 152_000);
    }

    #[test]
    fn missing_remix_element_is_none() {
        assert!(OcremixSource::parse_xml("<view><metadata/></view>").is_none());
    }
}
