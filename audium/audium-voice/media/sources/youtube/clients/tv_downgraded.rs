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

const CLIENT_NAME: &str = "TVHTML5";
const CLIENT_ID: &str = "85";
const CLIENT_VERSION: &str = "5.20260713";
#[cfg(test)]
const BROWSER_NAME: &str = "Cobalt";
#[cfg(test)]
const BROWSER_VERSION: &str = "5.150.00-qa";
const USER_AGENT: &str = "Mozilla/5.0 (Unknown) Cobalt/5.150.00-qa (unlike Gecko) Starboard/3, Unknown;Unknown;Unknown;0/Unknown (Chromium, Starboard, Wired)";
/// The "downgraded" TV client pins the older, non-SABR player parameters so the
/// response still carries direct format URLs instead of a SABR-only stream.
const PLAYER_PARAMS: &str = "2AMB";

pub struct TvDowngradedClient {
    http: Arc<reqwest::Client>,
}

impl TvDowngradedClient {
    pub fn new(http: Arc<reqwest::Client>) -> Self {
        Self { http }
    }

    fn config(&self) -> ClientConfig<'_> {
        ClientConfig {
            client_name: CLIENT_NAME,
            client_version: CLIENT_VERSION,
            client_id: CLIENT_ID,
            user_agent: USER_AGENT,
            ..Default::default()
        }
    }

    async fn player_request(
        &self,
        video_id: &str,
        visitor_data: Option<&str>,
        signature_timestamp: Option<u32>,
        _oauth: &Arc<YouTubeOAuth>,
    ) -> AnyResult<Value> {
        crate::media::sources::youtube::clients::common::make_player_request(
            crate::media::sources::youtube::clients::common::PlayerRequestOptions {
                http: &self.http,
                config: &self.config(),
                video_id,
                params: Some(PLAYER_PARAMS),
                visitor_data,
                signature_timestamp,
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
impl YouTubeClient for TvDowngradedClient {
    fn name(&self) -> &str {
        "TV_DOWN"
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
            return Err(format!("TVHTML5 search failed: {}", res.status()).into());
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

        let body = self
            .player_request(track_id, visitor_data, None, &oauth)
            .await?;

        crate::media::sources::youtube::clients::common::ensure_playable(&body, "TVHTML5")?;

        let streaming_data = match body.get("streamingData") {
            Some(sd) => sd,
            None => {
                tracing::error!("TVHTML5 player: no streamingData for {}", track_id);
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
            tracing::warn!("TVHTML5: no suitable audio format found for {}", track_id);
            return Ok(None);
        };

        match resolve_format_url(best, &player_page_url, &cipher_manager).await {
            Ok(Some(url)) => Ok(Some(url)),
            Ok(None) => {
                tracing::warn!("TVHTML5: resolve_format_url returned None for {}", track_id);
                Ok(None)
            }
            Err(e) => {
                tracing::warn!("TVHTML5: resolve_format_url error for {}: {}", track_id, e);
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
        self.player_request(track_id, visitor_data, None, &oauth)
            .await
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of this client is the older Cobalt identity; the user
    /// agent and browser fields must agree or YouTube downgrades the response.
    #[test]
    fn tv_downgraded_reports_cobalt_identity() {
        let client = TvDowngradedClient::new(Arc::new(reqwest::Client::new()));
        let cfg = client.config();
        assert_eq!(cfg.client_name, "TVHTML5");
        assert!(USER_AGENT.contains(BROWSER_NAME));
        assert!(USER_AGENT.contains(BROWSER_VERSION));
    }

    #[test]
    fn tv_downgraded_uses_distinct_client_id_from_tv_cast() {
        assert_eq!(CLIENT_ID, "85");
    }
}
