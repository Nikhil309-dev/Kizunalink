// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;

use crate::api::rest::json::{self, ApiPath, ApiQuery};
use crate::{
    server::AppState,
    sources::youtube::clients::common::{resolve_format_url, select_best_audio_format},
};

pub async fn get_youtube_info(
    OriginalUri(uri): OriginalUri,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = json::path_of(&uri);
    tracing::info!("GET /youtube");

    let ctx = match &state.youtube {
        Some(ctx) => ctx.clone(),
        None => {
            tracing::warn!("GET /youtube: YouTube source is not enabled");
            return json::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "YouTube source is not enabled",
                &path,
            );
        }
    };

    let count = ctx.oauth.get_refresh_tokens().await.len();
    tracing::debug!("GET /youtube: {} refresh token(s) configured", count);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "refreshTokens": count })),
    )
        .into_response()
}

pub async fn youtube_oauth_refresh(
    OriginalUri(uri): OriginalUri,
    ApiPath(refresh_token): ApiPath<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = json::path_of(&uri);
    tracing::info!("GET /youtube/oauth: refreshing configured token");

    let ctx = match &state.youtube {
        Some(ctx) => ctx.clone(),
        None => {
            tracing::warn!("GET /youtube/oauth: YouTube source is not enabled");
            return json::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "YouTube source is not enabled",
                &path,
            );
        }
    };

    match ctx.oauth.refresh_with_token(&refresh_token).await {
        Ok(body) => {
            if let Some(err) = body.get("error").and_then(|e| e.as_str()) {
                tracing::warn!("GET /youtube/oauth: token refresh returned error: {}", err);
                return json::error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Refreshing access token returned error: {err}"),
                    &path,
                );
            }
            tracing::debug!("GET /youtube/oauth: token refreshed successfully");
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => {
            tracing::error!("GET /youtube/oauth: refresh failed: {}", e);
            json::error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), &path)
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuery {
    pub itag: Option<u64>,
    pub with_client: Option<String>,
}

