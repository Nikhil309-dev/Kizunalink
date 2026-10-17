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
}

impl CachedClientId {
    fn is_stale(&self) -> bool {
        match self.last_updated {
            None => true,
            Some(t) => t.elapsed() > CLIENT_ID_REFRESH_INTERVAL,
        }
    }
}

impl SoundCloudTokenTracker {
    pub fn new(client: Arc<reqwest::Client>, config: &crate::config::SoundCloudConfig) -> Self {
        let configured = config.client_id.clone().filter(|id| is_valid_client_id(id));
        let cached = CachedClientId {
            last_updated: configured.as_ref().map(|_| Instant::now()),
            value: configured,
        };

        Self {
            client,
            client_id: Arc::new(RwLock::new(cached)),
        }
    }

    pub async fn get_client_id(&self) -> Option<String> {
        {
            let guard = self.client_id.read().await;
            if !guard.is_stale()
                && let Some(id) = &guard.value
            {
                return Some(id.clone());
            }
        }

        self.refresh_client_id().await
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

    pub async fn invalidate(&self) {
        let mut guard = self.client_id.write().await;
        guard.last_updated = None;
    }

    async fn store_client_id(&self, id: String) {
        let mut guard = self.client_id.write().await;
        guard.value = Some(id);
        guard.last_updated = Some(Instant::now());
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
    use super::{extract_client_id, is_valid_client_id, script_src_re};

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
}
