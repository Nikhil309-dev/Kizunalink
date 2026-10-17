// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde_json::{Value, json};

use super::YouTubeCipherManager;
use crate::common::types::AnyResult;

pub const INNERTUBE_API: &str = "https://youtubei.googleapis.com";

#[derive(Debug, Clone)]
pub struct ClientConfig<'a> {
    pub client_name: &'a str,
    pub client_version: &'a str,
    pub client_id: &'a str,
    pub user_agent: &'a str,
    pub os_name: Option<&'a str>,
    pub os_version: Option<&'a str>,
    pub device_make: Option<&'a str>,
    pub device_model: Option<&'a str>,
    pub platform: Option<&'a str>,
    pub android_sdk_version: Option<&'a str>,
    pub hl: &'a str,
    pub gl: &'a str,
    pub utc_offset_minutes: Option<i32>,
    pub third_party_embed_url: Option<&'a str>,
}

impl<'a> Default for ClientConfig<'a> {
    fn default() -> Self {
        Self {
            client_name: "",
            client_version: "",
            client_id: "",
            user_agent: "",
            os_name: None,
            os_version: None,
            device_make: None,
            device_model: None,
            platform: None,
            android_sdk_version: None,
            hl: "en",
            gl: "US",
            utc_offset_minutes: None,
            third_party_embed_url: None,
        }
    }
}

impl<'a> ClientConfig<'a> {
    pub fn build_context(&self, visitor_data: Option<&str>) -> Value {
        let mut client = json!({
            "clientName": self.client_name,
            "clientVersion": self.client_version,
            "userAgent": self.user_agent,
            "hl": self.hl,
            "gl": self.gl,
        });

        if let Some(obj) = client.as_object_mut() {
            if let Some(v) = self.os_name {
                obj.insert("osName".to_string(), v.into());
            }
            if let Some(v) = self.os_version {
                obj.insert("osVersion".to_string(), v.into());
            }
            if let Some(v) = self.device_make {
                obj.insert("deviceMake".to_string(), v.into());
            }
            if let Some(v) = self.device_model {
                obj.insert("deviceModel".to_string(), v.into());
            }
            if let Some(v) = self.platform {
                obj.insert("platform".to_string(), v.into());
            }
            if let Some(v) = self.android_sdk_version {
                obj.insert("androidSdkVersion".to_string(), v.into());
            }
            if let Some(v) = self.utc_offset_minutes {
                obj.insert("utcOffsetMinutes".to_string(), v.into());
            }
            if let Some(vd) = visitor_data {
                obj.insert("visitorData".to_string(), vd.into());
            }
        }

        let mut context = json!({
            "client": client,
            "user": { "lockedSafetyMode": false },
            "request": { "useSsl": true }
        });

        if let Some(url) = self.third_party_embed_url
            && let Some(obj) = context.as_object_mut()
        {
            obj.insert("thirdParty".to_string(), json!({ "embedUrl": url }));
        }

        context
    }
}

pub const AUDIO_ITAG_PRIORITY: &[i64] = &[251, 250, 140];

pub const ITAG_FALLBACK: i64 = 18;

pub fn decode_signature_cipher(cipher_str: &str) -> Option<(String, String)> {
    let mut url = None;
    let mut sig = None;

    for part in cipher_str.split('&') {
        if let Some((k, v)) = part.split_once('=') {
            let decoded = urlencoding::decode(v).ok()?.to_string();
            match k {
                "url" => url = Some(decoded),
                "s" => sig = Some(decoded),
                _ => {}
            }
        }
    }

    match (url, sig) {
        (Some(u), Some(s)) => Some((u, s)),
        _ => None,
    }
}

