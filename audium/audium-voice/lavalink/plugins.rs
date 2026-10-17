// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Lavalink plugin advertisement.
//!
//! Audium is a native Rust server, so it cannot load JVM `.jar` plugins. What it
//! *can* do is implement the behaviour those plugins add, and report the plugin
//! names clients look for in `/v4/info`'s `plugins` array. Clients gate features
//! on that array rather than probing endpoints, so leaving it empty makes them
//! disable features Audium actually supports (Magmastream even throws when
//! `lavasrc-plugin` is absent).
//!
//! Each entry is advertised **only when the matching capability is enabled**.
//! Claiming `sponsorblock-plugin` while the feature is off would send clients to
//! an endpoint that 404s — worse than advertising nothing. The names and
//! versions match the upstream `.jar` identities exactly, because the checks are
//! string comparisons (`plugins.some(p => p.name === "lavasrc-plugin")`).

use crate::config::AppConfig;
use crate::lavalink::protocol::info::Plugin;

/// Upstream plugin identities. These are the strings clients compare against.
pub const LAVASRC: &str = "lavasrc-plugin";
pub const LAVASEARCH: &str = "lavasearch-plugin";
pub const LAVALYRICS: &str = "lavalyrics-plugin";
pub const SPONSORBLOCK: &str = "sponsorblock-plugin";
pub const YOUTUBE: &str = "youtube-plugin";
pub const LAVADSPX: &str = "lavadspx-plugin";
pub const FILTER_PLUGIN: &str = "lavalink-filter-plugin";
pub const TTS: &str = "tts-plugin";
pub const JIOSAAVN: &str = "jiosaavn-plugin";

/// Versions are the upstream release lines clients pin against (e.g. Shoukaku
/// requires `lavalyrics-plugin ^1.1.0` and `lavadspx-plugin ^0.0.5`).
const LAVASRC_VERSION: &str = "4.4.1";
const LAVASEARCH_VERSION: &str = "1.0.0";
const LAVALYRICS_VERSION: &str = "1.1.0";
const SPONSORBLOCK_VERSION: &str = "3.0.0";
const YOUTUBE_VERSION: &str = "1.13.0";
const LAVADSPX_VERSION: &str = "0.0.5";
const FILTER_PLUGIN_VERSION: &str = "0.0.4";
const TTS_VERSION: &str = "1.0.0";
const JIOSAAVN_VERSION: &str = "1.0.0";

fn entry(name: &str, version: &str) -> Plugin {
    Plugin {
        name: name.to_string(),
        version: version.to_string(),
    }
}

/// Returns `true` when the source is present *and* enabled.
fn source_on<T>(source: &Option<T>, enabled: impl Fn(&T) -> bool) -> bool {
    source.as_ref().is_some_and(enabled)
}

