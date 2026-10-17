// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Tracker-module (MOD/XM/S3M/IT) decoding.
//!
//! The companion [`crate::engine::demux::tracker`] reader drives playback one
//! chunk at a time: each packet's timestamp is the *frame offset* the decoder
//! should resume from, which doubles as the seek signal (the processor calls
//! `FormatReader::seek` then `Decoder::reset`). The module is parsed once and
//! then rendered with [`xmrsplayer`], a pure safe-Rust tracker replayer.

use std::convert::Infallible;

use symphonia::core::{
    audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Layout, Signal, SignalSpec},
    codecs::{
        CodecDescriptor, CodecParameters, CodecType, Decoder, DecoderOptions, FinalizeResult,
        decl_codec_type,
    },
    errors::{Error, Result},
    formats::Packet,
    units::Duration,
};
use xmrs::prelude::Module;
use xmrsplayer::prelude::XmrsPlayer;

use crate::engine::constants::TARGET_SAMPLE_RATE;

/// Codec identity for a tracker module stream.
pub const CODEC_TYPE_TRACKER: CodecType = decl_codec_type(b"TRACKER");

/// Frames rendered per packet (~100 ms at 48 kHz). The processor pulls audio
/// packet by packet, so the reader emits one such chunk at a time.
pub const CHUNK_FRAMES: usize = 4800;

/// Convert a time in seconds to a frame offset at the mixer sample rate.
pub fn seconds_to_ts(seconds: f64) -> u64 {
    (seconds.max(0.0) * TARGET_SAMPLE_RATE as f64).round() as u64
}

// The player borrows the module it replays; store both together without unsafe.
self_cell::self_cell!(
    struct ModulePlayer {
        owner: Module,
        #[covariant]
        dependent: XmrsPlayer,
    }
);

/// Number of frames a module plays for, at the mixer sample rate.
pub fn module_duration_frames(bytes: &[u8]) -> std::result::Result<u64, String> {
    let module = Module::load(bytes).map_err(|e| format!("invalid tracker module: {e:?}"))?;
    let cell = ModulePlayer::try_new(module, |m| Ok(XmrsPlayer::new(m, TARGET_SAMPLE_RATE, 0)))
        .map_err(|_: Infallible| "tracker player init failed".to_string())?;
    let seconds = cell.borrow_dependent().duration_seconds();
    Ok(seconds_to_ts(seconds))
}

/// A minimal but valid 4-channel ProTracker MOD with one empty pattern.
///
/// Used by the tracker reader and decoder tests; it exercises the real
/// `Module::load` path without shipping a binary fixture.
#[cfg(test)]
pub fn tiny_mod_for_tests() -> Vec<u8> {
    let mut m = vec![0u8; 20];
    m.extend(vec![0u8; 31 * 30]); // 31 sample headers
    m.push(1); // song length (orders)
    m.push(127); // restart position
    m.extend(vec![0u8; 128]); // order table
    m.extend(b"M.K."); // 31-instrument MOD signature
    m.extend(vec![0u8; 1024]); // one 64-row pattern, 4 channels
    m
}

/// A MOD whose first row triggers a C-2 note on a looping saw sample.
///
/// Proves the decoder renders real audio rather than silence.
#[cfg(test)]
pub fn mod_with_note_for_tests() -> Vec<u8> {
    let mut m = vec![0u8; 20]; // title

    // Sample 1: 64 words (128 bytes), volume 64, one-word loop.
    let mut sample = vec![0u8; 30];
    sample[22..24].copy_from_slice(&64u16.to_be_bytes()); // length in words
    sample[25] = 64; // volume
    sample[28..30].copy_from_slice(&1u16.to_be_bytes()); // loop length in words
    m.extend(&sample);
    m.extend(vec![0u8; 30 * 30]); // samples 2..=31 are empty

    m.push(1); // song length
    m.push(127); // restart position
    let mut orders = vec![0u8; 128];
    orders[0] = 0;
    m.extend(orders);
    m.extend(b"M.K.");

    // Pattern 0, row 0, channel 0: C-2 (period 428) with sample 1.
    // MOD slot layout: b0 = sample-num hi nibble | period hi nibble,
    // b1 = period low byte, b2 = sample-num lo nibble << 4 | effect, b3 = param.
    let mut pattern = vec![0u8; 1024];
    const PERIOD: u16 = 428;
    pattern[0] = (PERIOD >> 8) as u8 & 0x0F;
    pattern[1] = (PERIOD & 0xFF) as u8;
    pattern[2] = 1 << 4;
    m.extend(&pattern);

    // Saw sample data so the render is clearly non-silent.
    m.extend((0..128u32).map(|i| (i * 2) as u8));
    m
}

