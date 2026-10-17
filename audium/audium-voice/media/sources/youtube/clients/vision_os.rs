// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
    YouTubeClient,
    common::{INNERTUBE_API, resolve_format_url, select_best_audio_format},
};
use crate::{
    common::types::AnyResult,
    lavalink::protocol::tracks::Track,
    media::sources::youtube::{
        cipher::YouTubeCipherManager, clients::common::ClientConfig, extractor::extract_track,
        oauth::YouTubeOAuth,
    },
};

const CLIENT_NAME: &str = "VISIONOS";
const CLIENT_ID: &str = "101";
const CLIENT_VERSION: &str = "1.08";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
/// VISIONOS is an internal YouTube client that does not require a PoToken or a
/// player script, which makes it the most reliable unauthenticated playback
/// path when datacenter IPs are bot-checked on WEB/ANDROID.
const PLAYER_PARAMS: &str = "2AMB";

pub struct VisionOsClient {
    http: Arc<reqwest::Client>,
}

impl VisionOsClient {
    pub fn new(http: Arc<reqwest::Client>) -> Self {
        Self { http }
    }

    fn config(&self) -> ClientConfig<'_> {
        ClientConfig {
            client_name: CLIENT_NAME,
            client_version: CLIENT_VERSION,
            client_id: CLIENT_ID,
            user_agent: USER_AGENT,
            device_make: Some("Apple"),
            device_model: Some("RealityDevice17,1"),
            os_name: Some("visionOS"),
            os_version: Some("26.5.23O471"),
            ..Default::default()
        }
    }

    async fn player_request(
        &self,
        video_id: &str,
        visitor_data: Option<&str>,
        _oauth: &Arc<YouTubeOAuth>,
    ) -> AnyResult<Value> {
        crate::media::sources::youtube::clients::common::make_player_request(
            crate::media::sources::youtube::clients::common::PlayerRequestOptions {
                http: &self.http,
                config: &self.config(),
                video_id,
                params: Some(PLAYER_PARAMS),
                visitor_data,
                signature_timestamp: None,
                auth_header: None,
                referer: None,
                origin: None,
                po_token: None,
                encrypted_host_flags: None,
                serialized_third_party_embed_config: false,
            },
        )
        .await
    }
}

#[async_trait]
impl YouTubeClient for VisionOsClient {
    fn name(&self) -> &str {
        "VisionOs"
    }
    fn client_name(&self) -> &str {
        CLIENT_NAME
    }
    fn client_version(&self) -> &str {
        CLIENT_VERSION
    }
    fn user_agent(&self) -> &str {
        USER_AGENT
    }

    async fn search(
        &self,
        query: &str,
        context: &Value,
        _oauth: Arc<YouTubeOAuth>,
    ) -> AnyResult<Vec<Track>> {
        let visitor_data = context
            .get("client")
            .and_then(|c| c.get("visitorData"))
            .and_then(|v| v.as_str())
            .or_else(|| context.get("visitorData").and_then(|v| v.as_str()));

        let body = json!({
            "context": self.config().build_context(visitor_data),
            "query": query,
            "params": "EgIQAQ%3D%3D"
        });

        let url = format!("{}/youtubei/v1/search?prettyPrint=false", INNERTUBE_API);

        let mut req = self
            .http
            .post(&url)
            .header("User-Agent", USER_AGENT)
            .header("X-YouTube-Client-Name", CLIENT_ID)
            .header("X-YouTube-Client-Version", CLIENT_VERSION)
            .header("X-Goog-Api-Format-Version", "2");

        if let Some(vd) = visitor_data {
            req = req.header("X-Goog-Visitor-Id", vd);
        }

        let res = req.json(&body).send().await?;
        if !res.status().is_success() {
            return Err(format!("VISIONOS search failed: {}", res.status()).into());
        }

        let response: Value =
            crate::common::http::read_json_capped::<serde_json::Value>(res).await?;
        let mut tracks = Vec::new();

        if let Some(sections) = response
            .get("contents")
            .and_then(|c| c.get("sectionListRenderer"))
            .and_then(|s| s.get("contents"))
            .and_then(|c| c.as_array())
        {
            for section in sections {
                if let Some(items) = section
                    .get("itemSectionRenderer")
                    .and_then(|i| i.get("contents"))
                    .and_then(|c| c.as_array())
                {
                    for item in items {
                        if let Some(track) = extract_track(item, "youtube") {
                            tracks.push(track);
                        }
                    }
                }
            }
        }

        Ok(tracks)
    }

