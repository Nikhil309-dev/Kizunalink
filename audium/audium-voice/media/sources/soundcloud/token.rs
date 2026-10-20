// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use regex::Regex;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use crate::common::types::SharedRw;

const SOUNDCLOUD_URL: &str = "https://soundcloud.com";
const CLIENT_ID_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);
/// How long to serve the last known-good `client_id` before re-attempting a
/// refresh that just failed.
const CLIENT_ID_REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(60);
const CLIENT_ID_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

fn asset_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:https?:)?//[^"'\s<>]+(?:sndcdn\.com|soundcloud\.com)[^"'\s<>]*\.js(?:\?[^"'\s<>]*)?"#)
            .expect("valid regex")
    })
}

fn script_src_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)<script[^>]+src\s*=\s*["']([^"']+)["']"#).expect("valid regex")
    })
}

fn client_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(?:client[_-]?id|clientId)\s*[:=]\s*["']?([a-zA-Z0-9_-]{16,})"#)
            .expect("valid regex")
    })
}

/// SoundCloud now ships the app credential inside a hydration JSON blob
/// (`{"hydratable":"apiClient","data":{"id":"<32 hex>"}}`) rather than as a
/// literal `client_id=` assignment, so the assignment regex above misses it.
fn api_client_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)"data"\s*:\s*\{\s*"id"\s*:\s*"([A-Za-z0-9_-]{16,})""#)
            .expect("valid regex")
    })
}

/// A SoundCloud `client_id` must be a well-formed credential before it is used
/// to build an API request; this rejects the `MISSING_CLIENT_ID` placeholder and
/// any other value that cannot be a real key.
fn is_valid_client_id(id: &str) -> bool {
    id.len() >= 16
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && id.to_ascii_uppercase().replace('-', "_") != "MISSING_CLIENT_ID"
}

/// Extract the first plausible `client_id` from page or asset JavaScript,
/// accepting both the assignment form and the hydration-blob form.
fn extract_client_id(text: &str) -> Option<String> {
    for re in [client_id_re(), api_client_id_re()] {
        for caps in re.captures_iter(text) {
            if let Some(m) = caps.get(1) {
                let id = m.as_str();
                if is_valid_client_id(id) {
                    return Some(id.to_owned());
                }
            }
        }
    }
    None
}

pub struct SoundCloudTokenTracker {
    client: Arc<reqwest::Client>,
    client_id: SharedRw<CachedClientId>,
}

struct CachedClientId {
    value: Option<String>,
    last_updated: Option<Instant>,
    /// Set when a refresh attempt failed. While this is present the cached
    /// `value` is still served, so a transient scrape failure (blocked egress,
    /// markup change, timeout) does not take search *and* playback down until
    /// the process is restarted.
    last_failed: Option<Instant>,
}

impl CachedClientId {
    fn is_stale(&self) -> bool {
        match self.last_updated {
            None => true,
            Some(t) => t.elapsed() > CLIENT_ID_REFRESH_INTERVAL,
        }
    }

    /// True while a recent refresh failure is still within the retry window, so
    /// callers should keep using `value` instead of hammering the scrape again.
    fn in_failed_retry_window(&self) -> bool {
        self.last_failed
            .is_some_and(|t| t.elapsed() < CLIENT_ID_REFRESH_RETRY_INTERVAL)
    }
}

impl SoundCloudTokenTracker {
    pub fn new(client: Arc<reqwest::Client>, config: &crate::config::SoundCloudConfig) -> Self {
        let configured = config.client_id.clone().filter(|id| is_valid_client_id(id));
        let cached = CachedClientId {
            last_updated: configured.as_ref().map(|_| Instant::now()),
            value: configured,
            last_failed: None,
        };

        Self {
            client,
            client_id: Arc::new(RwLock::new(cached)),
        }
    }

    /// Return a usable `client_id`, refreshing when the cached one has aged out.
    ///
    /// A failed refresh must not discard a still-working credential: the previous
    /// behaviour returned `None`, which made every SoundCloud search and every
    /// SoundCloud track resolve fail until the process was restarted (the
    /// configured id was only trusted for `CLIENT_ID_REFRESH_INTERVAL` from
    /// startup). Now the last known-good value is served while the scrape is
    /// retried, and `None` is only returned when there is genuinely no id.
    pub async fn get_client_id(&self) -> Option<String> {
        let (cached_value, should_refresh) = {
            let guard = self.client_id.read().await;
            let retry_blocked = guard.in_failed_retry_window();
            (guard.value.clone(), guard.is_stale() && !retry_blocked)
        };

        if !should_refresh {
            return cached_value;
        }

        match self.refresh_client_id().await {
            Some(id) => Some(id),
            None => {
                self.mark_refresh_failed().await;
                if cached_value.is_some() {
                    warn!(
                        "SoundCloud: client_id refresh failed; continuing with the cached \
                         credential and retrying in {:?}",
                        CLIENT_ID_REFRESH_RETRY_INTERVAL
                    );
                }
                cached_value
            }
        }
    }

