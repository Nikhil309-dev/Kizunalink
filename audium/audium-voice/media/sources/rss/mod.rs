// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! RSS / Atom podcast feed source.
//!
//! Expands a feed URL into a playlist of its playable enclosure items. The
//! enclosure URL is stored as each track's `uri`, so playback resolves through
//! the `http` source exactly like any other remote media. XML is parsed with
//! small, allocation-bounded regular expressions rather than a dependency: the
//! shapes we need (`<item>` / `<entry>` with `title`, `enclosure`, `link`,
//! `guid`) are stable across the feeds in the wild, and this keeps the crate's
//! dependency surface unchanged.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tracing::debug;

use crate::{
    config::sources::RssConfig,
    lavalink::protocol::tracks::{LoadResult, PlaylistData, PlaylistInfo, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
const DEFAULT_MAX_ITEMS: usize = 100;
/// Hard ceiling so a hostile feed cannot force an unbounded playlist.
const HARD_MAX_ITEMS: usize = 500;

fn url_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^https?://.+(\.rss|\.rrs|/feed/?$|/feeds/|/rss/?$|feed\.xml|/podcast[^?]*rss)",
        )
        .expect("valid regex")
    })
}

fn item_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<item\b[^>]*>(.*?)</item>").expect("valid regex"))
}

fn entry_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<entry\b[^>]*>(.*?)</entry>").expect("valid regex"))
}

fn enclosure_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<enclosure\b[^>]*/?>").expect("valid regex"))
}

fn atom_link_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<link\b[^>]*rel=["']enclosure["'][^>]*href=["']([^"']+)["']"#)
            .expect("valid regex")
    })
}

/// Tag/attribute regex cache. Only a handful of names are ever requested, so the
/// map stays tiny; regexes are leaked once and reused for the process lifetime.
fn tag_pattern(tag: &str) -> &'static Regex {
    cached_pattern(tag, |tag| format!(r"(?is)<{tag}\b[^>]*>(.*?)</{tag}>"))
}

fn attribute_pattern(name: &str) -> &'static Regex {
    cached_pattern(name, |name| {
        format!(r#"(?is){name}\s*=\s*["']([^"']*)["']"#)
    })
}

fn cached_pattern(key: &str, build: impl FnOnce(&str) -> String) -> &'static Regex {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static Regex>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().expect("regex cache poisoned");
    if let Some(re) = guard.get(key) {
        return re;
    }
    let leaked: &'static Regex = Box::leak(Box::new(Regex::new(&build(key)).expect("valid regex")));
    guard.insert(key.to_string(), leaked);
    leaked
}

pub struct RssSource {
    proxy: Option<crate::config::sources::HttpProxyConfig>,
    max_items: usize,
}

impl RssSource {
    pub fn new(config: RssConfig, _http: Arc<reqwest::Client>) -> Result<Self, String> {
        let max_items = if config.max_items == 0 {
            DEFAULT_MAX_ITEMS
        } else {
            config.max_items.min(HARD_MAX_ITEMS)
        };
        Ok(Self {
            proxy: config.proxy,
            max_items,
        })
    }

    async fn fetch_feed(&self, url: &str) -> Option<String> {
        let (host, _port, addresses) = super::http::resolve_and_validate_public_url(url)
            .map_err(|e| debug!("RSS feed rejected: {e}"))
            .ok()?;

        let pinned = self.proxy.is_none().then_some(addresses);
        let client = crate::engine::source::create_client_with_pinning(
            USER_AGENT.to_owned(),
            None,
            self.proxy.clone(),
            None,
            pinned.as_ref().map(|_| host),
            pinned,
        )
        .map_err(|e| debug!("RSS client build failed: {e}"))
        .ok()?;

        let response = client
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(
                reqwest::header::ACCEPT,
                "application/rss+xml, application/atom+xml, application/xml, text/xml, */*",
            )
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("RSS feed returned HTTP {}", response.status());
            return None;
        }