/// YouTube video ids are exactly 11 characters from `[A-Za-z0-9_-]`. Validating
/// the path segment before it is interpolated into upstream YouTube/cipher URLs
/// rejects malformed or injected ids up front.
fn is_valid_video_id(video_id: &str) -> bool {
    video_id.len() == 11
        && video_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub async fn youtube_stream(
    OriginalUri(uri): OriginalUri,
    ApiPath(video_id): ApiPath<String>,
    ApiQuery(params): ApiQuery<StreamQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = json::path_of(&uri);
    tracing::info!(
        "GET /youtube/stream/{} itag={:?} withClient={:?} Range={:?}",
        video_id,
        params.itag,
        params.with_client,
        headers.get(header::RANGE)
    );

    if !is_valid_video_id(&video_id) {
        return json::error_response(StatusCode::BAD_REQUEST, "Invalid YouTube video id", &path);
    }

    let Some(ctx) = &state.youtube else {
        return json::error_response(
            StatusCode::BAD_REQUEST,
            "YouTube source is not enabled",
            &path,
        );
    };

    let clients = match get_target_clients(ctx, params.with_client.as_deref()) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let visitor_data_str = ctx.visitor_data.read().await.clone();
    let player_page_url = format!("https://www.youtube.com/watch?v={}", video_id);

    let mut last_error = None;
    let mut found_formats = false;
    let mut last_was_exception = false;

    for client in &clients {
        tracing::debug!(
            "GET /youtube/stream/{}: attempting client '{}'",
            video_id,
            client.name()
        );

        let body = match client
            .get_player_body(&video_id, visitor_data_str.as_deref(), ctx.oauth.clone())
            .await
        {
            Some(b) => b,
            None => {
                tracing::debug!(
                    "GET /youtube/stream/{}: client '{}' does not support get_player_body, skipping",
                    video_id,
                    client.name()
                );
                continue;
            }
        };

        if let Err(e) = check_playability(&body, &video_id, client.name()) {
            last_error = Some(e);
            tracing::warn!(
                "{}",
                last_error.as_deref().unwrap_or("Playability check failed")
            );
            continue;
        }

        let streaming_data = match body.get("streamingData") {
            Some(sd) => sd,
            None => {
                let err_str = format!(
                    "Client '{}' returned no streamingData for video '{}'",
                    client.name(),
                    video_id
                );
                tracing::warn!("{}", err_str);
                last_error = Some(err_str);
                continue;
            }
        };

        let adaptive = streaming_data
            .get("adaptiveFormats")
            .and_then(|v| v.as_array());
        let formats = streaming_data.get("formats").and_then(|v| v.as_array());

        found_formats = true;
        last_was_exception = false;

        if params.itag.is_none()
            && let Some(hls_url) = streaming_data
                .get("hlsManifestUrl")
                .and_then(|v| v.as_str())
            && let Some(resp) = stream_via_hls(ctx, hls_url, client.name(), &video_id).await
        {
            return resp;
        }

        let format = if let Some(target_itag) = params.itag {
            let found = find_format_by_itag(adaptive, formats, target_itag);
            match found {
                Some(f) => f,
                None => {
                    let err_str = format!(
                        "itag {} not found in formats returned by client '{}' for video '{}'",
                        target_itag,
                        client.name(),
                        video_id
                    );
                    tracing::debug!("{}", err_str);
                    last_error = Some(err_str);
                    continue;
                }
            }
        } else {
            match select_best_audio_format(adaptive, formats) {
                Some(f) => f,
                None => {
                    let err_str = format!(
                        "Client '{}' returned no suitable audio formats for video '{}'",
                        client.name(),
                        video_id
                    );
                    tracing::warn!("{}", err_str);
                    last_error = Some(err_str);
                    continue;
                }
            }
        };

        let selected_itag = format.get("itag").and_then(|v| v.as_i64());
        let resolved_url =
            match resolve_format_url(format, &player_page_url, &ctx.cipher_manager).await {
                Ok(Some(url)) => url,
                Ok(None) => {
                    let err_str = format!(
                        "Client '{}' could not resolve a URL for video '{}' itag={:?}",
                        client.name(),
                        video_id,
                        selected_itag
                    );
                    last_was_exception = true;
                    tracing::warn!("{}", err_str);
                    last_error = Some(err_str);
                    continue;
                }
                Err(e) => {
                    let err_str = format!(
                        "Client '{}' cipher/n-param resolution failed for video '{}': {}",
                        client.name(),
                        video_id,
                        e
                    );
                    last_was_exception = true;
                    tracing::error!("{}", err_str);
                    last_error = Some(err_str);
                    continue;
                }
            };

        let content_length = format
            .get("contentLength")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let mime_type = format
            .get("mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream");

        let mut final_url = match reqwest::Url::parse(&resolved_url) {
            Ok(u) => u,
            Err(e) => {
                let err_str = format!(
                    "Failed to parse resolved URL for video '{}': {}",
                    video_id, e
                );
                last_was_exception = true;
                tracing::error!("{}", err_str);
                last_error = Some(err_str);
                continue;
            }
        };

        let (start, end) = parse_range_header(headers.get(header::RANGE), content_length)
            .unwrap_or((0, content_length.saturating_sub(1)));
        final_url
            .query_pairs_mut()
            .append_pair("range", &format!("{}-{}", start, end));

        tracing::info!(
            "GET /youtube/stream/{}: streaming via client '{}' itag={:?} mime={} range={}-{}",
            video_id,
            client.name(),
            selected_itag,
            mime_type,
            start,
            end
        );

        let upstream = match ctx.http.get(final_url.as_str()).send().await {
            Ok(r) => r,
            Err(e) => {
                let err_str = format!(
                    "Upstream request failed for client '{}': {}",
                    client.name(),
                    e
                );
                last_was_exception = true;
                tracing::error!("{}", err_str);
                last_error = Some(err_str);
                continue;
            }
        };

        if !upstream.status().is_success() {
            let err_str = format!(
                "Upstream returned {} for client '{}'",
                upstream.status(),
                client.name()
            );
            last_was_exception = true;
            tracing::warn!("{}", err_str);
            last_error = Some(err_str);
            continue;
        }

        let mut resp_headers = HeaderMap::new();
        if let Ok(v) = header::HeaderValue::from_str(mime_type) {
            resp_headers.insert(header::CONTENT_TYPE, v);
        }

        if let Some(v) = upstream.headers().get(header::CONTENT_LENGTH) {
            resp_headers.insert(header::CONTENT_LENGTH, v.clone());
        }
        if let Some(v) = upstream.headers().get(header::CONTENT_RANGE) {
            resp_headers.insert(header::CONTENT_RANGE, v.clone());
        } else if start > 0 || end < content_length.saturating_sub(1) {
            let cr = format!("bytes {}-{}/{}", start, end, content_length);
            if let Ok(v) = header::HeaderValue::from_str(&cr) {
                resp_headers.insert(header::CONTENT_RANGE, v);
            }
        }

        return (
            upstream.status(),
            resp_headers,
            Body::from_stream(upstream.bytes_stream()),
        )
            .into_response();
    }

    let (status, error_msg) = if found_formats && params.itag.is_some() && !last_was_exception {
        (
            StatusCode::BAD_REQUEST,
            last_error.unwrap_or_else(|| {
                format!(
                    "No formats found with the requested itag for video '{}'",
                    video_id
                )
            }),
        )
    } else if last_was_exception {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            last_error.unwrap_or_else(|| format!("This video cannot be loaded: '{}'", video_id)),
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            last_error
                .unwrap_or_else(|| format!("Could not find formats for video '{}'", video_id)),
        )
    };

    tracing::warn!("GET /youtube/stream/{}: {}", video_id, error_msg);
    json::error_response(status, error_msg, &path)
}

