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
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde_json::Value;

/// `Arc`-shared PoToken provider.
pub type SharedPotProvider = Arc<YouTubePotProvider>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// bgutil tokens are valid for hours; cache comfortably below that.
const CACHE_TTL: Duration = Duration::from_secs(4 * 60 * 60);

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
        Self {
            http,
            base_url: Some(base_url.trim_end_matches('/').to_string()),
            bypass_cache,
            cache: Mutex::new(None),
        }
    }

    /// Mint (or reuse) a PoToken bound to `video_id`.
    ///
    /// GVS format URLs from the WEB/ANDROID/IOS clients are bound to the video id
    /// (the `content` binding), so the token must be minted per video. Returns
    /// `None` when the provider is disabled or the request fails; the caller then
    /// sends the player request without a token.
    pub async fn fetch(&self, video_id: &str) -> Option<String> {
        let base = self.base_url.as_deref()?;
        if video_id.is_empty() {
            return None;
        }

        if !self.bypass_cache
            && let Some(token) = self.cached(video_id)
        {
            return Some(token);
        }

        let body = serde_json::json!({
            "content_binding": video_id,
            "bypass_cache": self.bypass_cache,
        });
        let url = format!("{}/get_pot", base);

        let response = match self
            .http
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("PoToken provider request failed: {}", e);
                return None;
            }
        };

        if !response.status().is_success() {
            tracing::warn!(
                "PoToken provider returned {} ({})",
                response.status(),
                response.text().await.unwrap_or_default()
            );
            return None;
        }

        let json: Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("PoToken provider returned invalid JSON: {}", e);
                return None;
            }
        };

        let token = parse_token(&json)?;
        tracing::debug!("PoToken acquired for video {}…", prefix(video_id));
        *self.cache.lock().unwrap() = Some(CachedToken {
            token: token.clone(),
            binding: video_id.to_string(),
            expires: Instant::now() + CACHE_TTL,
        });
        Some(token)
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
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(rt.block_on(p.fetch("vd")), None);
    }
}
