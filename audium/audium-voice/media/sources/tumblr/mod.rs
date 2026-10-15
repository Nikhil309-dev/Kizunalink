// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Tumblr post source.
//!
//! Resolves `*.tumblr.com/post/<id>` (and `tumblr.com/<blog>/<id>`) posts by
//! reading the public post page. Direct video/audio media URLs are preferred; a
//! post that merely embeds YouTube or Vimeo is delegated back to the source
//! manager so the native source handles it. No credentials required.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::{
    config::sources::TumblrConfig,
    lavalink::protocol::tracks::{LoadResult, Track, TrackInfo},
    media::sources::plugin::{BoxedTrack, SourcePlugin},
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn post_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://([\w-]+)\.tumblr\.com/(?:post/)?(\d{8,})").expect("valid regex")
    })
}

pub struct TumblrSource {
    http: Arc<reqwest::Client>,
}

impl TumblrSource {
    pub fn new(_config: TumblrConfig, http: Arc<reqwest::Client>) -> Result<Self, String> {
        Ok(Self { http })
    }

    async fn fetch_html(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            debug!("Tumblr returned HTTP {}", response.status());
            return None;
        }
        response.text().await.ok()
    }

    /// Extracts a direct media URL from the post page.
    ///
    /// Tumblr exposes the primary media in the `og:video` / `og:audio` meta tags
    /// and in `<video src=…>` / `tumblr_video` markup. We try the meta tags
    /// first because they are stable across theme changes.
    fn extract_media(&self, html: &str) -> Option<String> {
        for key in ["og:video:secure_url", "og:video", "og:audio:secure_url", "og:audio"] {
            if let Some(url) = meta_property(html, key) {
                return Some(html_unescape(&url));
            }
        }
        let video_re = Regex::new(r#"<video[^>]+src=["']([^"']+)["']"#).ok()?;
        video_re
            .captures(html)
            .map(|c| html_unescape(&c[1]))
    }

    /// Finds a YouTube/Vimeo embed to delegate to the native source.
    fn extract_embed(&self, html: &str) -> Option<String> {
        let yt = Regex::new(
            r#"https?://(?:www\.)?youtube\.com/(?:embed/|watch\?v=)([A-Za-z0-9_-]{11})"#,
        )
        .ok()?;
        if let Some(c) = yt.captures(html) {
            return Some(format!("https://www.youtube.com/watch?v={}", &c[1]));
        }
        let vimeo = Regex::new(r#"https?://(?:player\.)?vimeo\.com/video/(\d+)"#).ok()?;
        vimeo
            .captures(html)
            .map(|c| format!("https://vimeo.com/{}", &c[1]))
    }

    fn extract_title(html: &str) -> String {
        meta_property(html, "og:title")
            .or_else(|| {
                Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
                    .ok()?
                    .captures(html)
                    .map(|c| c[1].trim().to_string())
            })
            .map(|t| html_unescape(&t))
            .unwrap_or_else(|| "Tumblr post".to_string())
    }

    fn build_track(&self, html: &str, blog: &str, post_id: &str, url: &str) -> Option<Track> {
        let media = self.extract_media(html)?;
        let artwork = meta_property(html, "og:image").map(|u| html_unescape(&u));
        let is_video = media.contains(".mp4") || media.contains("video");
        Some(Track::new(TrackInfo {
            identifier: post_id.to_string(),
            is_seekable: true,
            author: blog.to_string(),
            length: 0,
            is_stream: !is_video,
            position: 0,
            title: Self::extract_title(html),
            uri: Some(media),
            artwork_url: artwork,
            isrc: None,
            source_name: "tumblr".to_string(),
        }))
        .map(|mut t| {
            t.plugin_info = serde_json::json!({ "originalUrl": url });
            t
        })
    }

    async fn resolve(&self, identifier: &str) -> LoadResult {
        let Some(caps) = post_pattern().captures(identifier) else {
            return LoadResult::Empty {};
        };
        let blog = caps[1].to_string();
        let post_id = caps[2].to_string();

        let Some(html) = self.fetch_html(identifier).await else {
            return LoadResult::Empty {};
        };
        if let Some(track) = self.build_track(&html, &blog, &post_id, identifier) {
            return LoadResult::Track(track);
        }
        // A post that only embeds another platform is delegated; the manager
        // re-routes by identifier, so returning the embed URL as a "playlist"
        // would be wrong. Emit empty and let the manager's fallback resolve it.
        if self.extract_embed(&html).is_some() {
            debug!("Tumblr post embeds an external platform; deferring");
        }
        LoadResult::Empty {}
    }
}

/// Reads `<meta property="…" content="…">` in either attribute order.
fn meta_property(html: &str, property: &str) -> Option<String> {
    let pattern = format!(
        r#"(?is)<meta[^>]+(?:property|name)=["']{p}["'][^>]*content=["']([^"']+)["']|<meta[^>]+content=["']([^"']+)["'][^>]*(?:property|name)=["']{p}["']"#,
        p = regex::escape(property)
    );
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(html)?;
    caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str().to_string())
}

