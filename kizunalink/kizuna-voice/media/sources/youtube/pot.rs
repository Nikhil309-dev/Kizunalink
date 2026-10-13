// Copyright (c) 2026 nikcodex (KizunaLink)
// Licensed under the MIT License

//! Proof-of-Origin (PoToken) provider integration.
//!
//! YouTube requires a PoToken on the GVS format URLs returned by the
//! WEB/ANDROID/IOS clients. Without one, playback from a datacenter egress
//! intermittently fails with `HTTP 403` on `googlevideo.com`. Tokens are minted
//! by an external [`bgutil-ytdlp-pot-provider`] HTTP server and bound to
//! `visitorData`; the same token is reused for both metadata lookups and
//! playback.
//!
//! [`bgutil-ytdlp-pot-provider`]: https://github.com/Brainicism/bgutil-ytdlp-pot-provider

use std::{
    fmt,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde_json::Value;

/// `Arc`-shared PoToken provider.
pub type SharedPotProvider = Arc<YouTubePotProvider>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// bgutil tokens are valid for hours; cache comfortably below that.
const CACHE_TTL: Duration = Duration::from_secs(4 * 60 * 60);

/// Why a PoToken could not be obtained.
///
/// Distinguishing these is the difference between an actionable operator error
/// and a silent, untokened request that YouTube answers with HTTP 403 later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PotError {
    /// `sources.youtube.pot.enabled` is false — no provider is configured.
    Disabled,
    /// A provider is configured but the request to it failed at the transport
    /// layer (connection refused, DNS, TLS, timeout). The provider is down or
    /// unreachable.
    Unreachable(String),
    /// The provider answered with a non-success HTTP status.
    ProviderStatus(u16, String),
    /// The provider answered 2xx but the body was not valid JSON, or lacked a
    /// usable token field.
    InvalidResponse(String),
    /// The request could not be bound to a video id (empty id).
    NotBound,
}

impl fmt::Display for PotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PotError::Disabled => write!(f, "PoToken provider is disabled"),
            PotError::Unreachable(e) => write!(
                f,
                "PoToken provider is unreachable (is the bgutil server running and is \
                 `sources.youtube.pot.url` correct?): {e}"
            ),
            PotError::ProviderStatus(code, body) => write!(
                f,
                "PoToken provider returned HTTP {code}: {}",
                truncate(body, 200)
            ),
            PotError::InvalidResponse(e) => {
                write!(f, "PoToken provider returned an unusable response: {e}")
            }
            PotError::NotBound => write!(f, "PoToken cannot be bound to an empty video id"),
        }
    }
}

impl std::error::Error for PotError {}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

struct CachedToken {
    token: String,
    binding: String,
    expires: Instant,
}

pub struct YouTubePotProvider {
    http: Arc<reqwest::Client>,
    /// `None` disables the provider (`pot::disabled`).
    base_url: Option<String>,
    bypass_cache: bool,
    cache: Mutex<Option<CachedToken>>,
}