        let bytes = response.bytes().await.ok()?;
        const MAX_FEED_BYTES: usize = 8 * 1024 * 1024;
        if bytes.len() > MAX_FEED_BYTES {
            debug!("RSS feed exceeds {} bytes; refusing", MAX_FEED_BYTES);
            return None;
        }
        String::from_utf8(bytes.to_vec()).ok()
    }

    fn feed_title(&self, xml: &str) -> String {
        let without_items = item_pattern().replace_all(xml, "");
        let without_entries = entry_pattern().replace_all(&without_items, "");
        self.extract_tag_text(&without_entries, "title")
            .unwrap_or_else(|| "RSS Feed".to_string())
    }

    fn extract_items(&self, xml: &str) -> Vec<String> {
        let mut items: Vec<String> = item_pattern()
            .captures_iter(xml)
            .map(|c| c[1].to_string())
            .collect();
        if items.is_empty() {
            items = entry_pattern()
                .captures_iter(xml)
                .map(|c| c[1].to_string())
                .collect();
        }
        items
    }

    fn extract_tag_text(&self, xml: &str, tag: &str) -> Option<String> {
        let re = tag_pattern(tag);
        let raw = re.captures(xml)?.get(1)?.as_str().trim();
        let raw = raw
            .strip_prefix("<![CDATA[")
            .and_then(|s| s.strip_suffix("]]>"))
            .unwrap_or(raw);
        let text = unescape_xml(raw.trim());
        if text.is_empty() { None } else { Some(text) }
    }

    fn extract_enclosure(&self, item: &str) -> Option<(String, Option<String>)> {
        let tag = enclosure_pattern().find(item)?.as_str().to_string();
        let url = extract_attribute(&tag, "url")?;
        let ty = extract_attribute(&tag, "type");
        Some((unescape_xml(&url), ty))
    }

    fn extract_atom_link(&self, item: &str) -> Option<String> {
        let caps = atom_link_pattern().captures(item)?;
        caps.get(1).map(|m| unescape_xml(m.as_str()))
    }

    fn build_track(&self, item: &str, index: usize) -> Option<Track> {
        let (media_url, media_type) = match self.extract_enclosure(item) {
            Some((url, ty)) => (url, ty),
            None => (self.extract_atom_link(item)?, None),
        };

        if let Some(ty) = &media_type
            && !ty.starts_with("audio/")
            && !ty.starts_with("video/")
            && !ty.contains("mpegurl")
            && !ty.contains("octet-stream")
        {
            return None;
        }

        let title = self
            .extract_tag_text(item, "title")
            .unwrap_or_else(|| format!("Episode {}", index + 1));
        let author = self
            .extract_tag_text(item, "itunes:author")
            .or_else(|| self.extract_tag_text(item, "author"))
            .or_else(|| self.extract_tag_text(item, "dc:creator"))
            .unwrap_or_else(|| "Unknown".to_string());
        let identifier = self
            .extract_tag_text(item, "guid")
            .unwrap_or_else(|| media_url.clone());

        Some(Track::new(TrackInfo {
            identifier,
            is_seekable: true,
            author,
            length: 0,
            is_stream: false,
            position: 0,
            title,
            uri: Some(media_url),
            artwork_url: extract_attribute(item, "href").filter(|s| s.starts_with("http")),
            isrc: None,
            source_name: "rss".to_string(),
        }))
    }
}

fn extract_attribute(tag: &str, name: &str) -> Option<String> {
    attribute_pattern(name)
        .captures(tag)
        .map(|c| c[1].to_string())
}

/// Decode the five predefined XML entities plus numeric character references.
fn unescape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        let after = &rest[idx..];
        if let Some(end) = after.find(';') {
            let entity = &after[1..end];
            let replacement = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            if let Some(ch) = replacement {
                out.push(ch);
                rest = &after[end + 1..];
                continue;
            }
        }

        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

#[async_trait]
impl SourcePlugin for RssSource {
    fn name(&self) -> &str {
        "rss"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        url_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("RSS loading feed: {}", identifier);

        let Some(xml) = self.fetch_feed(identifier).await else {
            return LoadResult::Empty {};
        };

        let feed_title = self.feed_title(&xml);
        let items = self.extract_items(&xml);
        let tracks: Vec<Track> = items
            .iter()
            .take(self.max_items)
            .enumerate()
            .filter_map(|(i, item)| self.build_track(item, i))
            .collect();

        match tracks.len() {
            0 => LoadResult::Empty {},
            1 => LoadResult::Track(tracks.into_iter().next().expect("len == 1")),
            _ => LoadResult::Playlist(PlaylistData {
                info: PlaylistInfo {
                    name: feed_title,
                    selected_track: -1,
                },
                plugin_info: json!({ "type": "rss", "url": identifier }),
                tracks,
            }),
        }
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

    fn source() -> RssSource {
        RssSource::new(RssConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>My Podcast</title>
<item>
  <title>Episode One</title>
  <itunes:author>Alice</itunes:author>
  <guid>ep-1</guid>
  <enclosure url="https://cdn.example.com/ep1.mp3" type="audio/mpeg" length="1234"/>
  <itunes:image href="https://img.example.com/ep1.png"/>
</item>
<item>
  <title>Episode Two &amp; More</title>
  <enclosure url="https://cdn.example.com/ep2.mp3" type="audio/mpeg"/>
</item>
<item>
  <title>Cover art only</title>
  <enclosure url="https://cdn.example.com/cover.jpg" type="image/jpeg"/>
</item>
</channel></rss>"#;

    #[test]
    fn recognises_feed_urls() {
        let s = source();
        assert!(s.can_handle("https://example.com/podcast.rss"));
        assert!(s.can_handle("https://example.com/feed"));
        assert!(s.can_handle("https://example.com/podcast/rss"));
        assert!(s.can_handle("https://example.com/feed.xml"));

        assert!(!s.can_handle("https://example.com/song.mp3"));
        assert!(!s.can_handle("https://example.com/watch?v=abc"));
    }

    #[test]
    fn parses_items_and_skips_non_media() {
        let s = source();
        let items = s.extract_items(FEED);
        assert_eq!(items.len(), 3);

        let first = s.build_track(&items[0], 0).expect("episode one");
        assert_eq!(first.info.title, "Episode One");
        assert_eq!(first.info.author, "Alice");
        assert_eq!(first.info.identifier, "ep-1");
        assert_eq!(
            first.info.uri.as_deref(),
            Some("https://cdn.example.com/ep1.mp3")
        );
        assert_eq!(
            first.info.artwork_url.as_deref(),
            Some("https://img.example.com/ep1.png")
        );

        assert!(s.build_track(&items[2], 2).is_none());
    }

    #[test]
    fn feed_title_excludes_item_titles() {
        let s = source();
        assert_eq!(s.feed_title(FEED), "My Podcast");
    }

    #[test]
    fn unescapes_entities() {
        assert_eq!(unescape_xml("A &amp; B &lt;x&gt; &#65;"), "A & B <x> A");

        assert_eq!(unescape_xml("a &nbsp; b"), "a &nbsp; b");
    }
}