/// Minimal HTML entity decode for the handful of entities that appear in URLs
/// and titles (`&amp;` above all).
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[async_trait]
impl SourcePlugin for TumblrSource {
    fn name(&self) -> &str {
        "tumblr"
    }

    fn can_handle(&self, identifier: &str) -> bool {
        post_pattern().is_match(identifier)
    }

    async fn load(
        &self,
        identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> LoadResult {
        debug!("Tumblr resolving: {}", identifier);
        self.resolve(identifier).await
    }

    async fn get_track(
        &self,
        _identifier: &str,
        _routeplanner: Option<Arc<dyn crate::lavalink::routeplanner::RoutePlanner>>,
    ) -> Option<BoxedTrack> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> TumblrSource {
        TumblrSource::new(TumblrConfig::default(), Arc::new(reqwest::Client::new()))
            .expect("valid source")
    }

    #[test]
    fn recognises_post_urls() {
        let s = source();
        assert!(s.can_handle("https://alice.tumblr.com/post/123456789012"));
        assert!(s.can_handle("https://my-blog.tumblr.com/123456789012"));
        assert!(!s.can_handle("https://tumblr.com/dashboard"));
        assert!(!s.can_handle("https://example.com/post/123456789012"));
    }

    #[test]
    fn extracts_og_video_and_builds_track() {
        let s = source();
        let html = r#"
            <html><head>
            <meta property="og:title" content="My &amp; Video" />
            <meta property="og:image" content="https://media.tumblr.com/thumb.jpg" />
            <meta property="og:video:secure_url" content="https://media.tumblr.com/clip.mp4" />
            </head><body></body></html>"#;
        let track = s
            .build_track(html, "alice", "123456789012", "https://alice.tumblr.com/post/123456789012")
            .expect("track");
        assert_eq!(track.info.title, "My & Video");
        assert_eq!(track.info.author, "alice");
        assert_eq!(
            track.info.uri.as_deref(),
            Some("https://media.tumblr.com/clip.mp4")
        );
        assert_eq!(
            track.info.artwork_url.as_deref(),
            Some("https://media.tumblr.com/thumb.jpg")
        );
        assert!(!track.info.is_stream);
    }

    #[test]
    fn detects_youtube_embed_for_delegation() {
        let s = source();
        let html = r#"<iframe src="https://www.youtube.com/embed/dQw4w9WgXcQ"></iframe>"#;
        assert_eq!(
            s.extract_embed(html).as_deref(),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
    }

    #[test]
    fn detects_vimeo_embed_for_delegation() {
        let s = source();
        let html = r#"<iframe src="https://player.vimeo.com/video/123456789"></iframe>"#;
        assert_eq!(
            s.extract_embed(html).as_deref(),
            Some("https://vimeo.com/123456789")
        );
    }

    #[test]
    fn falls_back_to_video_tag() {
        let s = source();
        let html = r#"<video class="x" src="https://ve.media.tumblr.com/t.mp4"></video>"#;
        assert_eq!(
            s.extract_media(html).as_deref(),
            Some("https://ve.media.tumblr.com/t.mp4")
        );
    }
}