type ClientFilterResult = Result<
    Vec<Arc<dyn audium::media::sources::youtube::clients::YouTubeClient>>,
    (StatusCode, Json<serde_json::Value>),
>;

/// Extracts the `n` parameter from a googlevideo URL, if present. The value
/// must be translated by the cipher manager before the URL will be accepted.
fn extract_n_param(url: &str) -> Option<&str> {
    if let Some(pos) = url.find("/n/") {
        return url[pos + 3..].split('/').next();
    }
    url.split("&n=")
        .nth(1)
        .or_else(|| url.split("?n=").nth(1))
        .and_then(|s| s.split('&').next())
}

/// Resolves a single HLS resource URL, applying n-param translation when needed.
async fn resolve_seg_url(
    url: &str,
    cipher: &Arc<audium::media::sources::youtube::cipher::YouTubeCipherManager>,
    player_url: &str,
) -> Result<String, String> {
    let Some(n) = extract_n_param(url) else {
        return Ok(url.to_string());
    };
    cipher
        .resolve_url(url, player_url, Some(n), None)
        .await
        .map_err(|e| e.to_string())
}

/// Streams audio over YouTube's HLS manifest.
///
/// Direct `videoplayback` URLs are increasingly refused with HTTP 403 unless a
/// PoToken is attached, while the HLS manifest for the same video keeps
/// working. The manifest itself is still fetchable from a blocked IP; only the
/// media segments are gated. This therefore returns `None` (so the caller can
/// fall back to the direct-URL path) whenever the manifest cannot be resolved
/// or the first segment cannot be read, and only commits to an HLS response
/// once a real segment has been fetched.
async fn stream_via_hls(
    ctx: &audium::media::sources::youtube::YoutubeStreamContext,
    hls_url: &str,
    client_name: &str,
    video_id: &str,
) -> Option<axum::response::Response> {
    use audium::media::sources::youtube::hls::{
        fetcher::fetch_segment_into, resolver::resolve_playlist,
    };

    let (segments, map) = match resolve_playlist(&ctx.http, hls_url).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                "GET /youtube/stream/{}: HLS playlist resolution failed for client '{}': {}",
                video_id,
                client_name,
                e
            );
            return None;
        }
    };

    if segments.is_empty() {
        tracing::debug!(
            "GET /youtube/stream/{}: HLS playlist for client '{}' had no segments",
            video_id,
            client_name
        );
        return None;
    }

    let cipher = ctx.cipher_manager.clone();
    let player_url = format!("https://www.youtube.com/watch?v={}", video_id);

    let mut body = Vec::new();
    if let Some(map_res) = &map {
        let mut resolved = map_res.clone();
        resolved.url = match resolve_seg_url(&map_res.url, &cipher, &player_url).await {
            Ok(u) => u,
            Err(e) => {
                tracing::debug!(
                    "GET /youtube/stream/{}: HLS map URL resolution failed: {}",
                    video_id,
                    e
                );
                return None;
            }
        };
        if let Err(e) = fetch_segment_into(&ctx.http, &resolved, &mut body).await {
            tracing::debug!(
                "GET /youtube/stream/{}: HLS init segment fetch failed for client '{}': {}",
                video_id,
                client_name,
                e
            );
            return None;
        }
    }

    let first = &segments[0];
    let mut resolved_first = first.clone();
    resolved_first.url = match resolve_seg_url(&first.url, &cipher, &player_url).await {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!(
                "GET /youtube/stream/{}: HLS segment URL resolution failed: {}",
                video_id,
                e
            );
            return None;
        }
    };
    let mut first_data = Vec::new();
    if let Err(e) = fetch_segment_into(&ctx.http, &resolved_first, &mut first_data).await {
        tracing::debug!(
            "GET /youtube/stream/{}: HLS segment fetch failed for client '{}': {}",
            video_id,
            client_name,
            e
        );
        return None;
    }
    body.extend_from_slice(&first_data);

    const PREFETCH_AHEAD: usize = 3;
    let (tx, rx) = flume::bounded::<Result<bytes::Bytes, String>>(PREFETCH_AHEAD);
    let rest = segments[1..].to_vec();
    let http = ctx.http.clone();
    let cipher_bg = cipher.clone();
    let player_url_bg = player_url.clone();
    let total = segments.len();

    tokio::spawn(async move {
        {
            for seg in rest {
                if tx.is_disconnected() {
                    return;
                }
                let url = match resolve_seg_url(&seg.url, &cipher_bg, &player_url_bg).await {
                    Ok(u) => u,
                    Err(e) => {
                        let _ = tx.send(Err(format!("HLS URL resolution: {e}")));
                        return;
                    }
                };
                let mut resolved = seg.clone();
                resolved.url = url;
                let mut buf = Vec::new();
                match fetch_segment_into(&http, &resolved, &mut buf).await {
                    Ok(()) => {
                        if tx.send(Ok(bytes::Bytes::from(buf))).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(format!("HLS segment fetch: {e}")));
                        return;
                    }
                }
            }
        }
    });

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("video/mp2t"),
    );

    let first_chunk = bytes::Bytes::from(body);
    let stream = futures::stream::unfold((Some(first_chunk), rx), |(pending, rx)| async move {
        if let Some(chunk) = pending {
            return Some((Ok(chunk), (None, rx)));
        }
        match rx.recv_async().await {
            Ok(Ok(chunk)) => Some((Ok(chunk), (None, rx))),
            Ok(Err(e)) => Some((Err(std::io::Error::other(e)), (None, rx))),
            Err(_) => None,
        }
    });

    tracing::info!(
        "GET /youtube/stream/{}: streaming via HLS with client '{}' ({} segment(s))",
        video_id,
        client_name,
        total
    );

    Some((StatusCode::OK, resp_headers, Body::from_stream(stream)).into_response())
}

