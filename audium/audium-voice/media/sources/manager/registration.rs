// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use crate::{
    common::HttpClientPool,
    media::sources::{
        amazonmusic::AmazonMusicSource,
        anghami::AnghamiSource,
        applemusic::AppleMusicSource,
        audiomack::AudiomackSource,
        audius::AudiusSource,
        bandcamp::BandcampSource,
        bilibili::BilibiliSource,
        bluesky::BlueskySource,
        deezer::DeezerSource,
        gaana::GaanaSource,
        googledrive::GoogleDriveSource,
        http::HttpSource,
        iheartradio::IheartradioSource,
        instagram::InstagramSource,
        jiosaavn::JioSaavnSource,
        lastfm::LastFMSource,
        local::LocalSource,
        mixcloud::MixcloudSource,
        netease::NeteaseSource,
        nicovideo::NicovideoSource,
        pandora::PandoraSource,
        pinterest::PinterestSource,
        pixeldrain::PixeldrainSource,
        plugin::BoxedSource,
        qobuz::QobuzSource,
        reddit::RedditSource,
        rss::RssSource,
        shazam::ShazamSource,
        songlink::SonglinkSource,
        soundcloud::SoundCloudSource,
        spotify::SpotifySource,
        telegram::TelegramSource,
        tidal::TidalSource,
        tiktok::TiktokSource,
        tumblr::TumblrSource,
        twitch::TwitchSource,
        twitter::TwitterSource,
        vimeo::VimeoSource,
        vkmusic::VkMusicSource,
        yandexmusic::YandexMusicSource,
        youtube::{YouTubeSource, YoutubeStreamContext, cipher::YouTubeCipherManager},
    },
};

/// Registrations for all audio sources.
pub fn register_all(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) -> (
    Option<Arc<YouTubeCipherManager>>,
    Option<Arc<YoutubeStreamContext>>,
) {
    let yt_ctx = register_core_sources(sources, config, http_pool);

    register_extra_sources(sources, config);

    yt_ctx
}

