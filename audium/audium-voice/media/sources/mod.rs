// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

pub mod error;
pub mod manager;
pub mod plugin;

pub use error::SourceError;
pub use manager::SourceManager;
pub use plugin::{BoxedSource, BoxedTrack, SourcePlugin};

pub mod amazonmusic;
pub mod anghami;
pub mod applemusic;
pub mod audiomack;
pub mod audius;
pub mod bandcamp;
pub mod bilibili;
pub mod bluesky;
pub mod deezer;
pub mod gaana;
pub mod googledrive;
pub mod http;
pub mod iheartradio;
pub mod instagram;
pub mod jiosaavn;
pub mod lastfm;
pub mod local;
pub mod mixcloud;
pub mod netease;
pub mod nicovideo;
pub mod pandora;
pub mod pinterest;
pub mod pixeldrain;
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