fn get_target_clients(
    ctx: &audium::media::sources::youtube::YoutubeStreamContext,
    filter: Option<&str>,
) -> ClientFilterResult {
    if let Some(filter) = filter {
        let lower = filter.to_lowercase();
        let matched: Vec<Arc<dyn audium::media::sources::youtube::clients::YouTubeClient>> = ctx
            .clients
            .iter()
            .filter(|c| c.name().to_lowercase() == lower)
            .cloned()
            .collect();

        if matched.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("No client named '{}' is configured for playback", filter)
                })),
            ));
        }
        Ok(matched)
    } else {
        Ok(ctx.clients.to_vec())
    }
}

fn check_playability(
    body: &serde_json::Value,
    video_id: &str,
    client_name: &str,
) -> Result<(), String> {
    let status = body
        .get("playabilityStatus")
        .and_then(|p| p.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("UNKNOWN");

    if status == "OK" {
        return Ok(());
    }

    let reason = body
        .get("playabilityStatus")
        .and_then(|p| p.get("reason"))
        .and_then(|r| r.as_str())
        .unwrap_or("no reason provided");

    Err(format!(
        "Video '{}' is not playable via client '{}': status={}, reason={}",
        video_id, client_name, status, reason
    ))
}

fn find_format_by_itag<'a>(
    adaptive: Option<&'a Vec<serde_json::Value>>,
    formats: Option<&'a Vec<serde_json::Value>>,
    target_itag: u64,
) -> Option<&'a serde_json::Value> {
    adaptive
        .iter()
        .flat_map(|v| v.iter())
        .chain(formats.iter().flat_map(|v| v.iter()))
        .find(|f| {
            f.get("itag")
                .and_then(|v| v.as_u64())
                .map(|i| i == target_itag)
                .unwrap_or(false)
        })
}