pub fn select_best_audio_format<'a>(
    adaptive_formats: Option<&'a Vec<Value>>,
    formats: Option<&'a Vec<Value>>,
) -> Option<&'a Value> {
    let all: Vec<&Value> = adaptive_formats
        .into_iter()
        .flatten()
        .chain(formats.into_iter().flatten())
        .collect();

    for &target_itag in AUDIO_ITAG_PRIORITY {
        for f in &all {
            let itag = f.get("itag").and_then(|v| v.as_i64()).unwrap_or(-1);
            let mime = f.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
            if itag == target_itag && mime.starts_with("audio/") {
                return Some(f);
            }
        }
    }

    for f in &all {
        let itag = f.get("itag").and_then(|v| v.as_i64()).unwrap_or(-1);
        if itag == ITAG_FALLBACK {
            return Some(f);
        }
    }

    let mut best: Option<&Value> = None;
    let mut best_bitrate = 0i64;
    for f in all {
        let mime = f.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
        if mime.starts_with("audio/") {
            let bitrate = f.get("bitrate").and_then(|v| v.as_i64()).unwrap_or(0);
            if bitrate > best_bitrate {
                best = Some(f);
                best_bitrate = bitrate;
            }
        }
    }
    best
}

pub async fn resolve_format_url(
    format: &Value,
    player_page_url: &str,
    cipher_manager: &Arc<YouTubeCipherManager>,
) -> AnyResult<Option<String>> {
    if let Some(url) = format.get("url").and_then(|u| u.as_str()) {
        let n_param = url
            .split("&n=")
            .nth(1)
            .or_else(|| url.split("?n=").nth(1))
            .and_then(|s| s.split('&').next());

        if n_param.is_none() {
            return Ok(Some(url.to_string()));
        }

        let resolved = cipher_manager
            .resolve_url(url, player_page_url, n_param, None)
            .await?;
        return Ok(Some(resolved));
    }

    let cipher_str = format
        .get("signatureCipher")
        .or_else(|| format.get("cipher"))
        .and_then(|c| c.as_str());

    if let Some(cipher_str) = cipher_str
        && let Some((url, sig)) = decode_signature_cipher(cipher_str)
    {
        let n_param = url
            .split("&n=")
            .nth(1)
            .or_else(|| url.split("?n=").nth(1))
            .and_then(|s| s.split('&').next());
        let resolved = cipher_manager
            .resolve_url(&url, player_page_url, n_param, Some(&sig))
            .await?;
        return Ok(Some(resolved));
    }

    Ok(None)
}

static DURATION_REGEX: OnceLock<Regex> = OnceLock::new();

pub fn is_duration(text: &str) -> bool {
    let re = DURATION_REGEX
        .get_or_init(|| Regex::new(r"^\d{1,2}:\d{2}(:\d{2})?$").expect("valid regex"));
    re.is_match(text)
}

pub fn parse_duration(duration: &str) -> u64 {
    let parts: Vec<&str> = duration.split(':').collect();
    let mut ms = 0u64;
    for part in parts {
        if let Ok(num) = part.parse::<u64>() {
            ms = ms * 60 + num;
        }
    }
    ms * 1000
}

pub fn extract_thumbnail(renderer: &Value, video_id: Option<&str>) -> Option<String> {
    let thumbnails = renderer
        .get("thumbnail")
        .and_then(|t| t.get("thumbnails"))
        .or_else(|| {
            renderer
                .get("thumbnail")
                .and_then(|t| t.get("musicThumbnailRenderer"))
                .and_then(|t| t.get("thumbnail"))
                .and_then(|t| t.get("thumbnails"))
        });

    if let Some(list) = thumbnails.and_then(|t| t.as_array())
        && !list.is_empty()
    {
        let lh3 = list.iter().rev().find_map(|t| {
            t.get("url")
                .and_then(|u| u.as_str())
                .filter(|u| u.contains("lh3.googleusercontent.com"))
                .map(|u| u.split('?').next().unwrap_or(u).to_string())
        });
        if let Some(url) = lh3 {
            return Some(url);
        }

        let best = list
            .iter()
            .max_by_key(|t| t.get("width").and_then(|w| w.as_u64()).unwrap_or(0));

        if let Some(url) = best.and_then(|t| t.get("url")).and_then(|u| u.as_str()) {
            let clean = url.split('?').next().unwrap_or(url);
            if clean.contains("i.ytimg.com") {
                let upgraded = clean
                    .replace("mqdefault", "maxresdefault")
                    .replace("sddefault", "maxresdefault")
                    .replace("hqdefault", "maxresdefault");
                return Some(upgraded);
            }
            return Some(clean.to_string());
        }
    }

    if let Some(id) = video_id {
        return Some(format!("https://i.ytimg.com/vi/{}/maxresdefault.jpg", id));
    }

    None
}