    pub async fn refresh_client_id(&self) -> Option<String> {
        debug!("Refreshing SoundCloud client_id...");
        trace!("SoundCloud: Fetching client_id from soundcloud.com...");

        let response = match self
            .client
            .get(SOUNDCLOUD_URL)
            .timeout(CLIENT_ID_REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                error!("SoundCloud: Failed to fetch main page: {}", e);
                return None;
            }
        };
        let status = response.status();
        let html = match crate::common::http::read_text_capped(response).await {
            Ok(t) => t,
            Err(e) => {
                error!("SoundCloud: Failed to read main page: {}", e);
                return None;
            }
        };
        if !status.is_success() || html.is_empty() {
            warn!(
                "SoundCloud: homepage returned HTTP {} with {} bytes; client_id discovery may be blocked",
                status,
                html.len()
            );
            return None;
        }
        let html = html.replace("&amp;", "&");

        if let Some(id) = extract_client_id(&html) {
            trace!("SoundCloud: Found client_id in main page");
            self.store_client_id(id.clone()).await;
            info!("Successfully refreshed SoundCloud client_id");
            return Some(id);
        }

        let mut asset_urls = HashSet::new();
        for caps in script_src_re().captures_iter(&html) {
            if let Some(src) = caps.get(1).map(|m| m.as_str()) {
                let url = if src.starts_with("//") {
                    format!("https:{src}")
                } else if src.starts_with('/') {
                    format!("https://soundcloud.com{src}")
                } else if src.starts_with("http") {
                    src.to_owned()
                } else {
                    continue;
                };
                asset_urls.insert(url);
            }
        }
        for m in asset_re().find_iter(&html) {
            let mut url = m.as_str().to_owned();
            if url.starts_with("//") {
                url = format!("https:{url}");
            }
            asset_urls.insert(url);
        }
        let asset_urls: Vec<String> = asset_urls.into_iter().collect();

        if asset_urls.is_empty() {
            warn!("SoundCloud: No asset JS URLs found in main page");
            return None;
        }

        trace!(
            "SoundCloud: Found {} asset URLs, probing for client_id",
            asset_urls.len()
        );

        for url in asset_urls.iter().rev().take(9) {
            let js = match self
                .client
                .get(url)
                .timeout(CLIENT_ID_REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(r) => match crate::common::http::read_text_capped(r).await {
                    Ok(t) => t,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };

            if let Some(id) = extract_client_id(&js) {
                trace!("SoundCloud: Found client_id in asset {url}");
                self.store_client_id(id.clone()).await;
                info!("Successfully refreshed SoundCloud client_id");
                return Some(id);
            }
        }

        warn!("SoundCloud: client_id not found in any asset scripts");
        None
    }

    /// Record a 401 from the API: the credential is known-bad, so drop it and
    /// let the next `get_client_id` scrape a fresh one.
    pub async fn invalidate(&self) {
        let mut guard = self.client_id.write().await;
        guard.last_updated = None;
        guard.value = None;
        guard.last_failed = None;
    }

    /// Record that a refresh attempt failed, without discarding the cached value.
    async fn mark_refresh_failed(&self) {
        let mut guard = self.client_id.write().await;
        guard.last_failed = Some(Instant::now());
    }

    async fn store_client_id(&self, id: String) {
        let mut guard = self.client_id.write().await;
        guard.value = Some(id);
        guard.last_updated = Some(Instant::now());
        guard.last_failed = None;
    }

