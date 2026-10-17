// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! JSON extractors and response wrappers that keep every `/v4` error body in the
//! same shape as the rest of the API.
//!
//! Axum's built-in `Query`/`Json` rejections reply with `text/plain`
//! (`Failed to deserialize query string: …` / `Failed to deserialize the JSON
//! body: …`), which no Lavalink client parses. `ApiQuery`/`ApiJson` wrap the
//! built-in extractors and translate a rejection into the project's
//! `AudiumError` JSON (`{timestamp,status,error,message,path}`), so a client
//! sees one error contract on every route.

use axum::{
    extract::{FromRequest, FromRequestParts, OriginalUri, Request, rejection::JsonRejection},
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::common::AudiumError;

/// Canonical JSON error body for a request path.
pub fn error_response(status: StatusCode, message: impl Into<String>, path: &str) -> Response {
    let error = AudiumError::new(
        status.as_u16(),
        status.canonical_reason().unwrap_or("Error"),
        message,
        path,
    );
    (status, axum::Json(error)).into_response()
}

/// Canonical `404` body for a missing session or player.
pub fn not_found(message: impl Into<String>, path: &str) -> Response {
    error_response(StatusCode::NOT_FOUND, message, path)
}

/// Canonical `400` body for a malformed request.
pub fn bad_request(message: impl Into<String>, path: &str) -> Response {
    error_response(StatusCode::BAD_REQUEST, message, path)
}

/// The full client-facing request path, for use as the `path` field of an
/// `AudiumError` from a handler that holds the raw `OriginalUri`.
pub fn path_of(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|pq| pq.as_str().to_owned())
        .unwrap_or_else(|| uri.path().to_owned())
}

/// The full client-facing request path.
///
/// `nest("/v4", …)` strips the mount prefix from `Request::uri()`, so an extractor
/// running inside the nested router would otherwise report `/loadtracks` instead of
/// `/v4/loadtracks`. `OriginalUri` carries the un-stripped URI when the outer router
/// installs it; fall back to the (possibly stripped) URI otherwise.
fn full_path(uri: &Uri, extensions: &axum::http::Extensions) -> String {
    let uri = extensions
        .get::<OriginalUri>()
        .map(|OriginalUri(u)| u)
        .unwrap_or(uri);
    uri.path_and_query()
        .map(|pq| pq.as_str().to_owned())
        .unwrap_or_else(|| uri.path().to_owned())
}

/// `axum::Json` that answers deserialization failures with the JSON error shape.
///
/// Every JSON rejection is mapped to `400 Bad Request`: the project (and official
/// Lavalink) treat a malformed body as a client error, whereas axum defaults some
/// variants to 422/415.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let path = full_path(req.uri(), req.extensions());
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => {
                let message = match rejection {
                    JsonRejection::MissingJsonContentType(_) => {
                        "Expected a JSON request body with Content-Type: application/json"
                            .to_owned()
                    }
                    other => other.body_text(),
                };
                // A body that exceeds `server.max_body_bytes` is reported as 413
                // (NodeLink parity) rather than a generic 400, so clients can tell
                // "too big" apart from "malformed".
                let status = if message.contains("length limit exceeded") {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                };
                Err(error_response(status, message, &path))
            }
        }
    }
}

impl<T: Serialize> IntoResponse for ApiJson<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::Query` that answers deserialization failures with the JSON error shape.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let path = full_path(&parts.uri, &parts.extensions);
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(error_response(
                StatusCode::BAD_REQUEST,
                rejection.body_text(),
                &path,
            )),
        }
    }
}

/// `axum::Path` that answers path-parameter deserialization failures with the
/// JSON error shape (e.g. a non-numeric `{guild_id}`).
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let path = full_path(&parts.uri, &parts.extensions);
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(ApiPath(value)),
            Err(rejection) => Err(error_response(
                StatusCode::BAD_REQUEST,
                rejection.body_text(),
                &path,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(resp: Response) -> serde_json::Value {
        let (parts, body) = resp.into_parts();
        assert!(parts.headers.get("content-type").is_some_and(|v| {
            v.to_str()
                .unwrap_or_default()
                .starts_with("application/json")
        }));
        let bytes = to_bytes(body, 64 * 1024).await.expect("body");
        serde_json::from_slice(&bytes).expect("json envelope")
    }

    /// Every route-level failure must carry the same five fields; a client that
    /// parses `message` should never have to special-case one endpoint.
    #[tokio::test]
    async fn not_found_uses_the_canonical_envelope() {
        let v = body_json(not_found("Session not found: x", "/v4/sessions/x/players")).await;
        assert_eq!(v["status"], 404);
        assert_eq!(v["error"], "Not Found");
        assert_eq!(v["message"], "Session not found: x");
        assert_eq!(v["path"], "/v4/sessions/x/players");
        assert!(v["timestamp"].as_u64().unwrap_or(0) > 0);
    }

    #[tokio::test]
    async fn bad_request_uses_the_canonical_envelope() {
        let v = body_json(bad_request("Invalid encoded track", "/v4/lyrics")).await;
        assert_eq!(v["status"], 400);
        assert_eq!(v["error"], "Bad Request");
        assert_eq!(v["message"], "Invalid encoded track");
        assert_eq!(v["path"], "/v4/lyrics");
    }

    #[test]
    fn path_of_keeps_query_string_and_defaults_to_root() {
        let uri: Uri = "/v4/lyrics?track=abc&skipTrackSource=true".parse().unwrap();
        assert_eq!(path_of(&uri), "/v4/lyrics?track=abc&skipTrackSource=true");
    }
}
