// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use serde::{Deserialize, Serialize};

use crate::{config::player::PlayerConfig, lavalink::protocol::tracks::Track};

/// Generate `serialize_with` helpers that emit a fixed default when the field is
/// `None`.
///
/// Lavalink echoes the *default* for any filter field a client leaves unset
/// (e.g. `timescale.speed` → `1.0`), never `null`. Emitting `null` breaks
/// clients that do arithmetic on the returned object. The engine applies these
/// same defaults via `unwrap_or` at processing time, so audio is unchanged.
macro_rules! default_serializers {
    ($ty:ty { $($name:ident = $val:expr),+ $(,)? }) => {
        $(
            fn $name<S>(value: &Option<$ty>, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                match value {
                    Some(v) => serde::Serialize::serialize(v, serializer),
                    None => serde::Serialize::serialize(&($val as $ty), serializer),
                }
            }
        )+
    };
}

default_serializers!(f32 {
    ser_zero = 0.0,
    ser_one = 1.0,
    ser_two = 2.0,
    ser_half = 0.5,
    ser_20 = 20.0,
    ser_100 = 100.0,
    ser_220 = 220.0,
    ser_neg10 = -10.0,
    ser_5 = 5.0,
    ser_50 = 50.0,
    ser_0_2 = 0.2,
    ser_1_5 = 1.5,
    ser_0_8 = 0.8,
    ser_0_25 = 0.25,
    ser_0_18 = 0.18,
    ser_0_22 = 0.22,
    ser_2500 = 2500.0,
});

default_serializers!(f64 {
    ser_f64_zero = 0.0,
    ser_f64_one = 1.0,
});

default_serializers!(i32 {
    ser_i32_200 = 200,
    ser_i32_4 = 4,
});

fn ser_true<S>(value: &Option<bool>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(v) => serde::Serialize::serialize(v, serializer),
        None => serde::Serialize::serialize(&true, serializer),
    }
}