pub struct PlayerRequestOptions<'a> {
    pub http: &'a reqwest::Client,
    pub config: &'a ClientConfig<'a>,
    pub video_id: &'a str,
    pub params: Option<&'a str>,
    pub visitor_data: Option<&'a str>,
    pub signature_timestamp: Option<u32>,
    pub auth_header: Option<String>,
    pub referer: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub po_token: Option<&'a str>,
    pub encrypted_host_flags: Option<String>,
    pub serialized_third_party_embed_config: bool,
}

/// How a YouTube player failure should be handled by the client fallback loop.
///
/// This is the difference between "try another client" and "stop and tell the
/// operator": retrying a request that requires new credentials or a video that
/// is simply gone wastes the request budget and hides the real cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// No client on any IP can play this: private, deleted, region-locked, or
    /// this client is structurally unsupported. Do not retry.
    Definitive,
    /// YouTube demands a signed-in / non-bot session (login prompt, bot check,
    /// age gate). Another client may have a laxer requirement, but a blind retry
    /// of the same client will not help.
    BotCheck,
    /// Rate limiting, a momentarily unavailable stream, or a network hiccup. A
    /// single backed-off retry, or another client, is reasonable.
    Transient,
    /// Could not classify from the response. Treated as transient.
    Unknown,
}

impl std::fmt::Display for FailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FailureClass::Definitive => "definitive",
            FailureClass::BotCheck => "bot-check",
            FailureClass::Transient => "transient",
            FailureClass::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// A classified YouTube failure that is carried through the `AnyResult` chain so
/// the playback loop can inspect it with `downcast_ref` without changing the
/// `YouTubeClient` trait signatures.
#[derive(Debug, Clone)]
pub struct YoutubeFailure {
    pub class: FailureClass,
    pub reason: String,
}

impl YoutubeFailure {
    pub fn new(class: FailureClass, reason: impl Into<String>) -> Self {
        Self {
            class,
            reason: reason.into(),
        }
    }
    pub fn definitive(reason: impl Into<String>) -> Self {
        Self::new(FailureClass::Definitive, reason)
    }
    pub fn bot_check(reason: impl Into<String>) -> Self {
        Self::new(FailureClass::BotCheck, reason)
    }
    pub fn transient(reason: impl Into<String>) -> Self {
        Self::new(FailureClass::Transient, reason)
    }
    pub fn is_definitive(&self) -> bool {
        self.class == FailureClass::Definitive
    }
}

impl std::fmt::Display for YoutubeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for YoutubeFailure {}

/// Classify a `playabilityStatus` (status + reason) into a [`FailureClass`].
pub fn classify_playability(status: &str, reason: &str) -> FailureClass {
    let r = reason.to_ascii_lowercase();
    let s = status.to_ascii_uppercase();

    let bot = s == "LOGIN_REQUIRED"
        || s == "AGE_CHECK_REQUIRED"
        || r.contains("not a bot")
        || r.contains("sign in")
        || r.contains("confirm your age")
        || r.contains("verify your age");
    if bot {
        return FailureClass::BotCheck;
    }

    let definitive = s == "UNPLAYABLE"
        || s == "LIVE_STREAM_OFFLINE"
        || s == "REGION_BLOCKED"
        || s == "CONTENT_CHECK_REQUIRED"
        || r.contains("private")
        || r.contains("unavailable")
        || r.contains("removed")
        || r.contains("not available in your country")
        || r.contains("not available in this app")
        || r.contains("no longer supported")
        || r.contains("no longer available")
        || (s == "ERROR" && !reason.trim().is_empty());
    if definitive {
        return FailureClass::Definitive;
    }

    FailureClass::Unknown
}