    async fn get_track_info(
        &self,
        _track_id: &str,
        _context: &Value,
        _oauth: Arc<YouTubeOAuth>,
    ) -> AnyResult<Option<Track>> {
        tracing::debug!("{} client does not support get_track_info", self.name());
        Ok(None)
    }

    async fn get_playlist(
        &self,
        _playlist_id: &str,
        _context: &Value,
        _oauth: Arc<YouTubeOAuth>,
    ) -> AnyResult<Option<(Vec<Track>, String)>> {
        tracing::debug!("{} client does not support get_playlist", self.name());
        Ok(None)
    }

    async fn resolve_url(
        &self,
        _url: &str,
        _context: &Value,
        _oauth: Arc<YouTubeOAuth>,
    ) -> AnyResult<Option<Track>> {
        tracing::debug!("{} client does not support resolve_url", self.name());
        Ok(None)
    }

    async fn get_track_url(
        &self,
        track_id: &str,
        context: &Value,
        cipher_manager: Arc<YouTubeCipherManager>,
        oauth: Arc<YouTubeOAuth>,
    ) -> AnyResult<Option<String>> {
        let visitor_data = context
            .get("client")
            .and_then(|c| c.get("visitorData"))
            .and_then(|v| v.as_str())
            .or_else(|| context.get("visitorData").and_then(|v| v.as_str()));

        let body = self.player_request(track_id, visitor_data, &oauth).await?;

        crate::media::sources::youtube::clients::common::ensure_playable(&body, "VISIONOS")?;

        let streaming_data = match body.get("streamingData") {
            Some(sd) => sd,
            None => {
                tracing::error!("VISIONOS player: no streamingData for {}", track_id);
                return Ok(None);
            }
        };

        if let Some(hls) = streaming_data
            .get("hlsManifestUrl")
            .and_then(|v| v.as_str())
        {
            return Ok(Some(hls.to_string()));
        }

        let adaptive = streaming_data
            .get("adaptiveFormats")
            .and_then(|v| v.as_array());
        let formats = streaming_data.get("formats").and_then(|v| v.as_array());
        let player_page_url = format!("https://www.youtube.com/watch?v={}", track_id);

        let selected = select_best_audio_format(adaptive, formats);
        let Some(best) = selected else {
            tracing::warn!("VISIONOS: no suitable audio format found for {}", track_id);
            return Ok(None);
        };

        match resolve_format_url(best, &player_page_url, &cipher_manager).await {
            Ok(Some(url)) => Ok(Some(url)),
            Ok(None) => {
                tracing::warn!(
                    "VISIONOS: resolve_format_url returned None for {}",
                    track_id
                );
                Ok(None)
            }
            Err(e) => {
                tracing::warn!("VISIONOS: resolve_format_url error for {}: {}", track_id, e);
                Ok(None)
            }
        }
    }

    async fn get_player_body(
        &self,
        track_id: &str,
        visitor_data: Option<&str>,
        oauth: Arc<YouTubeOAuth>,
    ) -> Option<serde_json::Value> {
        self.player_request(track_id, visitor_data, &oauth)
            .await
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VISIONOS must keep its device identity consistent: YouTube rejects the
    /// client when the reported OS/platform does not match the user agent.
    #[test]
    fn vision_os_reports_apple_device_identity() {
        let client = VisionOsClient::new(Arc::new(reqwest::Client::new()));
        let cfg = client.config();
        assert_eq!(cfg.client_name, "VISIONOS");
        assert_eq!(cfg.device_make, Some("Apple"));
        assert_eq!(cfg.os_name, Some("visionOS"));
        assert!(cfg.device_model.is_some(), "device model must be set");
    }

    #[test]
    fn vision_os_uses_stable_client_version() {
        assert_eq!(CLIENT_VERSION, "1.08");
        assert!(
            USER_AGENT.contains("Safari"),
            "VISIONOS user agent must look like a Safari/visionOS browser"
        );
    }
}
