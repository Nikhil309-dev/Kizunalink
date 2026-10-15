// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::{sync::Arc, time::Duration};

use dashmap::DashMap;
use reqwest::{Client, Proxy};
use tracing::warn;

use crate::{
    common::utils::{default_user_agent, redact_url},
    config::sources::HttpProxyConfig,
};

/// A pool of `reqwest::Client` instances shared across sources.
pub struct HttpClientPool {
    clients: DashMap<Option<HttpProxyConfig>, Arc<Client>>,
}

impl HttpClientPool {
    pub fn new() -> Self {
        Self {
            clients: DashMap::new(),
        }
    }

    /// Get a shared client for the given proxy configuration.
    pub fn get(&self, proxy: Option<HttpProxyConfig>) -> Arc<Client> {
        self.clients
            .entry(proxy.clone())
            .or_insert_with(|| Arc::new(self.create_client(proxy)))
            .clone()
    }

    fn create_client(&self, proxy: Option<HttpProxyConfig>) -> Client {
        let mut builder = Client::builder()
            .user_agent(default_user_agent())
            .gzip(true)
            .deflate(true)
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(70));

        if let Some(url) = proxy.as_ref().and_then(|config| config.url.as_ref()) {
            match Proxy::all(url) {
                Ok(mut proxy_obj) => {
                    if let Some((u, p)) = proxy
                        .as_ref()
                        .and_then(|c| c.username.as_ref().zip(c.password.as_ref()))
                    {
                        proxy_obj = proxy_obj.basic_auth(u, p);
                    }
                    builder = builder.proxy(proxy_obj);
                }
                Err(e) => {
                    // B18: Proxy config failure is logged loudly. If privacy is critical,
                    // users should validate their proxy config at startup. We do NOT make
                    // this fatal because some deployments may intentionally not need proxy.
                    warn!(
                        "HttpClientPool: failed to parse proxy URL '{}': {} — proxy will be ignored",
                        redact_url(url),
                        e
                    );
                }
            }
        }

        match builder.build() {
            Ok(client) => client,
            Err(e) => {
                warn!(
                    "HttpClientPool: failed to build client ({}), falling back to default",
                    e
                );
                Client::new()
            }
        }
    }
}

impl Default for HttpClientPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Upper bound for a source's metadata/HTML response body.
///
/// Source resolution parses JSON APIs and embed pages, never media payloads;
/// those go through the streaming readers. Without a cap a hostile or
/// misbehaving upstream could make `.text()`/`.json()` buffer an unbounded
/// body inside the 15s timeout window and exhaust memory.
pub const MAX_SOURCE_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Reads a response body as bytes, refusing to buffer more than
/// [`MAX_SOURCE_BODY_BYTES`]. Returns an error for over-sized bodies so callers
/// fall through to their normal "resolution failed" path.
pub async fn read_body_capped(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_SOURCE_BODY_BYTES as u64)
    {
        return Err("response body exceeds source size cap".to_string());
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_SOURCE_BODY_BYTES {
            return Err("response body exceeds source size cap".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads a capped response body and parses it as JSON.
pub async fn read_json_capped<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let bytes = read_body_capped(response).await?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// Reads a capped response body and decodes it as UTF-8 text.
pub async fn read_text_capped(response: reqwest::Response) -> Result<String, String> {
    let bytes = read_body_capped(response).await?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(url: &str) -> HttpProxyConfig {
        HttpProxyConfig {
            url: Some(url.to_string()),
            username: None,
            password: None,
        }
    }

    #[test]
    fn proxy_configs_are_cached_keyed_by_value() {
        let pool = HttpClientPool::new();
        let a = pool.get(Some(proxy("http://127.0.0.1:8080")));
        let b = pool.get(Some(proxy("http://127.0.0.1:8080")));
        // Same proxy config reuses the same pooled client.
        assert!(Arc::ptr_eq(&a, &b));

        let c = pool.get(None);
        // A direct client is distinct from a proxied one.
        assert!(!Arc::ptr_eq(&a, &c));
    }

    #[test]
    fn direct_and_proxied_clients_do_not_collide() {
        let pool = HttpClientPool::new();
        let direct1 = pool.get(None);
        let direct2 = pool.get(None);
        assert!(Arc::ptr_eq(&direct1, &direct2));
        assert_eq!(pool.clients.len(), 1);
        let _ = pool.get(Some(proxy("http://127.0.0.1:8081")));
        assert_eq!(pool.clients.len(), 2);
    }

    /// Spawns a one-shot HTTP server that replies with `body` and returns its URL.
    fn serve_once(body: Vec<u8>, content_length: Option<usize>) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let mut headers = String::from("HTTP/1.1 200 OK\r\n");
                if let Some(len) = content_length {
                    headers.push_str(&format!("Content-Length: {len}\r\n"));
                }
                headers.push_str("\r\n");
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn capped_reader_accepts_a_body_within_the_limit() {
        let url = serve_once(b"hello world".to_vec(), Some(11));
        let resp = reqwest::get(&url).await.expect("request");
        let body = read_body_capped(resp).await.expect("within cap");
        assert_eq!(body, b"hello world");
    }

    #[tokio::test]
    async fn capped_reader_rejects_an_oversized_declared_length() {
        let url = serve_once(Vec::new(), Some(MAX_SOURCE_BODY_BYTES + 1));
        let resp = reqwest::get(&url).await.expect("request");
        assert!(read_body_capped(resp).await.is_err());
    }

    #[tokio::test]
    async fn capped_reader_rejects_an_oversized_streamed_body() {
        // No Content-Length: the cap must still hold while streaming.
        let url = serve_once(vec![b'x'; MAX_SOURCE_BODY_BYTES + 64], None);
        let resp = reqwest::get(&url).await.expect("request");
        assert!(read_body_capped(resp).await.is_err());
    }

    #[tokio::test]
    async fn capped_json_and_text_helpers_round_trip() {
        let url = serve_once(br#"{"a":1}"#.to_vec(), None);
        let resp = reqwest::get(&url).await.expect("request");
        let value: serde_json::Value = read_json_capped(resp).await.expect("json");
        assert_eq!(value["a"], 1);

        let url = serve_once(b"plain".to_vec(), None);
        let resp = reqwest::get(&url).await.expect("request");
        assert_eq!(read_text_capped(resp).await.expect("text"), "plain");
    }
}
