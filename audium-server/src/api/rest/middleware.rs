// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use tracing::warn;

use crate::server::AppState;

pub async fn check_auth(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // A missing header is 401, a supplied-but-wrong secret is 403 — mirroring
    // official Lavalink (RequestAuthorizationFilter.kt) so clients can tell
    // misconfiguration (403) apart from a missing header (401). The comparison
    // itself is the shared constant-time helper in `crate::health`.
    let present = req.headers().contains_key("authorization");

    if crate::health::is_authorized(&state, req.headers()) {
        return Ok(next.run(req).await);
    }

    if present {
        warn!("REST authorization failed: invalid password");
        Err(StatusCode::FORBIDDEN)
    } else {
        warn!("REST authorization failed: missing authorization header");
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub async fn add_response_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    // Official Lavalink stamps every REST response with this header
    // (ResponseHeaderFilter.kt); keep the wire name identical.
    response
        .headers_mut()
        .insert("Lavalink-Api-Version", HeaderValue::from_static("4"));
    response
}