/// Parses a single-range `bytes=` header into a clamped inclusive `[start, end]`.
///
/// Returns `None` for absent, unparseable, or unsatisfiable ranges so callers
/// fall back to the full `[0, content_length - 1]` range rather than forwarding
/// a malformed range upstream. Open-ended (`bytes=1000-`) and suffix
/// (`bytes=-500`) forms are supported.
fn parse_range_header(
    range_header: Option<&axum::http::HeaderValue>,
    content_length: u64,
) -> Option<(u64, u64)> {
    let last = content_length.checked_sub(1)?;
    let range_val = range_header?.to_str().ok()?;

    let regex = regex::Regex::new(r"^bytes=(\d*)-(\d*)$").expect("invalid regex");
    let caps = regex.captures(range_val.trim())?;
    let start_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let end_str = caps.get(2).map(|m| m.as_str()).unwrap_or("");

    if start_str.is_empty() {
        if end_str.is_empty() {
            return None;
        }
        let suffix = end_str.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some((content_length.saturating_sub(suffix).min(last), last));
    }

    let start = start_str.parse::<u64>().ok()?;
    if start > last {
        return None;
    }

    let end = match end_str.parse::<u64>() {
        Ok(end) => end.min(last),
        Err(_) => last,
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::{is_valid_video_id, parse_range_header};
    use axum::http::HeaderValue;

    fn range(value: &str) -> Option<HeaderValue> {
        HeaderValue::from_str(value).ok()
    }

    #[test]
    fn absent_range_falls_back_to_the_caller_default() {
        assert_eq!(parse_range_header(None, 1000), None);
    }

    #[test]
    fn parses_bounded_open_and_suffix_ranges() {
        assert_eq!(
            parse_range_header(range("bytes=0-499").as_ref(), 1000),
            Some((0, 499))
        );
        assert_eq!(
            parse_range_header(range("bytes=500-").as_ref(), 1000),
            Some((500, 999))
        );
        assert_eq!(
            parse_range_header(range("bytes=-200").as_ref(), 1000),
            Some((800, 999))
        );
    }

    #[test]
    fn clamps_an_over_long_end_to_the_last_byte() {
        assert_eq!(
            parse_range_header(range("bytes=900-99999").as_ref(), 1000),
            Some((900, 999))
        );
    }

    #[test]
    fn rejects_unsatisfiable_or_malformed_ranges() {
        assert_eq!(
            parse_range_header(range("bytes=5000-6000").as_ref(), 1000),
            None
        );

        assert_eq!(
            parse_range_header(range("bytes=800-100").as_ref(), 1000),
            None
        );

        assert_eq!(parse_range_header(range("bytes=-0").as_ref(), 1000), None);
        assert_eq!(parse_range_header(range("bytes=abc").as_ref(), 1000), None);

        assert_eq!(
            parse_range_header(range("bytes=0-1,5-6").as_ref(), 1000),
            None
        );

        assert_eq!(parse_range_header(None, 0), None);
    }

    #[test]
    fn accepts_well_formed_video_ids() {
        assert!(is_valid_video_id("dQw4w9WgXcQ"));
        assert!(is_valid_video_id("_-aB3cD4eF5"));
    }

    #[test]
    fn rejects_malformed_or_injected_video_ids() {
        assert!(!is_valid_video_id("dQw4w9WgXc"));
        assert!(!is_valid_video_id("dQw4w9WgXcQQ"));

        assert!(!is_valid_video_id("aaaaaaaaa/ "));
        assert!(!is_valid_video_id("a?b=c&d=e#f"));
        assert!(!is_valid_video_id("../../etc/x"));
        assert!(!is_valid_video_id("dQw4w9WgXc\n"));
        assert!(!is_valid_video_id(""));
    }
}