fn register_core_sources(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) -> (
    Option<Arc<YouTubeCipherManager>>,
    Option<Arc<YoutubeStreamContext>>,
) {
    let mut yt_ctx = (None, None);

    /// Registers a source that takes only `(config, http_client)` and is
    /// skipped when its config is absent or disabled.
    macro_rules! register_simple {
        ($cfg:expr, $name:literal, $ctor:path) => {
            if let Some(c) = $cfg.as_ref()
                && c.enabled
            {
                let proxy = c.proxy.clone();
                match $ctor(c.clone(), http_pool.get(proxy)) {
                    Ok(src) => {
                        tracing::info!("Loaded source: {}", $name);
                        sources.push(Box::new(src));
                    }
                    Err(e) => tracing::error!("{} source failed to initialize: {}", $name, e),
                }
            }
        };
    }

    macro_rules! register {
        ($enabled:expr, $name:literal, $proxy:expr, $ctor:expr) => {
            if $enabled {
                if let Some(p) = &$proxy {
                    tracing::info!(
                        "Loading {} with proxy: {}",
                        $name,
                        p.url.as_ref().unwrap_or(&"enabled".to_owned())
                    );
                }
                match $ctor {
                    Ok(src) => {
                        tracing::info!("Loaded source: {}", $name);
                        sources.push(Box::new(src));
                    }
                    Err(e) => {
                        tracing::error!("{} source failed to initialize: {}", $name, e);
                    }
                }
            }
        };
    }

    if config.sources.youtube.as_ref().is_some_and(|c| c.enabled) {
        let yt_proxy = config
            .sources
            .youtube
            .as_ref()
            .and_then(|c| c.proxy.clone());
        if let Some(p) = &yt_proxy {
            tracing::info!(
                "Loading YouTube with proxy: {}",
                p.url.as_ref().unwrap_or(&"enabled".to_owned())
            );
        }
        let yt_client = http_pool.get(yt_proxy.clone());
        let yt = YouTubeSource::new(config.sources.youtube.clone(), yt_client);
        yt_ctx = (Some(yt.cipher_manager()), Some(yt.stream_context()));
        sources.push(Box::new(yt));
        tracing::info!("Loaded source: YouTube");
    }

    let soundcloud_proxy = config
        .sources
        .soundcloud
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config
            .sources
            .soundcloud
            .as_ref()
            .is_some_and(|c| c.enabled),
        "SoundCloud",
        soundcloud_proxy,
        SoundCloudSource::new(
            config.sources.soundcloud.clone().unwrap(),
            http_pool.get(soundcloud_proxy.clone())
        )
    );

    register!(
        config.sources.spotify.as_ref().is_some_and(|c| c.enabled),
        "Spotify",
        None::<crate::config::sources::HttpProxyConfig>,
        SpotifySource::new(config.sources.spotify.clone(), http_pool.get(None))
    );

    let jiosaavn_proxy = config
        .sources
        .jiosaavn
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config.sources.jiosaavn.as_ref().is_some_and(|c| c.enabled),
        "JioSaavn",
        jiosaavn_proxy,
        JioSaavnSource::new(
            config.sources.jiosaavn.clone(),
            http_pool.get(jiosaavn_proxy.clone())
        )
    );

    register_deezer(sources, config, http_pool);

    let apple_proxy = config
        .sources
        .applemusic
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config
            .sources
            .applemusic
            .as_ref()
            .is_some_and(|c| c.enabled),
        "Apple Music",
        apple_proxy,
        AppleMusicSource::new(
            config.sources.applemusic.clone(),
            http_pool.get(apple_proxy.clone())
        )
    );

    let gaana_proxy = config.sources.gaana.as_ref().and_then(|c| c.proxy.clone());
    register!(
        config.sources.gaana.as_ref().is_some_and(|c| c.enabled),
        "Gaana",
        gaana_proxy,
        GaanaSource::new(
            config.sources.gaana.clone(),
            http_pool.get(gaana_proxy.clone())
        )
    );

    let tidal_proxy = config.sources.tidal.as_ref().and_then(|c| c.proxy.clone());
    register!(
        config.sources.tidal.as_ref().is_some_and(|c| c.enabled),
        "Tidal",
        tidal_proxy,
        TidalSource::new(
            config.sources.tidal.clone(),
            http_pool.get(tidal_proxy.clone())
        )
    );

    let audiomack_proxy = config
        .sources
        .audiomack
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config.sources.audiomack.as_ref().is_some_and(|c| c.enabled),
        "Audiomack",
        audiomack_proxy,
        AudiomackSource::new(
            config.sources.audiomack.clone(),
            http_pool.get(audiomack_proxy.clone())
        )
    );

    let pandora_proxy = config
        .sources
        .pandora
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config.sources.pandora.as_ref().is_some_and(|c| c.enabled),
        "Pandora",
        pandora_proxy,
        PandoraSource::new(
            config.sources.pandora.clone(),
            http_pool.get(pandora_proxy.clone())
        )
    );

    let qobuz_proxy = config.sources.qobuz.as_ref().and_then(|c| c.proxy.clone());
    if config.sources.qobuz.as_ref().is_some_and(|c| c.enabled) {
        let token_provided = config
            .sources
            .qobuz
            .as_ref()
            .and_then(|c| c.user_token.as_ref())
            .is_some_and(|t| !t.is_empty());

        if !token_provided {
            tracing::warn!("Qobuz user_token is missing; all playback will fall back to mirrors.");
        }

        register!(
            true,
            "Qobuz",
            qobuz_proxy,
            QobuzSource::new(config, http_pool.get(qobuz_proxy.clone()))
        );
    }

    let anghami_proxy = config
        .sources
        .anghami
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config.sources.anghami.as_ref().is_some_and(|c| c.enabled),
        "Anghami",
        anghami_proxy,
        AnghamiSource::new(config, http_pool.get(anghami_proxy.clone()))
    );

    let shazam_proxy = config.sources.shazam.as_ref().and_then(|c| c.proxy.clone());
    register!(
        config.sources.shazam.as_ref().is_some_and(|c| c.enabled),
        "Shazam",
        shazam_proxy,
        ShazamSource::new(config, http_pool.get(shazam_proxy.clone()))
    );

    let mixcloud_proxy = config
        .sources
        .mixcloud
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config.sources.mixcloud.as_ref().is_some_and(|c| c.enabled),
        "Mixcloud",
        mixcloud_proxy,
        MixcloudSource::new(
            config.sources.mixcloud.clone(),
            http_pool.get(mixcloud_proxy.clone())
        )
    );

    let bandcamp_proxy = config
        .sources
        .bandcamp
        .as_ref()
        .and_then(|c| c.proxy.clone());
    register!(
        config.sources.bandcamp.as_ref().is_some_and(|c| c.enabled),
        "Bandcamp",
        bandcamp_proxy,
        BandcampSource::new(
            config.sources.bandcamp.clone(),
            http_pool.get(bandcamp_proxy.clone())
        )
    );

    let reddit_proxy = config.sources.reddit.as_ref().and_then(|c| c.proxy.clone());
    register!(
        config.sources.reddit.as_ref().is_some_and(|c| c.enabled),
        "Reddit",
        reddit_proxy,
        RedditSource::new(
            config.sources.reddit.clone(),
            http_pool.get(reddit_proxy.clone())
        )
    );

    register!(
        config.sources.lastfm.as_ref().is_some_and(|c| c.enabled),
        "Last.fm",
        None::<crate::config::sources::HttpProxyConfig>,
        LastFMSource::new(config.sources.lastfm.clone(), http_pool.get(None))
    );

    let audius_proxy = config.sources.audius.as_ref().and_then(|c| c.proxy.clone());
    register!(
        config.sources.audius.as_ref().is_some_and(|c| c.enabled),
        "Audius",
        audius_proxy,
        AudiusSource::new(
            config.sources.audius.clone(),
            http_pool.get(audius_proxy.clone())
        )
    );

    register_yandex(sources, config, http_pool);
    register_vkmusic(sources, config, http_pool);
    register_netease(sources, config, http_pool);
    register_twitch(sources, config, http_pool);
    register_amazonmusic(sources, config, http_pool);

    if let Some(c) = config.sources.rss.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        match RssSource::new(c.clone(), http_pool.get(proxy.clone())) {
            Ok(src) => {
                tracing::info!("Loaded source: RSS");
                sources.push(Box::new(src));
            }
            Err(e) => tracing::error!("RSS source failed to initialize: {e}"),
        }
    }

    if let Some(c) = config.sources.iheartradio.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        match IheartradioSource::new(c.clone(), http_pool.get(proxy.clone())) {
            Ok(src) => {
                tracing::info!("Loaded source: iHeartRadio");
                sources.push(Box::new(src));
            }
            Err(e) => tracing::error!("iHeartRadio source failed to initialize: {e}"),
        }
    }

    register_simple!(config.sources.tumblr, "Tumblr", TumblrSource::new);
    register_simple!(config.sources.telegram, "Telegram", TelegramSource::new);

    register_simple!(
        config.sources.pixeldrain,
        "Pixeldrain",
        PixeldrainSource::new
    );

    register_simple!(config.sources.bilibili, "Bilibili", BilibiliSource::new);
    register_simple!(config.sources.nicovideo, "NicoVideo", NicovideoSource::new);
    register_simple!(config.sources.instagram, "Instagram", InstagramSource::new);
    register_simple!(config.sources.twitter, "Twitter", TwitterSource::new);
    register_simple!(config.sources.pinterest, "Pinterest", PinterestSource::new);
    register_simple!(
        config.sources.googledrive,
        "Google Drive",
        GoogleDriveSource::new
    );

    register_simple!(config.sources.songlink, "Songlink", SonglinkSource::new);
    register_simple!(config.sources.vimeo, "Vimeo", VimeoSource::new);

    if let Some(c) = config.sources.tiktok.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        match TiktokSource::new(c.clone(), http_pool.get(proxy.clone())) {
            Ok(src) => {
                tracing::info!("Loaded source: TikTok");
                sources.push(Box::new(src));
            }
            Err(e) => tracing::error!("TikTok source failed to initialize: {e}"),
        }
    }

    if let Some(c) = config.sources.bluesky.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        match BlueskySource::new(c.clone(), http_pool.get(proxy.clone())) {
            Ok(src) => {
                tracing::info!("Loaded source: Bluesky");
                sources.push(Box::new(src));
            }
            Err(e) => tracing::error!("Bluesky source failed to initialize: {e}"),
        }
    }

    if config.sources.http.as_ref().is_some_and(|c| c.enabled) {
        tracing::info!("Loaded source: http");
        sources.push(Box::new(HttpSource::new()));
    }

    yt_ctx
}

