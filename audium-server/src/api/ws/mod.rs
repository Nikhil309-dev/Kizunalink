// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

mod handler;
mod opcodes;

pub use opcodes::{MAX_RESUME_TIMEOUT_SECS, clamp_resume_timeout};

use std::sync::Arc;

use axum::{
    extract::{State, ws::WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
pub use handler::handle_socket;
use tracing::{debug, info, warn};

use crate::{
    common::types::{SessionId, UserId},
    server::AppState,
};

pub async fn websocket_handler(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Result<Response, (StatusCode, &'static str)> {
    if !crate::health::is_authorized(&state, &headers) {
        if headers.contains_key("authorization") {
            warn!("Authorization failed: Invalid password provided");
        } else {
            warn!("Authorization failed: Missing Authorization header");
        }
        return Err((StatusCode::UNAUTHORIZED, "Unauthorized"));
    }

    let Some(user_id) = headers
        .get("user-id")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(std::num::NonZeroU64::new)
        .map(|n| UserId(n.get()))
    else {
        warn!("Client connected without a valid 'User-Id' header");
        return Err((StatusCode::BAD_REQUEST, "Missing or invalid User-Id header"));
    };

    if let Some(name) = headers.get("client-name").and_then(|h| h.to_str().ok()) {
        info!("Incoming connection from client: {name}");
    } else {
        debug!("Client connected without 'Client-Name' header");
    }

    let client_session_id = headers
        .get("session-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| SessionId(s.to_string()));

    let resuming = client_session_id
        .as_ref()
        .is_some_and(|sid| state.resumable_sessions.contains_key(sid));

    const MAX_CLIENT_WS_MESSAGE_SIZE: usize = 1024 * 1024;
    let ws = ws
        .max_message_size(MAX_CLIENT_WS_MESSAGE_SIZE)
        .max_frame_size(MAX_CLIENT_WS_MESSAGE_SIZE);

    let ws_permit = match &state.connection_limit {
        Some(limit) => match limit.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                warn!("WebSocket rejected: connection limit reached");
                return Err((StatusCode::SERVICE_UNAVAILABLE, "Connection limit reached"));
            }
        },
        None => None,
    };

    let upgrade_callback = move |socket| async move {
        let _permit = ws_permit;
        handle_socket(socket, state, Some(user_id), client_session_id).await
    };
    let mut response = ws.on_upgrade(upgrade_callback).into_response();

    let resumed_val = if resuming {
        axum::http::HeaderValue::from_static("true")
    } else {
        axum::http::HeaderValue::from_static("false")
    };
    response
        .headers_mut()
        .insert("Session-Resumed", resumed_val);

    response.headers_mut().insert(
        "Lavalink-Api-Version",
        axum::http::HeaderValue::from_static("4"),
    );
    response.headers_mut().insert(
        "Lavalink-Major-Version",
        axum::http::HeaderValue::from_static("4"),
    );

    Ok(response)
}
