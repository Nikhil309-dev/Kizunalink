// Copyright (c) 2026 nikcodex (KizunaLink)
// Licensed under the MIT License

use serde::Serialize;

/// Server statistics.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub players: u64,
    pub playing_players: u64,
    pub uptime: u64,
    pub memory: Memory,
    pub cpu: Cpu,
    // Official nodes always emit this key, serialized as `null` when there are
    // no players (both over the WebSocket and on `/v4/stats`).
    pub frame_stats: Option<FrameStats>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub free: u64,
    pub used: u64,
    pub allocated: u64,
    pub reservable: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cpu {
    pub cores: i32,
    pub system_load: f64,
    pub lavalink_load: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameStats {
    pub sent: i32,
    pub nulled: i32,
    pub deficit: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_stats() -> Stats {
        Stats {
            players: 0,
            playing_players: 0,
            uptime: 0,
            memory: Memory {
                free: 0,
                used: 0,
                allocated: 0,
                reservable: 0,
            },
            cpu: Cpu {
                cores: 1,
                system_load: 0.0,
                lavalink_load: 0.0,
            },
            frame_stats: None,
        }
    }

    /// Official nodes always serialize `frameStats` (as `null` when idle); a
    /// missing key breaks strict clients.
    #[test]
    fn frame_stats_is_always_present_as_null() {
        let json = serde_json::to_value(base_stats()).unwrap();
        assert!(
            json.get("frameStats").is_some(),
            "frameStats key must exist"
        );
        assert!(json["frameStats"].is_null());
    }

    #[test]
    fn frame_stats_values_serialize_camel_case() {
        let mut stats = base_stats();
        stats.frame_stats = Some(FrameStats {
            sent: 6000,
            nulled: 10,
            deficit: -3010,
        });
        let json = serde_json::to_value(stats).unwrap();
        assert_eq!(json["frameStats"]["sent"], 6000);
        assert_eq!(json["frameStats"]["nulled"], 10);
        assert_eq!(json["frameStats"]["deficit"], -3010);
    }
}