fn register_deezer(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) {
    let (token_provided, key_provided) = if let Some(c) = config.sources.deezer.as_ref() {
        let arls_provided = c
            .arls
            .as_ref()
            .is_some_and(|a| !a.is_empty() && a.iter().any(|s| !s.is_empty()));
        let key_provided = c
            .master_decryption_key
            .as_ref()
            .is_some_and(|k| !k.is_empty());
        (arls_provided, key_provided)
    } else {
        (false, false)
    };

    if config.sources.deezer.as_ref().is_some_and(|c| c.enabled) {
        if !token_provided || !key_provided {
            let mut missing = Vec::new();
            if !token_provided {
                missing.push("arls");
            }
            if !key_provided {
                missing.push("master_decryption_key");
            }
            tracing::warn!(
                "Deezer source is enabled but {} {} missing; it will be disabled.",
                missing.join(" and "),
                if missing.len() > 1 { "are" } else { "is" }
            );
        } else {
            let proxy = config.sources.deezer.as_ref().and_then(|c| c.proxy.clone());
            let source = DeezerSource::new(
                config.sources.deezer.clone().unwrap(),
                http_pool.get(proxy.clone()),
            );

            match source {
                Ok(src) => {
                    tracing::info!("Loaded source: Deezer");
                    sources.push(Box::new(src));
                }
                Err(e) => {
                    tracing::error!("Deezer source failed to initialize: {}", e);
                }
            }
        }
    }
}

