// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Minimal liveness/readiness surface for orchestrators (k8s, docker healthcheck).
//!
//! Deliberately *not* placed behind the `/v4` auth middleware: probes such as
//! the Docker `HEALTHCHECK` cannot rotate the authorization secret, so the
//! endpoint must answer unauthenticated. To avoid leaking operational state to
//! anyone who can reach the port, the response is *tiered*:
//!
//! - unauthenticated → `{"status":"ok"}` only;
//! - with a correct `Authorization` header → adds `uptimeMs` and `players`.
//!
//! The tier is chosen by constant-time comparison against the shared secret, so
//! the endpoint doubles as a cheap way for an operator to verify credentials
//! without reaching into a `/v4` route.

use std::sync::Arc;

use axum::{Json, extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse};
use serde::Serialize;

use crate::server::AppState;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    /// Present only on the authenticated tier.
    #[serde(rename = "uptimeMs", skip_serializing_if = "Option::is_none")]
    uptime_ms: Option<u128>,
    /// Present only on the authenticated tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    players: Option<usize>,
}

/// True when `headers` carries an `Authorization` value equal to `secret`.
///
/// Constant-time so a wrong secret cannot be discovered byte-by-byte through
/// response timing. Split out from [`is_authorized`] so it can be unit-tested
/// without constructing a whole [`AppState`].
pub(crate) fn header_matches_secret(headers: &HeaderMap, secret: &str) -> bool {
    use subtle::ConstantTimeEq;

    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|auth| bool::from(auth.as_bytes().ct_eq(secret.as_bytes())))
}

/// True when the request carries the configured authorization secret.
///
/// Shares the constant-time comparison used by the `/v4` middleware; kept as a
/// free function so both call sites stay in lockstep if the scheme changes.
pub(crate) fn is_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    header_matches_secret(headers, &state.config.server.authorization)
}

/// GET /health — always 200 when the process is serving.
///
/// Returns `{"status":"ok"}` to unauthenticated callers (enough for a
/// liveness/readiness probe) and adds uptime plus the current player count when
/// the caller presents the authorization secret.
pub async fn health_check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let detailed = is_authorized(&state, &headers);

    let health = Health {
        status: "ok",
        uptime_ms: detailed.then(|| state.start_time.elapsed().as_millis()),
        players: detailed.then(|| state.sessions.iter().map(|s| s.players.len()).sum()),
    };
    (StatusCode::OK, Json(health))
}

#[cfg(test)]
mod tests {
    use super::{Health, header_matches_secret};
    use axum::http::{HeaderMap, HeaderValue};

    fn headers_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn correct_secret_is_accepted() {
        assert!(header_matches_secret(&headers_with("s3cret"), "s3cret"));
    }

    #[test]
    fn wrong_or_missing_secret_is_rejected() {
        assert!(!header_matches_secret(&headers_with("s3cret"), "s3cret!"));
        assert!(!header_matches_secret(&HeaderMap::new(), "s3cret"));

        assert!(!header_matches_secret(&headers_with("s3"), "s3cret"));
    }

    #[test]
    fn unauthenticated_body_is_status_only() {
        let body = serde_json::to_string(&Health {
            status: "ok",
            uptime_ms: None,
            players: None,
        })
        .unwrap();
        assert_eq!(body, r#"{"status":"ok"}"#);
    }

    #[test]
    fn authenticated_body_includes_detail() {
        let body = serde_json::to_string(&Health {
            status: "ok",
            uptime_ms: Some(1234),
            players: Some(2),
        })
        .unwrap();
        assert_eq!(body, r#"{"status":"ok","uptimeMs":1234,"players":2}"#);
    }
}