/// Classify a non-2xx HTTP status from the InnerTube endpoint.
pub fn classify_http_status(status: u16) -> FailureClass {
    match status {
        429 => FailureClass::Transient,
        500..=599 => FailureClass::Transient,
        401 | 403 => FailureClass::BotCheck,
        400 | 404 => FailureClass::Definitive,
        _ => FailureClass::Unknown,
    }
}

/// Shorten a response body for inclusion in an error message.
pub fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Validate a player body for playback and return its `streamingData`, or a
/// classified failure explaining why no stream can be produced.
///
/// Used by every client's `get_track_url`. A `Definitive` failure (private,
/// removed, region-locked, structurally unsupported) is returned as an error so
/// the playback loop stops instead of hammering the remaining clients; the
/// caller can map it back to `Ok(None)` if it deliberately wants to try a peer.
pub fn ensure_playable(body: &Value, client_name: &str) -> Result<(), YoutubeFailure> {
    let status = body
        .get("playabilityStatus")
        .and_then(|p| p.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("UNKNOWN");
    let reason = body
        .get("playabilityStatus")
        .and_then(|p| p.get("reason"))
        .and_then(|r| r.as_str())
        .unwrap_or("");

    let sd = body.get("streamingData");
    if status != "OK" && sd.is_none() {
        let class = classify_playability(status, reason);
        let detail = if reason.trim().is_empty() {
            status.to_string()
        } else {
            format!("{status}: {reason}")
        };
        tracing::warn!("{client_name} player: not playable ({detail})");
        return Err(YoutubeFailure::new(class, detail));
    }

    if sd.is_none() {
        return Err(YoutubeFailure::transient(format!(
            "player response contained no streamingData (status={status})"
        )));
    }
    Ok(())
}

/// Build the InnerTube `player` request body from already-resolved inputs.
///
/// Kept pure (no HTTP, no provider calls) so the exact payload — including
/// whether a PoToken was attached — is deterministically unit-testable.
#[allow(clippy::too_many_arguments)]
pub fn build_player_body(
    config: &ClientConfig<'_>,
    visitor_data: Option<&str>,
    video_id: &str,
    params: Option<&str>,
    signature_timestamp: Option<u32>,
    po_token: Option<&str>,
    encrypted_host_flags: Option<&str>,
    serialized_third_party_embed_config: bool,
) -> Value {
    let mut body = json!({
        "context": config.build_context(visitor_data),
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true
    });

    if serialized_third_party_embed_config && let Some(obj) = body.as_object_mut() {
        obj.insert(
            "serializedThirdPartyEmbedConfig".to_string(),
            "{\"hideInfoBar\":true,\"disableRelatedVideos\":true}".into(),
        );
    }

    if let Some(token) = po_token
        && !token.is_empty()
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert(
            "serviceIntegrityDimensions".to_string(),
            json!({ "poToken": token }),
        );
    }

    if let Some(p) = params
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("params".to_string(), p.into());
    }

    if let Some(sts) = signature_timestamp
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert(
            "playbackContext".to_string(),
            json!({
                "contentPlaybackContext": {
                    "signatureTimestamp": sts
                }
            }),
        );
    }

    if let Some(flags) = encrypted_host_flags
        && let Some(obj) = body.as_object_mut()
    {
        let playback_context = obj
            .entry("playbackContext".to_string())
            .or_insert_with(|| json!({}));
        let content_playback_context = playback_context
            .as_object_mut()
            .unwrap()
            .entry("contentPlaybackContext".to_string())
            .or_insert_with(|| json!({}));
        content_playback_context
            .as_object_mut()
            .unwrap()
            .insert("encryptedHostFlags".to_string(), flags.into());
    }

    body
}

