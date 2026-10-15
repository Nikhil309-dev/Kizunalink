// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Telegram public-message source.
//!
//! Resolves `t.me/<channel>/<id>` (also `telegram.me`, `telegram.dog`) public
//! posts through Telegram's keyless embed widget (`?embed=1`). Every video or
//! audio attachment in the message becomes a track; a message with several
//! attachments yields a playlist. No account or bot token required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    config::sources::TelegramConfig,
    lavalink::protocol::tracks::{
        LoadResult, PlaylistData, PlaylistInfo, Track, TrackInfo,
    },
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn message_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(?:t\.me|telegram\.me|telegram\.dog)/([^/\s]+)/(\d+)")
            .expect("valid regex")
    })
}

pub struct TelegramSource {
    http: Arc<reqwest::Client>,
}

impl TelegramSource {
    pub fn new(_config: TelegramConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_embed(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Telegram embed returned HTTP {}", response.status());
            return None;
        }
        response.text().await.ok()
    }

    /// Collects `<video src>` / `<audio src>` attachments in message order.
    fn extract_media(html: &str) -> Vec<String> {
        let re = Regex::new(r#"<(?:video|audio)[^>]+src="([^"]+)""#).expect("valid regex");
        let mut seen = Vec::new();
        for caps in re.captures_iter(html) {
            let url = caps[1].to_string();
            if !seen.contains(&url) {
                seen.push(url);
            }
        }
        seen
    }

    fn extract_author(html: &str) -> String {
        Regex::new(r#"class="tgme_widget_message_author[^>]*>[\s\S]*?<span dir="auto">([^<]+)</span>"#)
            .ok()
            .and_then(|re| re.captures(html))
            .map(|c| c[1].trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Telegram Channel".to_string())
    }

    fn extract_description(html: &str) -> String {
        Regex::new(r#"class="tgme_widget_message_text[^>]*>([\s\S]*?)</div>"#)
            .ok()
            .and_then(|re| re.captures(html))
            .map(|c| {
                c[1].replace("<br>", "\n")
                    .replace("<br/>", "\n")
                    .replace("<br />", "\n")
            })
            .map(|t| Regex::new(r"<[^>]*>").expect("valid").replace_all(&t, "").trim().to_string())
            .unwrap_or_default()
    }

    /// Parses `[h:]m:s` durations into milliseconds.
    fn parse_duration_ms(text: &str) -> u64 {
        let parts: Vec<u64> = text
            .split(':')
            .filter_map(|p| p.trim().parse::<u64>().ok())
            .collect();
        match parts.as_slice() {
            [h, m, s] => (h * 3600 + m * 60 + s) * 1000,
            [m, s] => (m * 60 + s) * 1000,
            _ => 0,
        }
    }

    /// Reads the duration that follows each media block, if present.
    fn durations(html: &str) -> Vec<u64> {
        let re = Regex::new(r#"class="tgme_widget_message_video_duration">([\d:]+)</time>"#)
            .expect("valid regex");
        re.captures_iter(html)
            .map(|c| Self::parse_duration_ms(&c[1]))
            .collect()
    }

    fn build_tracks(html: &str, channel: &str, message_id: &str, url: &str) -> Vec<Track> {
        let media = Self::extract_media(html);
        let author = Self::extract_author(html);
        let description = Self::extract_description(html);
        let first_line = description.lines().next().unwrap_or("").trim().to_string();
        let durations = Self::durations(html);

        media
            .into_iter()
            .enumerate()
            .map(|(i, media_url)| {
                let is_video = media_url.contains(".mp4") || media_url.contains("/video");
                let title = if first_line.is_empty() {
                    format!("Telegram {channel} #{message_id}")
                } else if i == 0 {
                    first_line.clone()
                } else {
                    format!("{first_line} ({})", i + 1)
                };
                let mut track = Track::new(TrackInfo {
                    identifier: format!("{channel}/{message_id}/{i}"),
                    is_seekable: true,
                    author: author.clone(),
                    length: durations.get(i).copied().unwrap_or(0),
                    is_stream: !is_video,
                    position: 0,
                    title,
                    uri: Some(media_url),
                    artwork_url: None,
                    isrc: None,
                    source_name: "telegram".to_string(),
                });
                track.plugin_info = serde_json::json!({ "originalUrl": url });
                track
            })
            .collect()
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = message_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let channel = caps[1].to_string();
        let message_id = caps[2].to_string();

        let embed_url = format!("{identifier}?embed=1");
        let Some(html) = self.fetch_embed(&embed_url).await else {
            return LoadResult::Empty {};
        };

        let tracks = Self::build_tracks(&html, &channel, &message_id, identifier);
        match tracks.len() {
            0 => LoadResult::Empty {},
            1 => LoadResult::Track(tracks.into_iter().next().expect("one track")),
            _ => LoadResult::Playlist(PlaylistData {
                info: PlaylistInfo {
                    name: format!("Telegram {channel} #{message_id}"),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({ "originalUrl": identifier }),
                tracks,
            }),
        }
    }
}

#[async_trait]
impl SourcePlugin for TelegramSource {
    fn name(&self) -> &str {
        "telegram"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        message_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Telegram resolving: {}", identifier);
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

    #[test]
    fn recognises_message_urls() {
        assert!(message_pattern().is_match("https://t.me/mychannel/123"));
        assert!(message_pattern().is_match("https://telegram.me/c/45"));
        assert!(message_pattern().is_match("https://telegram.dog/x/9"));
        assert!(!message_pattern().is_match("https://t.me/mychannel"));
        assert!(!message_pattern().is_match("https://example.com/x/1"));
    }

    #[test]
    fn extracts_multiple_attachments() {
        let html = r#"
            <div class="tgme_widget_message_author"><span dir="auto">Chan</span></div>
            <div class="tgme_widget_message_text">Line one<br/>Line two</div>
            <a class="tgme_widget_message_video_player"><video src="https://cdn/a.mp4"></video>
            <time class="tgme_widget_message_video_duration">1:05</time></a>
            <video src="https://cdn/b.mp4"></video>"#;
        let tracks = TelegramSource::build_tracks(html, "chan", "7", "https://t.me/chan/7");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].info.title, "Line one");
        assert_eq!(tracks[0].info.length, 65_000);
        assert_eq!(tracks[1].info.title, "Line one (2)");
        assert_eq!(tracks[0].info.author, "Chan");
        assert_eq!(tracks[0].info.source_name, "telegram");
    }

    #[test]
    fn dedupes_repeated_media() {
        let html = r#"<video src="https://cdn/a.mp4"></video><video src="https://cdn/a.mp4"></video>"#;
        assert_eq!(TelegramSource::extract_media(html).len(), 1);
    }

    #[test]
    fn parses_duration_formats() {
        assert_eq!(TelegramSource::parse_duration_ms("1:05"), 65_000);
        assert_eq!(TelegramSource::parse_duration_ms("1:02:03"), 3_723_000);
        assert_eq!(TelegramSource::parse_duration_ms("bad"), 0);
    }

    #[test]
    fn no_media_yields_no_tracks() {
        assert!(TelegramSource::build_tracks("<html></html>", "c", "1", "u").is_empty());
    }
}
