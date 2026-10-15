// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

pub mod amazonmusic;
pub mod anghami;
pub mod applemusic;
pub mod audiomack;
pub mod audius;
pub mod bandcamp;
pub mod bilibili;
pub mod bluesky;
pub mod deezer;
pub mod flowery;
pub mod gaana;
pub mod google_tts;
pub mod googledrive;
pub mod http;
pub mod iheartradio;
pub mod instagram;
pub mod jiosaavn;
pub mod kwai;
pub mod lastfm;
pub mod lazypytts;
pub mod local;
pub mod mixcloud;
pub mod netease;
pub mod newgrounds;
pub mod nicovideo;
pub mod pandora;
pub mod pinterest;
pub mod qobuz;
pub mod reddit;
pub mod rss;
pub mod shazam;
pub mod songlink;
pub mod soundcloud;
pub mod spotify;
pub mod telegram;
pub mod tidal;
pub mod tiktok;
pub mod tumblr;
pub mod twitch;
pub mod twitter;
pub mod vimeo;
pub mod vkmusic;
pub mod yandexmusic;
pub mod youtube;

pub use amazonmusic::*;
pub use anghami::*;
pub use applemusic::*;
pub use audiomack::*;
pub use audius::*;
pub use bandcamp::*;
pub use bilibili::*;
pub use bluesky::*;
pub use deezer::*;
pub use flowery::*;
pub use gaana::*;
pub use google_tts::*;
pub use googledrive::*;
pub use http::*;
pub use iheartradio::*;
pub use instagram::*;
pub use jiosaavn::*;
pub use kwai::*;
pub use lastfm::*;
pub use lazypytts::*;
pub use local::*;
pub use mixcloud::*;
pub use netease::*;
pub use newgrounds::*;
pub use nicovideo::*;
pub use pandora::*;
pub use pinterest::*;
pub use qobuz::*;
pub use reddit::*;
pub use rss::*;
pub use shazam::*;
pub use songlink::*;
pub use soundcloud::*;
pub use spotify::*;
pub use telegram::*;
pub use tidal::*;
pub use tiktok::*;
pub use tumblr::*;
pub use twitch::*;
pub use twitter::*;
pub use vimeo::*;
pub use vkmusic::*;
pub use yandexmusic::*;
pub use youtube::*;

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq, Hash)]
pub struct HttpProxyConfig {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
pub struct SourcesConfig {
    pub youtube: Option<YouTubeConfig>,
    pub spotify: Option<SpotifyConfig>,
    pub amazonmusic: Option<AmazonMusicConfig>,
    pub http: Option<HttpSourceConfig>,
    pub local: Option<LocalSourceConfig>,
    pub jiosaavn: Option<JioSaavnConfig>,
    pub deezer: Option<DeezerConfig>,
    pub applemusic: Option<AppleMusicConfig>,
    pub gaana: Option<GaanaConfig>,
    pub tidal: Option<TidalConfig>,
    pub soundcloud: Option<SoundCloudConfig>,
    pub audiomack: Option<AudiomackConfig>,
    pub audius: Option<AudiusConfig>,
    pub pandora: Option<PandoraConfig>,
    pub qobuz: Option<QobuzConfig>,
    pub anghami: Option<AnghamiConfig>,
    pub shazam: Option<ShazamConfig>,
    pub mixcloud: Option<MixcloudConfig>,
    pub bandcamp: Option<BandcampConfig>,
    pub twitch: Option<TwitchConfig>,
    pub netease: Option<NeteaseMusicConfig>,
    pub vkmusic: Option<VkMusicConfig>,
    pub yandexmusic: Option<YandexMusicConfig>,
    pub google_tts: Option<GoogleTtsConfig>,
    pub flowery: Option<FloweryConfig>,
    pub reddit: Option<RedditConfig>,
    pub lastfm: Option<LastFmConfig>,
    pub rss: Option<RssConfig>,
    pub iheartradio: Option<IheartradioConfig>,
    pub bluesky: Option<BlueskyConfig>,
    pub tiktok: Option<TiktokConfig>,
    pub tumblr: Option<TumblrConfig>,
    pub newgrounds: Option<NewgroundsConfig>,
    pub kwai: Option<KwaiConfig>,
    pub telegram: Option<TelegramConfig>,
    pub googledrive: Option<GoogleDriveConfig>,
    pub lazypytts: Option<LazypyttsConfig>,
    pub pinterest: Option<PinterestConfig>,
    pub songlink: Option<SonglinkConfig>,
    pub vimeo: Option<VimeoConfig>,
    pub bilibili: Option<BilibiliConfig>,
    pub nicovideo: Option<NicovideoConfig>,
    pub instagram: Option<InstagramConfig>,
    pub twitter: Option<TwitterConfig>,
}

pub fn default_true() -> bool {
    true
}
pub fn default_false() -> bool {
    false
}

pub fn default_limit_10() -> usize {
    10
}
pub fn default_limit_20() -> usize {
    20
}
pub fn default_limit_50() -> usize {
    50
}
pub fn default_limit_100() -> usize {
    100
}
pub fn default_limit_3000() -> usize {
    3000
}

pub fn default_country_code() -> String {
    "us".to_string()
}
pub fn default_zero() -> usize {
    0
}
pub fn default_five() -> usize {
    5
}

pub fn default_tidal_quality() -> String {
    "LOSSLESS".to_string()
}