/// The plugins to advertise for this configuration, in a stable order.
///
/// Only capabilities that are actually available are listed, so the array is a
/// truthful statement of what a client may use.
pub fn advertised(config: &AppConfig) -> Vec<Plugin> {
    let mut plugins = Vec::new();

    let lavasrc_on = source_on(&config.sources.spotify, |c| c.enabled)
        || source_on(&config.sources.applemusic, |c| c.enabled)
        || source_on(&config.sources.deezer, |c| c.enabled)
        || source_on(&config.sources.yandexmusic, |c| c.enabled)
        || source_on(&config.sources.tidal, |c| c.enabled);
    if lavasrc_on {
        plugins.push(entry(LAVASRC, LAVASRC_VERSION));
    }

    plugins.push(entry(LAVASEARCH, LAVASEARCH_VERSION));

    let lyrics = &config.lyrics;
    let any_lyrics = lyrics.youtubemusic
        || lyrics.lrclib
        || lyrics.genius
        || lyrics.deezer
        || lyrics.musixmatch
        || lyrics.letrasmus
        || lyrics.yandex
        || lyrics.netease;
    if any_lyrics {
        plugins.push(entry(LAVALYRICS, LAVALYRICS_VERSION));
    }

    if config.player.sponsorblock.enabled {
        plugins.push(entry(SPONSORBLOCK, SPONSORBLOCK_VERSION));
    }

    if source_on(&config.sources.youtube, |c| c.enabled) {
        plugins.push(entry(YOUTUBE, YOUTUBE_VERSION));
    }

    let f = &config.filters;
    let any_dspx = f.echo
        || f.compressor
        || f.flanger
        || f.phaser
        || f.phonograph
        || f.reverb
        || f.spatial
        || f.chorus
        || f.high_pass
        || f.normalization;
    if any_dspx {
        plugins.push(entry(LAVADSPX, LAVADSPX_VERSION));
    }

    if f.echo || f.reverb {
        plugins.push(entry(FILTER_PLUGIN, FILTER_PLUGIN_VERSION));
    }

    if source_on(&config.sources.google_tts, |c| c.enabled) {
        plugins.push(entry(TTS, TTS_VERSION));
    }

    if source_on(&config.sources.jiosaavn, |c| c.enabled) {
        plugins.push(entry(JIOSAAVN, JIOSAAVN_VERSION));
    }

    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a config fragment the same way the server does, so serde defaults
    /// (which decide `enabled`) are exercised rather than hand-built structs.
    /// `[server]` has no serde default (the secret must be explicit), so it is
    /// always supplied.
    fn cfg(body: &str) -> AppConfig {
        let raw = format!("[server]\nauthorization = 'test-secret'\n{body}");
        toml::from_str(&raw).expect("test config deserializes")
    }

    fn names(config: &AppConfig) -> Vec<String> {
        advertised(config).iter().map(|p| p.name.clone()).collect()
    }

    fn has(config: &AppConfig, name: &str) -> bool {
        names(config).contains(&name.to_string())
    }

    /// With no sources and no lyrics, the always-on plugins still appear:
    /// LavaSearch (native loadsearch) plus the DSPX/filter plugins, because
    /// every audio filter defaults to enabled.
    #[test]
    fn minimal_config_advertises_only_capabilities_that_exist() {
        assert_eq!(
            names(&cfg("")),
            vec![
                LAVASEARCH.to_string(),
                LAVADSPX.to_string(),
                FILTER_PLUGIN.to_string(),
            ]
        );
    }

    #[test]
    fn youtube_is_advertised_only_when_enabled() {
        assert!(!has(&cfg(""), YOUTUBE));
        assert!(!has(&cfg("[sources.youtube]\nenabled = false"), YOUTUBE));
        assert!(has(&cfg("[sources.youtube]\nenabled = true"), YOUTUBE));
    }

    #[test]
    fn sponsorblock_is_advertised_only_when_enabled() {
        assert!(!has(&cfg(""), SPONSORBLOCK));
        assert!(has(
            &cfg("[player.sponsorblock]\nenabled = true"),
            SPONSORBLOCK
        ));
    }

    #[test]
    fn lavasrc_requires_an_enabled_source() {
        assert!(!has(&cfg(""), LAVASRC));
        assert!(!has(&cfg("[sources.spotify]\nenabled = false"), LAVASRC));
        assert!(has(&cfg("[sources.spotify]\nenabled = true"), LAVASRC));

        assert!(has(&cfg("[sources.deezer]\nenabled = true"), LAVASRC));
        assert!(has(&cfg("[sources.applemusic]\nenabled = true"), LAVASRC));
        assert!(has(&cfg("[sources.tidal]\nenabled = true"), LAVASRC));
    }

    #[test]
    fn lyrics_require_at_least_one_provider() {
        assert!(!has(&cfg(""), LAVALYRICS));
        assert!(has(&cfg("[lyrics]\nlrclib = true"), LAVALYRICS));
        assert!(has(&cfg("[lyrics]\ngenius = true"), LAVALYRICS));
    }

    #[test]
    fn dspx_and_filter_plugins_follow_the_filter_toggles() {
        assert!(has(&cfg(""), LAVADSPX));
        assert!(has(&cfg(""), FILTER_PLUGIN));

        let no_dspx = cfg(
            "[filters]\necho = false\ncompressor = false\nflanger = false\n\
             phaser = false\nphonograph = false\nreverb = false\nspatial = false\n\
             chorus = false\nhigh_pass = false\nnormalization = false",
        );
        assert!(!has(&no_dspx, LAVADSPX));
        assert!(!has(&no_dspx, FILTER_PLUGIN));

        assert!(has(&no_dspx, LAVASEARCH));
    }

    #[test]
    fn tts_and_jiosaavn_are_gated_on_their_sources() {
        assert!(!has(&cfg(""), TTS));
        assert!(!has(&cfg(""), JIOSAAVN));
        assert!(has(&cfg("[sources.google_tts]\nenabled = true"), TTS));
        assert!(has(&cfg("[sources.jiosaavn]\nenabled = true"), JIOSAAVN));
    }

    #[test]
    fn every_advertised_entry_is_well_formed() {
        let full = cfg("[sources.youtube]\nenabled = true\n\
             [sources.spotify]\nenabled = true\n\
             [sources.jiosaavn]\nenabled = true\n\
             [sources.google_tts]\nenabled = true\n\
             [lyrics]\nlrclib = true\n\
             [player.sponsorblock]\nenabled = true");
        for plugin in advertised(&full) {
            assert!(!plugin.name.is_empty(), "plugin name must not be empty");
            assert!(
                plugin.version.split('.').count() >= 2,
                "{} version {} must look like a release",
                plugin.name,
                plugin.version
            );
        }
    }

    #[test]
    fn advertisement_is_deterministic_and_duplicate_free() {
        let config = cfg("[sources.youtube]\nenabled = true\n[lyrics]\nlrclib = true");
        let first = names(&config);
        assert_eq!(first, names(&config), "advertisement must be deterministic");

        let mut sorted = first.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), first.len(), "no duplicate plugin entries");
    }
}
