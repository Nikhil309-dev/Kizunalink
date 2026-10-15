// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use axum::{extract::State, response::Json};

use crate::{discord::player::Filters, protocol, server::AppState};

/// GET /v4/info
pub async fn get_info(State(state): State<Arc<AppState>>) -> Json<protocol::Info> {
    tracing::info!("GET /v4/info");

    Json(protocol::Info {
        version: protocol_version(),
        build_time: option_env!("BUILD_TIME")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        git: protocol::GitInfo {
            branch: option_env!("GIT_BRANCH").unwrap_or("unknown").to_string(),
            commit: option_env!("GIT_COMMIT_SHORT")
                .or(option_env!("GIT_COMMIT"))
                .unwrap_or("unknown")
                .to_string(),
            commit_time: option_env!("GIT_COMMIT_TIME")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        },
        jvm: "Rust".to_string(),
        lavaplayer: "symphonia".to_string(),
        source_managers: state.source_manager.source_names(),
        filters: Filters::names()
            .into_iter()
            .filter(|name| state.config.filters.is_enabled(name))
            .collect(),
        plugins: vec![],
    })
}

// A malformed build-time label must never make /v4/info emit invalid SemVer.
fn valid_pre_release(label: &str) -> Option<&str> {
    if label.split('.').all(|part| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !(part.len() > 1
                && part.starts_with('0')
                && part.bytes().all(|b| b.is_ascii_digit()))
    }) {
        Some(label)
    } else {
        None
    }
}

fn parse_semver(v: &str) -> (u32, u32, u32, Option<String>) {
    let mut parts = v.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(4);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let patch_raw = parts.next().unwrap_or("0");
    let (patch_str, pre_release) = if let Some(idx) = patch_raw.find('-') {
        (&patch_raw[..idx], Some(patch_raw[idx + 1..].to_string()))
    } else {
        (patch_raw, None)
    };

    let patch = patch_str.parse().ok().unwrap_or(0);

    (major, minor, patch, pre_release)
}

/// Wire-protocol version reported by `/v4/info` and `/version`.
///
/// Clients compare `major` against the *Lavalink protocol* version (official
/// nodes report `4.x.y`); the crate's own semver (`1.x.y`) is the server
/// release, not the wire protocol, so it is normalised to `4.x.y` here. Both
/// endpoints derive from this single helper so they can never disagree.
fn protocol_version() -> protocol::Version {
    let version_str = env!("CARGO_PKG_VERSION");
    let (crate_major, minor, patch, mut pre_release) = parse_semver(version_str);

    if pre_release.is_none()
        && let Some(pre) = option_env!("AUDIUM_PRE_RELEASE")
    {
        pre_release = valid_pre_release(pre).map(str::to_string);
    }

    let major = if crate_major < 4 { 4 } else { crate_major };

    protocol::Version {
        semver: match &pre_release {
            Some(pre) => format!("{major}.{minor}.{patch}-{pre}"),
            None => format!("{major}.{minor}.{patch}"),
        },
        major,
        minor,
        patch,
        pre_release: pre_release.unwrap_or_default(),
        build: option_env!("BUILD_NUMBER").map(|s| s.to_string()),
    }
}

pub async fn get_stats(State(state): State<Arc<AppState>>) -> Json<protocol::Stats> {
    tracing::info!("GET /v4/stats");
    let mut stats = crate::monitoring::collect_stats(&state, None);

    if state.config.metrics.detailed_stats {
        let counters = &state.detailed_counters;
        // The registry only holds sources that are both compiled-in and
        // config-enabled, so `loaded == enabled`; `top` needs a per-source
        // resolve counter the engine does not track yet.
        let loaded = state.source_manager.sources.len();
        let detailed = counters.snapshot(loaded, loaded, None, stats.cpu.cores.max(0) as usize);
        stats.detailed_stats = Some(protocol::DetailedStats {
            total_requests: detailed.total_requests,
            total_errors: detailed.total_errors,
            process: protocol::DetailedProcess {
                process_id: detailed.process.process_id,
                threads: detailed.process.threads,
                cpu_cores: detailed.process.cpu_cores,
            },
            sources: protocol::DetailedSources {
                loaded: detailed.sources.loaded,
                enabled: detailed.sources.enabled,
                top: detailed.sources.top,
            },
            playback: protocol::DetailedPlayback {
                tracks_started: detailed.playback.tracks_started,
                tracks_ended: detailed.playback.tracks_ended,
                exceptions: detailed.playback.exceptions,
            },
        });
    }

    Json(stats)
}

pub async fn get_version() -> String {
    tracing::info!("GET /version");
    protocol_version().semver
}

#[cfg(test)]
mod tests {
    use super::valid_pre_release;

    #[test]
    fn pre_release_label_must_be_valid_semver() {
        for valid in ["alpha", "alpha.1", "rc-2", "0", "1.2"] {
            assert_eq!(valid_pre_release(valid), Some(valid));
        }
        for invalid in [
            "", " ", ".alpha", "alpha.", "alpha..1", "01", "beta.02", "a_b",
        ] {
            assert_eq!(valid_pre_release(invalid), None);
        }
    }
}
