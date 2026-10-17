// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

pub mod format;
pub mod tracker;
pub mod webm_opus;

pub use format::detect_format;
use symphonia::core::{
    codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatOptions, FormatReader},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};
pub use tracker::TrackerFormatReader;
pub use webm_opus::WebmOpusDemuxer;

pub use crate::common::types::AudioFormat;
use crate::engine::constants::{MIXER_CHANNELS, TARGET_SAMPLE_RATE};

pub enum DemuxResult {
    Transcode {
        format: Box<dyn FormatReader>,
        track_id: u32,
        decoder: Box<dyn Decoder>,
        sample_rate: u32,
        channels: usize,
    },
}

pub fn open_format(
    source: Box<dyn MediaSource>,
    kind: Option<crate::common::types::AudioFormat>,
) -> Result<DemuxResult, Error> {
    if matches!(kind, Some(AudioFormat::Tracker)) {
        let format = TrackerFormatReader::open(source)?;
        let track = format.tracks().first().ok_or_else(|| {
            Error::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no tracker track found",
            ))
        })?;
        let track_id = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(TARGET_SAMPLE_RATE);
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count())
            .unwrap_or(MIXER_CHANNELS);
        let decoder: Box<dyn Decoder> = Box::new(
            crate::engine::codec::tracker_decoder::TrackerCodecDecoder::with_module(
                &track.codec_params,
                format.module_bytes(),
            )?,
        );
        return Ok(DemuxResult::Transcode {
            format: Box::new(format),
            track_id,
            decoder,
            sample_rate,
            channels,
        });
    }

    let mss = MediaSourceStream::new(source, Default::default());

    let mut hint = Hint::new();
    if let Some(k) = &kind {
        hint.with_extension(k.as_ext());
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| {
            Error::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no audio track found",
            ))
        })?;

    let track_id = track.id;
    let codec = track.codec_params.codec;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(TARGET_SAMPLE_RATE);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(MIXER_CHANNELS);

    let decoder: Box<dyn Decoder> = if codec == symphonia::core::codecs::CODEC_TYPE_OPUS {
        Box::new(
            crate::engine::codec::opus_decoder::OpusCodecDecoder::try_new(
                &track.codec_params,
                &DecoderOptions::default(),
            )?,
        )
    } else {
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?
    };

    Ok(DemuxResult::Transcode {
        format,
        track_id,
        decoder,
        sample_rate,
        channels,
    })
}
