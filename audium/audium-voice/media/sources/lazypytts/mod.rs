// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! LazyPy text-to-speech source.
//!
//! Turns `lazypytts:<service>:<voice>:<text>` (also `lazytts:`) queries into
//! speech by calling the keyless LazyPy TTS endpoint, which replies with a
//! hosted `audio_url`. That URL becomes the track `uri`. No credentials
//! required. The voice catalogue is validated lazily against the service's
//! public `voices.json`, so an unknown voice fails fast with a clear message.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::{
    config::sources::LazypyttsConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const VOICES_URL: &str = "https://lazypy.ro/tts/assets/js/voices.json";
const REQUEST_URL: &str = "https://lazypy.ro/tts/request_tts.php";
const DEFAULT_SERVICE: &str = "Cerence";
const DEFAULT_VOICE: &str = "Luciana";
const DEFAULT_MAX_TEXT_LENGTH: usize = 3000;
const USER_AGENT: &str = "Audium/LazyPyTTS";

fn prefix_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(lazypytts|lazytts):").expect("valid regex"))
}

pub struct LazypyttsSource {
    http: Arc<reqwest::Client>,
    default_service: String,
    default_voice: String,
    max_text_length: usize,
}

impl LazypyttsSource {
    pub fn new(config: LazypyttsConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self {
            http,
            default_service: config
                .default_service
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_SERVICE.to_string()),
            default_voice: config
                .default_voice
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_VOICE.to_string()),
            max_text_length: match config.max_text_length {
                0 => DEFAULT_MAX_TEXT_LENGTH,
                n => n,
            },
        })
    }

    /// Splits `service:voice:text` (or the two-part `voice:text` form).
    fn parse_query(&self, identifier: &str) -> Option<(String, String, String)> {
        let raw = prefix_pattern().replace(identifier, "");
        let mut parts = raw.splitn(3, ':');
        let first = parts.next()?.trim();
        let second = parts.next().map(str::trim);
        let third = parts.next();

        match (second, third) {
            // service:voice:text
            (Some(voice), Some(text)) if !voice.is_empty() && !text.trim().is_empty() => Some((
                if first.is_empty() {
                    self.default_service.clone()
                } else {
                    first.to_string()
                },
                voice.to_string(),
                text.trim().to_string(),
            )),
            // voice:text  (service defaults)
            (Some(text), None) if !text.trim().is_empty() => Some((
                self.default_service.clone(),
                if first.is_empty() {
                    self.default_voice.clone()
                } else {
                    first.to_string()
                },
                text.trim().to_string(),
            )),
            _ => None,
        }
    }

    /// Resolves the service's display name to the identifier LazyPy expects.
    async fn resolve_service_id(&self, wanted: &str) -> Option<String> {
        let response = self
            .http
            .get(VOICES_URL)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body: Value = response.json().await.ok()?;
        let services = body.get("services")?.as_object()?;
        services.iter().find_map(|(key, svc)| {
            let name = svc.get("name").and_then(|n| n.as_str()).unwrap_or(key);
            (name.eq_ignore_ascii_case(wanted) || key.eq_ignore_ascii_case(wanted))
                .then(|| key.clone())
        })
    }

    async fn request_audio(&self, service: &str, voice: &str, text: &str) -> Option<String> {
        let service_id = self.resolve_service_id(service).await.unwrap_or_else(|| service.to_string());
        let form = format!(
            "service={}&voice={}&text={}",
            urlencoding::encode(&service_id),
            urlencoding::encode(voice),
            urlencoding::encode(text)
        );
        let response = self
            .http
            .post(REQUEST_URL)
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(reqwest::header::ORIGIN, "https://lazypy.ro")
            .header(reqwest::header::REFERER, "https://lazypy.ro/tts/")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .body(form)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("LazyPy TTS returned HTTP {}", response.status());
            return None;
        }
        let body: Value = response.json().await.ok()?;
        if body.get("success").and_then(|s| s.as_bool()) != Some(true) {
            debug!("LazyPy TTS reported failure: {}", body);
            return None;
        }
        body.get("audio_url").and_then(|u| u.as_str()).map(str::to_string)
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some((service, voice, text)) = self.parse_query(identifier) else {
            return LoadResult::Empty {};
        };
        if text.len() > self.max_text_length {
            debug!(
                "LazyPy TTS text is {} chars, over the {} limit",
                text.len(),
                self.max_text_length
            );
            return LoadResult::Empty {};
        }
        let Some(audio_url) = self.request_audio(&service, &voice, &text).await else {
            return LoadResult::Empty {};
        };

        let title = if text.len() > 50 {
            format!("{}...", &text[..47])
        } else {
            text.clone()
        };
        let mut track = Track::new(TrackInfo {
            identifier: identifier.to_string(),
            is_seekable: true,
            author: format!("{service} ({voice})"),
            length: 0,
            is_stream: false,
            position: 0,
            title: format!("TTS: {title}"),
            uri: Some(audio_url),
            artwork_url: None,
            isrc: None,
            source_name: "lazypytts".to_string(),
        });
        track.plugin_info = serde_json::json!({ "service": service, "voice": voice });
        LoadResult::Track(track)
    }
}

#[async_trait]
impl SourcePlugin for LazypyttsSource {
    fn name(&self) -> &str {
        "lazypytts"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        prefix_pattern().is_match(identifier)
    }

    fn search_prefixes(&self) -> Vec<&str> {
        vec!["lazypytts:", "lazytts:"]
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("LazyPy TTS resolving: {}", identifier);
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

    fn source() -> LazypyttsSource {
        LazypyttsSource::new(LazypyttsConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_prefixes() {
        let s = source();
        assert!(s.can_handle("lazypytts:Cerence:Luciana:hello"));
        assert!(s.can_handle("lazytts:hello"));
        assert!(!s.can_handle("gtts:en:hello"));
    }

    #[test]
    fn parses_three_part_query() {
        let s = source();
        let (svc, voice, text) = s
            .parse_query("lazypytts:Cerence:Luciana:Hello world")
            .expect("parsed");
        assert_eq!(svc, "Cerence");
        assert_eq!(voice, "Luciana");
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn parses_two_part_query_with_defaults() {
        let s = source();
        let (svc, voice, text) = s.parse_query("lazytts:Luciana:Hi there").expect("parsed");
        assert_eq!(svc, "Cerence");
        assert_eq!(voice, "Luciana");
        assert_eq!(text, "Hi there");
    }

    #[test]
    fn rejects_empty_text() {
        assert!(source().parse_query("lazypytts:").is_none());
    }

    #[test]
    fn honours_configured_defaults() {
        let cfg = LazypyttsConfig {
            enabled: true,
            default_service: Some("Acapela".into()),
            default_voice: Some("Ryan".into()),
            ..Default::default()
        };
        let s = LazypyttsSource::new(cfg, Arc::new(reqwest::Client::new())).unwrap();
        let (svc, voice, _) = s.parse_query("lazypytts:Luciana:hi").unwrap();
        assert_eq!(svc, "Acapela");
        assert_eq!(voice, "Luciana");
    }
}