impl YouTubePotProvider {
    pub fn new(http: Arc<reqwest::Client>, base_url: String, bypass_cache: bool) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        // A co-located provider (loopback URL) must be reached directly: the
        // shared client carries the YouTube egress proxy, which lives on another
        // network and cannot see 127.0.0.1. A remote provider keeps the shared
        // (proxy-aware) client.
        let http = if host_is_loopback(&base_url) {
            Arc::new(
                reqwest::Client::builder()
                    .no_proxy()
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
            )
        } else {
            http
        };
        Self {
            http,
            base_url: Some(base_url),
            bypass_cache,
            cache: Mutex::new(None),
        }
    }

    /// Whether a provider is configured at all (i.e. `pot.enabled = true` with a
    /// URL). Disabled providers are not an error condition, just an absent token.
    pub fn enabled(&self) -> bool {
        self.base_url.is_some()
    }

    /// The provider base URL, for diagnostics.
    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// Mint (or reuse) a PoToken bound to `video_id`, reporting *why* it failed.
    ///
    /// GVS format URLs from the WEB/ANDROID/IOS clients are bound to the video id
    /// (the `content` binding), so the token must be minted per video.
    pub async fn fetch_typed(&self, video_id: &str) -> Result<String, PotError> {
        let base = self.base_url.as_deref().ok_or(PotError::Disabled)?;
        if video_id.is_empty() {
            return Err(PotError::NotBound);
        }

        if !self.bypass_cache
            && let Some(token) = self.cached(video_id)
        {
            return Ok(token);
        }

        let body = serde_json::json!({
            "content_binding": video_id,
            "bypass_cache": self.bypass_cache,
        });
        let url = format!("{}/get_pot", base);

        let response = self
            .http
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| PotError::Unreachable(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(PotError::ProviderStatus(status.as_u16(), text));
        }

        let json: Value = response
            .json()
            .await
            .map_err(|e| PotError::InvalidResponse(e.to_string()))?;

        let token = parse_token(&json)
            .ok_or_else(|| PotError::InvalidResponse(truncate(&json.to_string(), 200)))?;
        tracing::debug!("PoToken acquired for video {}…", prefix(video_id));
        *self.cache.lock().unwrap() = Some(CachedToken {
            token: token.clone(),
            binding: video_id.to_string(),
            expires: Instant::now() + CACHE_TTL,
        });
        Ok(token)
    }

    /// Convenience wrapper that collapses the typed error to `Option`, preserving
    /// the old behaviour for callers that only need "a token or not".
    pub async fn fetch(&self, video_id: &str) -> Option<String> {
        match self.fetch_typed(video_id).await {
            Ok(t) => Some(t),
            Err(PotError::Disabled) => None,
            Err(e) => {
                tracing::warn!("PoToken provider request failed: {e}");
                None
            }
        }
    }

    /// Probe the provider with a throwaway binding to confirm it is reachable and
    /// well-formed. Used once at startup so misconfiguration surfaces loudly
    /// instead of as an HTTP 403 during playback.
    pub async fn probe(&self) -> Result<(), PotError> {
        // A dedicated binding keeps the probe out of the real token cache.
        self.fetch_typed("kizunalink-pot-probe").await.map(|_| ())
    }

    fn cached(&self, video_id: &str) -> Option<String> {
        let guard = self.cache.lock().unwrap();
        let entry = guard.as_ref()?;
        if entry.expires > Instant::now() && entry.binding == video_id {
            Some(entry.token.clone())
        } else {
            None
        }
    }
}