pub struct TrackerCodecDecoder {
    params: CodecParameters,
    /// `None` until the first packet (which carries the module bytes) arrives.
    cell: Option<ModulePlayer>,
    buf: AudioBuffer<i16>,
    /// Interleaved L/R scratch, reused across chunks.
    pcm: Vec<i16>,
    /// Frame offset the module has been rendered to, for change detection.
    rendered_until: u64,
}

// `XmrsPlayer` is `Send` but not `Sync` (its optional observer lists hold
// `Box<dyn … + Send>`). Every mutation goes through `&mut self`, so shared
// access cannot observe it concurrently — the same justification as
// `OpusCodecDecoder`'s `unsafe impl Sync`.
unsafe impl Sync for TrackerCodecDecoder {}

impl TrackerCodecDecoder {
    /// Build a decoder that already owns `bytes`, the module it replays.
    ///
    /// Preferred over the registry constructor: the module then no longer has to
    /// travel through packets, so a seek issued before the first packet (for
    /// example resuming at a saved position) still finds a parsed module.
    pub fn with_module(params: &CodecParameters, bytes: &[u8]) -> Result<Self> {
        let mut decoder = Self::try_new(params, &DecoderOptions::default())?;
        decoder.parse(bytes)?;
        Ok(decoder)
    }

    fn parse(&mut self, bytes: &[u8]) -> Result<()> {
        let module = Module::load(bytes).map_err(|e| {
            Error::IoError(std::io::Error::other(format!(
                "invalid tracker module: {e:?}"
            )))
        })?;
        let mut cell =
            ModulePlayer::try_new(module, |m| Ok(XmrsPlayer::new(m, TARGET_SAMPLE_RATE, 0)))
                .map_err(|_: Infallible| {
                    Error::IoError(std::io::Error::other("tracker player init failed"))
                })?;
        // Play the module through exactly once; looping forever would never end.
        cell.with_dependent_mut(|_m, player| player.set_max_loop_count(1));
        self.cell = Some(cell);
        self.rendered_until = 0;
        Ok(())
    }
}

