// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Global connection admission control.
//!
//! A per-IP token bucket ([`crate::api::rate_limit`]) bounds *request rate*, but a
//! slow or half-open connection flood can still pin file descriptors and spawn one
//! task per socket. This middleware holds a permit from a shared semaphore for the
//! lifetime of each request/connection, rejecting over-limit connections with
//! `503 Service Unavailable` before any handler work happens.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use tracing::warn;

use crate::server::AppState;

pub async fn connection_limit(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(limit) = state.connection_limit.clone() else {
        return next.run(req).await;
    };

    // The WebSocket route owns its own long-lived permit inside the handler: this
    // middleware's permit is released as soon as the 101 response is produced,
    // while the socket task keeps running afterwards. Skipping avoids counting a
    // socket twice (here + in the handler), which would halve the real limit.
    if req.uri().path() == "/v4/websocket" {
        return next.run(req).await;
    }

    match limit.try_acquire_owned() {
        Ok(permit) => {
            let resp = next.run(req).await;
            drop(permit);
            resp
        }
        Err(_) => {
            warn!("connection limit reached, rejecting new connection");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [("retry-after", "1")],
                "Server connection limit reached",
            )
                .into_response()
        }
    }
}