fn register_yandex(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) {
    if let Some(c) = config.sources.yandexmusic.as_ref()
        && c.enabled
    {
        if c.access_token.is_none() {
            tracing::warn!(
                "Yandex Music source is enabled but the access_token is missing; it will be disabled."
            );
        } else {
            let proxy = c.proxy.clone();
            let source = YandexMusicSource::new(
                config.sources.yandexmusic.clone(),
                http_pool.get(proxy.clone()),
            );

            match source {
                Ok(src) => {
                    tracing::info!("Loaded source: Yandex Music");
                    sources.push(Box::new(src));
                }
                Err(e) => {
                    tracing::error!("Yandex Music source failed to initialize: {}", e);
                }
            }
        }
    }
}

fn register_vkmusic(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) {
    if let Some(c) = config.sources.vkmusic.as_ref()
        && c.enabled
    {
        if c.user_token.is_none() && c.user_cookie.is_none() {
            tracing::warn!(
                "VK Music source is enabled but neither user_token nor user_cookie is set; API calls will fail."
            );
        }

        let proxy = c.proxy.clone();
        match VkMusicSource::new(config.sources.vkmusic.clone(), http_pool.get(proxy.clone())) {
            Ok(src) => {
                tracing::info!("Loaded source: VK Music");
                sources.push(Box::new(src));
            }
            Err(e) => {
                tracing::error!("VK Music source failed to initialize: {}", e);
            }
        }
    }
}

fn register_netease(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) {
    if let Some(c) = config.sources.netease.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        match NeteaseSource::new(config.sources.netease.clone(), http_pool.get(proxy.clone())) {
            Ok(src) => {
                tracing::info!("Loaded source: Netease Music");
                sources.push(Box::new(src));
            }
            Err(e) => {
                tracing::error!("Netease Music source failed to initialize: {}", e);
            }
        }
    }
}

fn register_twitch(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) {
    if let Some(c) = config.sources.twitch.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        tracing::info!("Loaded source: Twitch");
        sources.push(Box::new(TwitchSource::new(c.clone(), http_pool.get(proxy))));
    }
}

fn register_amazonmusic(
    sources: &mut Vec<BoxedSource>,
    config: &crate::config::AppConfig,
    http_pool: &Arc<HttpClientPool>,
) {
    if let Some(c) = config.sources.amazonmusic.as_ref()
        && c.enabled
    {
        let proxy = c.proxy.clone();
        match AmazonMusicSource::new(c.clone(), http_pool.get(proxy)) {
            Ok(src) => {
                tracing::info!("Loaded source: Amazon Music");
                tracing::warn!(
                    "Amazon Music is enabled but requires an authenticated Amazon session \
                     (accessToken/csrf from music.amazon.com/config.json). Without one, \
                     searches and track lookups will return empty. Disable it unless you \
                     have such a session."
                );
                sources.push(Box::new(src));
            }
            Err(e) => {
                tracing::error!("Amazon Music source failed to initialize: {}", e);
            }
        }
    }
}

fn register_extra_sources(sources: &mut Vec<BoxedSource>, config: &crate::config::AppConfig) {
    if let Some(c) = config.sources.local.as_ref()
        && c.enabled
    {
        if let Some(media_dir) = c.media_dir.clone() {
            tracing::info!("Loaded source: local (media_dir={media_dir})");
            sources.push(Box::new(LocalSource::new(Some(media_dir))));
        } else {
            tracing::warn!(
                "Local source is enabled but sources.local.media_dir is unset; refusing unscoped filesystem access"
            );
        }
    }
}