    pub fn init(self: Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            this.get_client_id().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CachedClientId, CLIENT_ID_REFRESH_INTERVAL, CLIENT_ID_REFRESH_RETRY_INTERVAL,
        extract_client_id, is_valid_client_id, script_src_re,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn script_sources_are_case_insensitive() {
        let html =
            r#"<SCRIPT SRC="/assets/app.js"></SCRIPT><script src='/assets/other.js'></script>"#;
        let sources: Vec<_> = script_src_re()
            .captures_iter(html)
            .filter_map(|caps| caps.get(1).map(|m| m.as_str()))
            .collect();
        assert_eq!(sources, ["/assets/app.js", "/assets/other.js"]);
    }

    #[test]
    fn extracts_client_id_from_hydration_blob() {
        let html = r#"{"hydratable":"apiClient","data":{"id":"QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k","isExpiring":true}}"#;
        assert_eq!(
            extract_client_id(html).as_deref(),
            Some("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k")
        );
    }

    #[test]
    fn extracts_client_id_from_assignment() {
        let js = r#"var x = client_id:"AbCdEfGhIjKlMnOpQrStUvWx";"#;
        assert_eq!(
            extract_client_id(js).as_deref(),
            Some("AbCdEfGhIjKlMnOpQrStUvWx")
        );
    }

    #[test]
    fn rejects_placeholder_and_short_ids() {
        assert!(!is_valid_client_id("MISSING_CLIENT_ID"));
        assert!(!is_valid_client_id("MISSING-CLIENT-ID"));
        assert!(!is_valid_client_id("short"));
        assert!(!is_valid_client_id("has spaces in it here"));
        assert!(is_valid_client_id("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k"));

        let html = r#"{"data":{"id":"MISSING_CLIENT_ID"}}"#;
        assert_eq!(extract_client_id(html), None);
    }

    fn fresh(value: &str) -> CachedClientId {
        CachedClientId {
            value: Some(value.to_string()),
            last_updated: Some(Instant::now()),
            last_failed: None,
        }
    }

    #[test]
    fn a_configured_id_is_fresh_at_startup() {
        assert!(!fresh("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k").is_stale());
    }

    #[test]
    fn an_id_past_the_refresh_interval_is_stale() {
        let mut cache = fresh("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k");
        cache.last_updated = Some(Instant::now() - CLIENT_ID_REFRESH_INTERVAL - Duration::from_secs(1));
        assert!(cache.is_stale());
    }

    /// Regression: a stale id whose refresh just failed must keep being served,
    /// otherwise every SoundCloud search and resolve returns empty until the
    /// process is restarted (the original ~1h outage).
    #[test]
    fn a_recent_failure_suppresses_the_refresh_attempt() {
        let mut cache = fresh("QhVEWrrJrgluuy");
        cache.last_updated = Some(Instant::now() - CLIENT_ID_REFRESH_INTERVAL - Duration::from_secs(1));
        cache.last_failed = Some(Instant::now());

        assert!(cache.is_stale(), "the id is past its refresh interval");
        assert!(
            cache.in_failed_retry_window(),
            "a just-recorded failure must block an immediate re-scrape"
        );
    }

    #[test]
    fn the_retry_window_expires_so_refresh_is_attempted_again() {
        let mut cache = fresh("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k");
        cache.last_failed =
            Some(Instant::now() - CLIENT_ID_REFRESH_RETRY_INTERVAL - Duration::from_secs(1));

        assert!(
            !cache.in_failed_retry_window(),
            "after the retry window the scrape must be retried"
        );
    }

    #[test]
    fn a_successful_store_clears_the_failure_marker() {
        let mut cache = fresh("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k");
        cache.last_failed = Some(Instant::now());

        cache.value = Some("FreshClientId1234567".to_string());
        cache.last_updated = Some(Instant::now());
        cache.last_failed = None;

        assert!(!cache.is_stale());
        assert!(!cache.in_failed_retry_window());
    }

    /// Core regression: a stale id whose refresh just failed must still be
    /// returned. Before the fix `get_client_id` returned `None`, which made every
    /// SoundCloud search and resolve fail until the process was restarted.
    ///
    /// The recorded failure short-circuits the refresh, so this never touches the
    /// network.
    #[tokio::test]
    async fn a_stale_id_with_a_recent_failure_is_still_served() {
        let config = crate::config::SoundCloudConfig {
            client_id: Some("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k".to_string()),
            ..Default::default()
        };
        let tracker = super::SoundCloudTokenTracker::new(
            std::sync::Arc::new(reqwest::Client::new()),
            &config,
        );

        {
            let mut guard = tracker.client_id.write().await;
            guard.last_updated = Some(Instant::now() - CLIENT_ID_REFRESH_INTERVAL * 2);
            guard.last_failed = Some(Instant::now());
        }

        assert_eq!(
            tracker.get_client_id().await.as_deref(),
            Some("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k"),
            "a failed refresh must not discard the cached credential"
        );
    }

    /// A fresh cached value is served without touching the network at all.
    #[tokio::test]
    async fn a_fresh_id_is_served_without_a_refresh() {
        let config = crate::config::SoundCloudConfig {
            client_id: Some("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k".to_string()),
            ..Default::default()
        };
        let tracker = super::SoundCloudTokenTracker::new(
            std::sync::Arc::new(reqwest::Client::new()),
            &config,
        );

        assert_eq!(
            tracker.get_client_id().await.as_deref(),
            Some("QhVEWrrJKvW4uSU5u5EoI0FTXYUGzy5k")
        );
    }

    /// A stored id is served back and clears any recorded failure, which is what
    /// ends an outage once the scrape starts working again.
    #[tokio::test]
    async fn a_stored_id_is_served_and_clears_the_failure_marker() {
        let config = crate::config::SoundCloudConfig::default();
        let tracker = super::SoundCloudTokenTracker::new(
            std::sync::Arc::new(reqwest::Client::new()),
            &config,
        );

        tracker
            .store_client_id("StoredClientId123456789".to_string())
            .await;

        {
            let guard = tracker.client_id.read().await;
            assert_eq!(guard.value.as_deref(), Some("StoredClientId123456789"));
            assert!(guard.last_failed.is_none());
        }

        assert_eq!(
            tracker.get_client_id().await.as_deref(),
            Some("StoredClientId123456789")
        );
    }
}