pub async fn make_player_request(opts: PlayerRequestOptions<'_>) -> AnyResult<Value> {
    let po_token: Option<String> = match opts.po_token {
        Some(t) if !t.is_empty() => Some(t.to_string()),
        _ => match crate::media::sources::youtube::pot::fetch_global_typed(opts.video_id).await {
            Ok(token) => token,
            Err(e) => {
                tracing::warn!(
                    "PoToken unavailable for video {} ({}); issuing untokened player request — \
                     expect HTTP 403 if this client requires a token",
                    opts.video_id,
                    e
                );
                None
            }
        },
    };

    let body = build_player_body(
        opts.config,
        opts.visitor_data,
        opts.video_id,
        opts.params,
        opts.signature_timestamp,
        po_token.as_deref(),
        opts.encrypted_host_flags.as_deref(),
        opts.serialized_third_party_embed_config,
    );

    let url = format!("{}/youtubei/v1/player?prettyPrint=false", INNERTUBE_API);

    let mut req = opts
        .http
        .post(&url)
        .header("User-Agent", opts.config.user_agent)
        .header("X-YouTube-Client-Name", opts.config.client_id)
        .header("X-YouTube-Client-Version", opts.config.client_version)
        .header("X-Goog-Api-Format-Version", "2");

    if let Some(vd) = opts.visitor_data {
        req = req.header("X-Goog-Visitor-Id", vd);
    }

    if let Some(auth) = opts.auth_header {
        req = req.header("Authorization", auth);
    }

    if let Some(ref_url) = opts.referer {
        req = req.header("Referer", ref_url);
    }

    if let Some(orig_url) = opts.origin {
        req = req.header("Origin", orig_url);
    }

    let res = req.json(&body).send().await?;
    let status = res.status();
    if !status.is_success() {
        let code = status.as_u16();
        let text = res
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(Box::new(YoutubeFailure::new(
            classify_http_status(code),
            format!(
                "player request failed (status={code}): {}",
                truncate(&text, 300)
            ),
        )));
    }

    Ok(crate::common::http::read_json_capped::<serde_json::Value>(res).await?)
}