/// Extract the token from a provider response, accepting both the upstream
/// (`poToken`) and Rust (`po_token`) field names.
fn parse_token(json: &Value) -> Option<String> {
    json.get("poToken")
        .or_else(|| json.get("po_token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn prefix(s: &str) -> String {
    s.chars().take(12).collect()
}

/// Whether a base URL targets this machine (loopback), so it must bypass the
/// YouTube egress proxy.
fn host_is_loopback(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = if let Some(rest) = rest.strip_prefix('[') {
        // bracketed IPv6 literal, e.g. [::1]:4416
        rest.split(']').next().unwrap_or("")
    } else {
        rest.split(['/', ':', '?']).next().unwrap_or("")
    };
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn disabled_provider() -> YouTubePotProvider {
    YouTubePotProvider {
        http: Arc::new(reqwest::Client::new()),
        base_url: None,
        bypass_cache: false,
        cache: Mutex::new(None),
    }
}

/// The shared no-op provider used when `sources.youtube.pot` is disabled.
pub fn disabled() -> SharedPotProvider {
    Arc::new(disabled_provider())
}

static GLOBAL: OnceLock<SharedPotProvider> = OnceLock::new();

/// Publish the process-wide provider so client request builders can attach
/// tokens without threading a field through every client struct. Called once by
/// `YouTubeSource::new`; later calls are ignored (only one YouTube source).
pub fn set_global(provider: SharedPotProvider) {
    let _ = GLOBAL.set(provider);
}

/// The process-wide provider, if one has been installed.
pub fn global() -> Option<&'static SharedPotProvider> {
    GLOBAL.get()
}

/// Fetch a PoToken bound to `video_id` from the global provider, if enabled.
pub async fn fetch_global(video_id: &str) -> Option<String> {
    let provider = global()?;
    provider.fetch(video_id).await
}

/// Typed variant of [`fetch_global`]: reports whether the provider is disabled or
/// failed, so callers can log an actionable message instead of silently dropping
/// the token. Returns `Ok(None)` only when no provider is installed or it is
/// explicitly disabled.
pub async fn fetch_global_typed(video_id: &str) -> Result<Option<String>, PotError> {
    match global() {
        None => Ok(None),
        Some(provider) => match provider.fetch_typed(video_id).await {
            Ok(token) => Ok(Some(token)),
            Err(PotError::Disabled) => Ok(None),
            Err(e) => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_upstream_camelcase_token() {
        let v = json!({"poToken": "abc123", "contentBinding": "vd"});
        assert_eq!(parse_token(&v).as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_rust_snakecase_token() {
        let v = json!({"po_token": "xyz"});
        assert_eq!(parse_token(&v).as_deref(), Some("xyz"));
    }

    #[test]
    fn rejects_empty_or_missing_token() {
        assert_eq!(parse_token(&json!({"poToken": ""})), None);
        assert_eq!(parse_token(&json!({"error": "boom"})), None);
    }

    #[test]
    fn disabled_provider_returns_no_token() {
        let p = disabled_provider();
        assert!(p.base_url.is_none());
        assert!(!p.enabled());
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(rt.block_on(p.fetch("vd")), None);
        assert_eq!(rt.block_on(p.fetch_typed("vd")), Err(PotError::Disabled));
    }

    #[test]
    fn empty_video_id_is_not_bound() {
        let p = YouTubePotProvider::new(
            Arc::new(reqwest::Client::new()),
            "http://127.0.0.1:1".into(),
            false,
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(rt.block_on(p.fetch_typed("")), Err(PotError::NotBound));
    }

    #[test]
    fn enabled_reports_configured_provider() {
        let p = YouTubePotProvider::new(
            Arc::new(reqwest::Client::new()),
            "http://127.0.0.1:4416".into(),
            false,
        );
        assert!(p.enabled());
        assert_eq!(p.base_url(), Some("http://127.0.0.1:4416"));
    }

    #[test]
    fn loopback_detection() {
        assert!(host_is_loopback("http://127.0.0.1:4416"));
        assert!(host_is_loopback("http://localhost:4416/get_pot"));
        assert!(host_is_loopback("http://[::1]:4416"));
        assert!(!host_is_loopback("http://pot.example.com:4416"));
        assert!(!host_is_loopback("http://10.0.0.5:4416"));
    }

    /// Spawn an in-process mock bgutil provider and return its base URL.
    async fn spawn_mock_provider(
        responder: impl Fn(&str) -> axum::response::Response + Send + Sync + 'static,
    ) -> String {
        use axum::{Router, routing::post};

        let responder = std::sync::Arc::new(responder);
        let app = Router::new().route(
            "/get_pot",
            post(move |body: String| {
                let responder = responder.clone();
                async move { responder(&body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn provider_for(base: String) -> YouTubePotProvider {
        YouTubePotProvider::new(Arc::new(reqwest::Client::new()), base, false)
    }

    #[tokio::test]
    async fn fetch_typed_returns_token_and_binds_to_video() {
        let base = spawn_mock_provider(|body| {
            assert!(
                body.contains("\"content_binding\":\"vid1\""),
                "body: {body}"
            );
            axum::response::Response::builder()
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"poToken":"tok-from-mock"}"#))
                .unwrap()
        })
        .await;
        let p = provider_for(base);
        assert_eq!(p.fetch_typed("vid1").await.unwrap(), "tok-from-mock");
    }

    #[tokio::test]
    async fn fetch_typed_caches_per_binding() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        let base = spawn_mock_provider(move |_body| {
            calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            axum::response::Response::builder()
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"poToken":"cached"}"#))
                .unwrap()
        })
        .await;
        let p = provider_for(base);
        // Same binding twice -> one network call; a different binding -> two.
        let _ = p.fetch_typed("a").await.unwrap();
        let _ = p.fetch_typed("a").await.unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = p.fetch_typed("b").await.unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fetch_typed_reports_provider_status() {
        let base = spawn_mock_provider(|_body| {
            axum::response::Response::builder()
                .status(500)
                .body(axum::body::Body::from("boom"))
                .unwrap()
        })
        .await;
        let p = provider_for(base);
        assert!(matches!(
            p.fetch_typed("vid1").await,
            Err(PotError::ProviderStatus(500, _))
        ));
    }

    #[tokio::test]
    async fn fetch_typed_reports_invalid_response() {
        let base = spawn_mock_provider(|_body| {
            axum::response::Response::builder()
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"unexpected":true}"#))
                .unwrap()
        })
        .await;
        let p = provider_for(base);
        assert!(matches!(
            p.fetch_typed("vid1").await,
            Err(PotError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn probe_uses_mock_provider() {
        let base = spawn_mock_provider(|body| {
            assert!(body.contains("kizunalink-pot-probe"));
            axum::response::Response::builder()
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"poToken":"probe-ok"}"#))
                .unwrap()
        })
        .await;
        let p = provider_for(base);
        assert!(p.probe().await.is_ok());
    }

    #[tokio::test]
    async fn probe_fails_when_provider_down() {
        // Nothing is listening on this port.
        let p = provider_for("http://127.0.0.1:1".into());
        assert!(matches!(p.probe().await, Err(PotError::Unreachable(_))));
    }
}