impl Decoder for TrackerCodecDecoder {
    fn try_new(params: &CodecParameters, _options: &DecoderOptions) -> Result<Self> {
        if params.codec != CODEC_TYPE_TRACKER {
            return Err(Error::Unsupported("not a tracker module stream"));
        }
        let spec = SignalSpec::new_with_layout(TARGET_SAMPLE_RATE, Layout::Stereo);
        Ok(Self {
            params: params.clone(),
            cell: None,
            buf: AudioBuffer::<i16>::new(CHUNK_FRAMES as Duration, spec),
            pcm: vec![0i16; CHUNK_FRAMES * 2],
            rendered_until: 0,
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[CodecDescriptor {
            codec: CODEC_TYPE_TRACKER,
            short_name: "tracker",
            long_name: "Tracker module (MOD/XM/S3M/IT via xmrsplayer)",
            inst_func: |params, opts| Ok(Box::new(TrackerCodecDecoder::try_new(params, opts)?)),
        }]
    }

    fn reset(&mut self) {
        // The module and its player are retained across seeks; the next packet's
        // timestamp re-points the player at the requested position.
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        let Some(cell) = self.cell.as_mut() else {
            // The module normally arrives via `with_module`. Falling back to the
            // first packet keeps the registry constructor usable, but a stream
            // that never supplied one must fail loudly rather than emit silence.
            if packet.data.is_empty() {
                return Err(Error::IoError(std::io::Error::other(
                    "tracker decoder was given no module",
                )));
            }
            self.parse(packet.data.as_ref())?;
            return self.decode(packet);
        };

        let target = packet.ts();
        let mut rendered_until = self.rendered_until;
        let mut pcm = std::mem::take(&mut self.pcm);
        let mut frames = 0usize;

        cell.with_dependent_mut(|_m, player| {
            if target != rendered_until {
                let seconds = target as f64 / TARGET_SAMPLE_RATE as f64;
                let _ = player.seek_seconds(seconds);
                rendered_until = target;
            }
            while frames < CHUNK_FRAMES {
                match (player.next(), player.next()) {
                    (Some(left), Some(right)) => {
                        pcm[frames * 2] = left;
                        pcm[frames * 2 + 1] = right;
                        frames += 1;
                    }
                    (Some(left), None) => {
                        pcm[frames * 2] = left;
                        pcm[frames * 2 + 1] = left;
                        frames += 1;
                        break;
                    }
                    _ => break,
                }
            }
        });

        // Pad the tail with silence when the module ended mid-chunk.
        for sample in &mut pcm[frames * 2..CHUNK_FRAMES * 2] {
            *sample = 0;
        }

        self.buf.clear();
        self.buf.render_reserved(Some(frames.max(1)));
        let written = frames.max(1);
        let (left, right) = self.buf.chan_pair_mut(0, 1);
        for i in 0..written {
            left[i] = pcm[i * 2];
            right[i] = pcm[i * 2 + 1];
        }

        self.rendered_until = target + CHUNK_FRAMES as u64;
        self.pcm = pcm;
        Ok(self.buf.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::demux::tracker::TrackerFormatReader;
    use std::io::{Read, Seek, SeekFrom};
    use symphonia::core::{audio::SampleBuffer, formats::FormatReader, io::MediaSource};

    struct BytesSource(std::io::Cursor<Vec<u8>>);
    impl Read for BytesSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Seek for BytesSource {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(pos)
        }
    }
    impl MediaSource for BytesSource {
        fn is_seekable(&self) -> bool {
            true
        }
        fn byte_len(&self) -> Option<u64> {
            Some(self.0.get_ref().len() as u64)
        }
    }

    #[test]
    fn decodes_tracker_module_to_stereo_pcm() {
        let bytes = tiny_mod_for_tests();
        let frames = module_duration_frames(&bytes).expect("duration");
        assert!(frames > 0, "a module must have a positive duration");

        let src = Box::new(BytesSource(std::io::Cursor::new(bytes.clone())));
        let mut reader = TrackerFormatReader::open(src).expect("open");
        let first = reader.next_packet().expect("first packet");

        let params = reader.tracks()[0].codec_params.clone();
        let mut decoder =
            TrackerCodecDecoder::with_module(&params, reader.module_bytes()).expect("decoder");
        let decoded = decoder.decode(&first).expect("decode");
        let spec = decoded.spec();
        assert_eq!(spec.rate, TARGET_SAMPLE_RATE);
        assert_eq!(spec.channels.count(), 2);
        assert_eq!(decoded.frames(), CHUNK_FRAMES);
    }

    #[test]
    fn rejects_non_tracker_codec() {
        let mut params = CodecParameters::new();
        params.with_sample_rate(48_000);
        assert!(TrackerCodecDecoder::try_new(&params, &DecoderOptions::default()).is_err());
    }

    #[test]
    fn renders_a_triggered_note_as_audible_pcm() {
        let bytes = mod_with_note_for_tests();
        let src = Box::new(BytesSource(std::io::Cursor::new(bytes)));
        let mut reader = TrackerFormatReader::open(src).expect("open");
        let first = reader.next_packet().expect("first packet");

        let params = reader.tracks()[0].codec_params.clone();
        let mut decoder =
            TrackerCodecDecoder::with_module(&params, reader.module_bytes()).expect("decoder");
        let decoded = decoder.decode(&first).expect("decode");
        let spec = *decoded.spec();
        let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        assert!(
            buf.samples().iter().any(|&s| s != 0),
            "a triggered note must produce non-silent samples"
        );
    }

    #[test]
    fn decodes_after_a_seek_issued_before_the_first_packet() {
        // The processor checks for a pending seek before reading any packet, so a
        // decoder built without the module in hand must still render after one.
        let bytes = mod_with_note_for_tests();
        let src = Box::new(BytesSource(std::io::Cursor::new(bytes)));
        let mut reader = TrackerFormatReader::open(src).expect("open");
        let params = reader.tracks()[0].codec_params.clone();

        let mut decoder =
            TrackerCodecDecoder::with_module(&params, reader.module_bytes()).expect("decoder");
        reader
            .seek(
                symphonia::core::formats::SeekMode::Coarse,
                symphonia::core::formats::SeekTo::Time {
                    time: symphonia::core::units::Time::new(1, 0.0),
                    track_id: Some(0),
                },
            )
            .expect("seek");
        let packet = reader.next_packet().expect("packet after seek");
        assert_eq!(packet.ts(), 48_000);
        assert!(decoder.decode(&packet).is_ok(), "seek must still render");
    }
}