/// Deserializer for track encoded field which can be null or string.
pub fn deserialize_track_encoded<'de, D>(deserializer: D) -> Result<Option<TrackEncoded>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(Some(TrackEncoded::Clear)),
        serde_json::Value::String(s) => Ok(Some(TrackEncoded::Set(s))),
        _ => Err(serde::de::Error::custom("expected string or null")),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub guild_id: crate::common::types::GuildId,
    pub track: Option<Track>,
    pub volume: i32,
    pub paused: bool,
    pub state: PlayerState,
    pub voice: VoiceState,
    pub filters: Filters,
    pub dave: Option<DaveState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaveState {
    pub protocol_version: u16,
    pub privacy_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceState {
    pub token: String,
    pub endpoint: String,
    pub session_id: String,
    #[serde(default)]
    pub channel_id: Option<String>,
}

#[derive(Clone, Default)]
pub struct VoiceConnectionState {
    pub token: String,
    pub endpoint: String,
    pub session_id: String,
    pub channel_id: Option<String>,
}

impl From<VoiceState> for VoiceConnectionState {
    fn from(v: VoiceState) -> Self {
        Self {
            token: v.token,
            endpoint: v.endpoint,
            session_id: v.session_id,
            channel_id: v.channel_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum EndTime {
    #[default]
    Clear,
    Set(u64),
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerUpdate {
    #[serde(default, deserialize_with = "deserialize_track_encoded")]
    pub encoded_track: Option<TrackEncoded>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub track: Option<PlayerUpdateTrack>,
    #[serde(default)]
    pub position: Option<u64>,
    #[serde(default)]
    pub end_time: Option<EndTime>,
    #[serde(default)]
    pub volume: Option<i32>,
    #[serde(default)]
    pub paused: Option<bool>,
    #[serde(default)]
    pub filters: Option<Filters>,
    #[serde(default)]
    pub voice: Option<VoiceState>,
    #[serde(default)]
    pub config: Option<PlayerConfig>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerUpdateTrack {
    #[serde(default, deserialize_with = "deserialize_track_encoded")]
    pub encoded: Option<TrackEncoded>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub user_data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TrackEncoded {
    Clear,
    Set(String),
}

macro_rules! define_filters {
    ($($field:ident : $type:ty => $name:expr),* $(,)?) => {
        #[derive(Debug, Clone, Default, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Filters {
            $(
                #[serde(skip_serializing_if = "Option::is_none")]
                pub $field: Option<$type>,
            )*
        }

        impl Filters {
            pub fn names() -> Vec<String> {
                vec![
                    $($name.into()),*
                ]
            }

            pub fn merge_from(&mut self, incoming: Filters) {
                $(
                    if incoming.$field.is_some() {
                        self.$field = incoming.$field;
                    }
                )*
            }

            pub fn is_all_none(&self) -> bool {
                $(
                    self.$field.is_none() &&
                )* true
            }
        }
    };
}

define_filters! {
    volume: f32 => "volume",
    equalizer: Vec<EqBand> => "equalizer",
    karaoke: KaraokeFilter => "karaoke",
    timescale: TimescaleFilter => "timescale",
    tremolo: TremoloFilter => "tremolo",
    vibrato: VibratoFilter => "vibrato",
    distortion: DistortionFilter => "distortion",
    rotation: RotationFilter => "rotation",
    channel_mix: ChannelMixFilter => "channelMix",
    low_pass: LowPassFilter => "lowPass",
    echo: EchoFilter => "echo",
    high_pass: HighPassFilter => "highPass",
    normalization: NormalizationFilter => "normalization",
    chorus: ChorusFilter => "chorus",
    compressor: CompressorFilter => "compressor",
    flanger: FlangerFilter => "flanger",
    phaser: PhaserFilter => "phaser",
    phonograph: PhonographFilter => "phonograph",
    reverb: ReverbFilter => "reverb",
    spatial: SpatialFilter => "spatial",
    plugin_filters: std::collections::HashMap<String, serde_json::Value> => "pluginFilters",
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBand {
    pub band: u8,
    pub gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KaraokeFilter {
    #[serde(serialize_with = "ser_one")]
    pub level: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub mono_level: Option<f32>,
    #[serde(serialize_with = "ser_220")]
    pub filter_band: Option<f32>,
    #[serde(serialize_with = "ser_100")]
    pub filter_width: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimescaleFilter {
    #[serde(serialize_with = "ser_f64_one")]
    pub speed: Option<f64>,
    #[serde(serialize_with = "ser_f64_one")]
    pub pitch: Option<f64>,
    #[serde(serialize_with = "ser_f64_one")]
    pub rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TremoloFilter {
    #[serde(serialize_with = "ser_two")]
    pub frequency: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub depth: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VibratoFilter {
    #[serde(serialize_with = "ser_two")]
    pub frequency: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub depth: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistortionFilter {
    #[serde(serialize_with = "ser_zero")]
    pub sin_offset: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub sin_scale: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub cos_offset: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub cos_scale: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub tan_offset: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub tan_scale: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub offset: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub scale: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationFilter {
    #[serde(serialize_with = "ser_f64_zero")]
    pub rotation_hz: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMixFilter {
    #[serde(serialize_with = "ser_one")]
    pub left_to_left: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub left_to_right: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub right_to_left: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub right_to_right: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LowPassFilter {
    #[serde(serialize_with = "ser_20")]
    pub smoothing: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoFilter {
    #[serde(serialize_with = "ser_one")]
    pub echo_length: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub decay: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighPassFilter {
    #[serde(serialize_with = "ser_i32_200")]
    pub cutoff_frequency: Option<i32>,
    #[serde(serialize_with = "ser_one")]
    pub boost_factor: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationFilter {
    #[serde(serialize_with = "ser_one")]
    pub max_amplitude: Option<f32>,
    #[serde(serialize_with = "ser_true")]
    pub adaptive: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChorusFilter {
    #[serde(serialize_with = "ser_1_5")]
    pub rate: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub depth: Option<f32>,
    #[serde(serialize_with = "ser_two")]
    pub delay: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub mix: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub feedback: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressorFilter {
    #[serde(serialize_with = "ser_neg10")]
    pub threshold: Option<f32>,
    #[serde(serialize_with = "ser_two")]
    pub ratio: Option<f32>,
    #[serde(serialize_with = "ser_5")]
    pub attack: Option<f32>,
    #[serde(serialize_with = "ser_50")]
    pub release: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub makeup_gain: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlangerFilter {
    #[serde(serialize_with = "ser_0_2")]
    pub rate: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub depth: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub feedback: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaserFilter {
    #[serde(serialize_with = "ser_i32_4")]
    pub stages: Option<i32>,
    #[serde(serialize_with = "ser_zero")]
    pub rate: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub depth: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub feedback: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub mix: Option<f32>,
    #[serde(serialize_with = "ser_100")]
    pub min_frequency: Option<f32>,
    #[serde(serialize_with = "ser_2500")]
    pub max_frequency: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonographFilter {
    #[serde(serialize_with = "ser_0_8")]
    pub frequency: Option<f32>,
    #[serde(serialize_with = "ser_0_25")]
    pub depth: Option<f32>,
    #[serde(serialize_with = "ser_0_18")]
    pub crackle: Option<f32>,
    #[serde(serialize_with = "ser_0_18")]
    pub flutter: Option<f32>,
    #[serde(serialize_with = "ser_0_22")]
    pub room: Option<f32>,
    #[serde(serialize_with = "ser_0_25")]
    pub mic_agc: Option<f32>,
    #[serde(serialize_with = "ser_0_25")]
    pub drive: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverbFilter {
    #[serde(serialize_with = "ser_zero")]
    pub mix: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub room_size: Option<f32>,
    #[serde(serialize_with = "ser_half")]
    pub damping: Option<f32>,
    #[serde(serialize_with = "ser_one")]
    pub width: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpatialFilter {
    #[serde(serialize_with = "ser_zero")]
    pub depth: Option<f32>,
    #[serde(serialize_with = "ser_zero")]
    pub rate: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Unset filter fields must serialize to Lavalink's concrete defaults, not
    /// `null`, so clients can safely do arithmetic on the returned object.
    #[test]
    fn unset_filter_fields_serialize_lavalink_defaults() {
        let filters = Filters {
            timescale: Some(TimescaleFilter {
                speed: None,
                pitch: None,
                rate: None,
            }),
            karaoke: Some(KaraokeFilter {
                level: None,
                mono_level: None,
                filter_band: None,
                filter_width: None,
            }),
            tremolo: Some(TremoloFilter {
                frequency: None,
                depth: None,
            }),
            vibrato: Some(VibratoFilter {
                frequency: None,
                depth: None,
            }),
            rotation: Some(RotationFilter { rotation_hz: None }),
            channel_mix: Some(ChannelMixFilter {
                left_to_left: None,
                left_to_right: None,
                right_to_left: None,
                right_to_right: None,
            }),
            low_pass: Some(LowPassFilter { smoothing: None }),
            distortion: Some(DistortionFilter {
                sin_offset: None,
                sin_scale: None,
                cos_offset: None,
                cos_scale: None,
                tan_offset: None,
                tan_scale: None,
                offset: None,
                scale: None,
            }),
            ..Default::default()
        };

        let value = serde_json::to_value(&filters).unwrap();

        assert_eq!(
            value["timescale"],
            json!({"speed": 1.0, "pitch": 1.0, "rate": 1.0})
        );
        assert_eq!(
            value["karaoke"],
            json!({"level": 1.0, "monoLevel": 1.0, "filterBand": 220.0, "filterWidth": 100.0})
        );
        assert_eq!(value["tremolo"], json!({"frequency": 2.0, "depth": 0.5}));
        assert_eq!(value["vibrato"], json!({"frequency": 2.0, "depth": 0.5}));
        assert_eq!(value["rotation"], json!({"rotationHz": 0.0}));
        assert_eq!(
            value["channelMix"],
            json!({"leftToLeft": 1.0, "leftToRight": 0.0, "rightToLeft": 0.0, "rightToRight": 1.0})
        );
        assert_eq!(value["lowPass"], json!({"smoothing": 20.0}));
        assert_eq!(
            value["distortion"],
            json!({
                "sinOffset": 0.0, "sinScale": 1.0, "cosOffset": 0.0, "cosScale": 1.0,
                "tanOffset": 0.0, "tanScale": 1.0, "offset": 0.0, "scale": 1.0
            })
        );
    }

    /// Explicit values still round-trip unchanged.
    #[test]
    fn set_filter_fields_are_preserved() {
        let filters = Filters {
            timescale: Some(TimescaleFilter {
                speed: Some(1.5),
                pitch: Some(0.75),
                rate: Some(2.0),
            }),
            low_pass: Some(LowPassFilter {
                smoothing: Some(5.0),
            }),
            ..Default::default()
        };

        let value = serde_json::to_value(&filters).unwrap();
        assert_eq!(
            value["timescale"],
            json!({"speed": 1.5, "pitch": 0.75, "rate": 2.0})
        );
        assert_eq!(value["lowPass"], json!({"smoothing": 5.0}));
    }
}