pub async fn make_next_request(
    http: &reqwest::Client,
    config: &ClientConfig<'_>,
    video_id: Option<&str>,
    playlist_id: Option<&str>,
    visitor_data: Option<&str>,
    auth_header: Option<String>,
) -> AnyResult<Value> {
    let mut body = json!({
        "context": config.build_context(visitor_data),
    });

    if let Some(vid) = video_id
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("videoId".to_string(), vid.into());
    }

    if let Some(pid) = playlist_id
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("playlistId".to_string(), pid.into());
    }

    let url = format!("{}/youtubei/v1/next?prettyPrint=false", INNERTUBE_API);

    let mut req = http
        .post(&url)
        .header("User-Agent", config.user_agent)
        .header("X-YouTube-Client-Name", config.client_id)
        .header("X-YouTube-Client-Version", config.client_version);

    if let Some(vd) = visitor_data {
        req = req.header("X-Goog-Visitor-Id", vd);
    }

    if let Some(auth) = auth_header {
        req = req.header("Authorization", auth);
    }

    let res = req.json(&body).send().await?;
    let status = res.status();
    if !status.is_success() {
        let text = res
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Next request failed (status={}): {}", status, text).into());
    }

    Ok(crate::common::http::read_json_capped::<serde_json::Value>(res).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ClientConfig<'static> {
        ClientConfig {
            client_name: "WEB",
            client_version: "2.20260114.01.00",
            client_id: "1",
            user_agent: "test-agent",
            ..Default::default()
        }
    }

    #[test]
    fn player_body_includes_configured_po_token() {
        let cfg = test_config();
        let body = build_player_body(
            &cfg,
            Some("visitor-abc"),
            "dQw4w9WgXcQ",
            None,
            None,
            Some("po-token-xyz"),
            None,
            false,
        );
        assert_eq!(
            body.get("serviceIntegrityDimensions")
                .and_then(|s| s.get("poToken"))
                .and_then(|v| v.as_str()),
            Some("po-token-xyz")
        );

        assert_eq!(
            body.get("context")
                .and_then(|c| c.get("client"))
                .and_then(|c| c.get("visitorData"))
                .and_then(|v| v.as_str()),
            Some("visitor-abc")
        );
        assert_eq!(
            body.get("videoId").and_then(|v| v.as_str()),
            Some("dQw4w9WgXcQ")
        );
    }

    #[test]
    fn player_body_omits_service_integrity_without_token() {
        let cfg = test_config();
        let body = build_player_body(&cfg, None, "vid", None, None, None, None, false);
        assert!(body.get("serviceIntegrityDimensions").is_none());
    }

    #[test]
    fn empty_token_is_not_attached() {
        let cfg = test_config();
        let body = build_player_body(&cfg, None, "vid", None, None, Some(""), None, false);
        assert!(body.get("serviceIntegrityDimensions").is_none());
    }

    #[test]
    fn token_and_playback_context_coexist() {
        let cfg = test_config();
        let body = build_player_body(
            &cfg,
            None,
            "vid",
            None,
            Some(20000),
            Some("tok"),
            Some("flags-blob"),
            true,
        );
        assert_eq!(
            body.get("serviceIntegrityDimensions")
                .and_then(|s| s.get("poToken"))
                .and_then(|v| v.as_str()),
            Some("tok")
        );
        let cpc = body
            .get("playbackContext")
            .and_then(|p| p.get("contentPlaybackContext"))
            .unwrap();
        assert_eq!(
            cpc.get("signatureTimestamp").and_then(|v| v.as_u64()),
            Some(20000)
        );
        assert_eq!(
            cpc.get("encryptedHostFlags").and_then(|v| v.as_str()),
            Some("flags-blob")
        );
        assert!(body.get("serializedThirdPartyEmbedConfig").is_some());
    }

    #[test]
    fn classify_login_required_is_bot_check() {
        assert_eq!(
            classify_playability("LOGIN_REQUIRED", "Sign in to confirm you're not a bot"),
            FailureClass::BotCheck
        );
        assert_eq!(
            classify_playability("LOGIN_REQUIRED", "This video may be inappropriate"),
            FailureClass::BotCheck
        );
        assert_eq!(
            classify_playability("AGE_CHECK_REQUIRED", ""),
            FailureClass::BotCheck
        );
    }

    #[test]
    fn classify_unplayable_and_private_are_definitive() {
        assert_eq!(
            classify_playability("UNPLAYABLE", "This video is unavailable"),
            FailureClass::Definitive
        );
        assert_eq!(
            classify_playability("ERROR", "This video is private"),
            FailureClass::Definitive
        );
        assert_eq!(
            classify_playability("UNPLAYABLE", "Playback on other websites has been disabled"),
            FailureClass::Definitive
        );
    }

    #[test]
    fn classify_http_statuses() {
        assert_eq!(classify_http_status(429), FailureClass::Transient);
        assert_eq!(classify_http_status(503), FailureClass::Transient);
        assert_eq!(classify_http_status(403), FailureClass::BotCheck);
        assert_eq!(classify_http_status(401), FailureClass::BotCheck);
        assert_eq!(classify_http_status(404), FailureClass::Definitive);
    }

    #[test]
    fn ensure_playable_reports_definitive_for_private_video() {
        let body = json!({
            "playabilityStatus": { "status": "ERROR", "reason": "This video is private" }
        });
        let err = ensure_playable(&body, "Web").unwrap_err();
        assert_eq!(err.class, FailureClass::Definitive);
        assert!(err.reason.contains("private"));
    }

    #[test]
    fn ensure_playable_passes_when_streaming_data_present() {
        let body = json!({
            "playabilityStatus": { "status": "OK" },
            "streamingData": { "adaptiveFormats": [] }
        });
        assert!(ensure_playable(&body, "Web").is_ok());
    }

    #[test]
    fn ensure_playable_flags_missing_streaming_data_as_transient() {
        let body = json!({ "playabilityStatus": { "status": "OK" } });
        let err = ensure_playable(&body, "Web").unwrap_err();
        assert_eq!(err.class, FailureClass::Transient);
    }
}
