// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Tracker-module (MOD/XM/S3M/IT) container support.
//!
//! Neither Lavalink nor NodeLink can play tracker modules; Audium does so
//! through a native, pure-Rust path. A module file is a single self-contained
//! document, so this reader exposes it as exactly one packet — all of its bytes
//! — which [`crate::engine::codec::tracker_decoder`] replays into PCM. Seeks
//! are handled inside the decoder, which owns the module timeline.

use std::io::{Read, Seek, SeekFrom};

use symphonia::core::{
    audio::Channels,
    codecs::CodecParameters,
    errors::{Error, Result},
    formats::{Cue, FormatOptions, FormatReader, Packet, SeekMode, SeekTo, SeekedTo, Track},
    io::{MediaSource, MediaSourceStream},
    meta::{Metadata, MetadataLog},
};

use crate::engine::codec::tracker_decoder::{
    CHUNK_FRAMES, CODEC_TYPE_TRACKER, module_duration_frames, seconds_to_ts,
};
use crate::engine::constants::TARGET_SAMPLE_RATE;

/// A `FormatReader` that re-emits a tracker module one chunk at a time.
///
/// Each packet carries the full module bytes (the decoder parses them once) and
/// a timestamp equal to the frame offset to render next. That timestamp doubles
/// as the seek signal: after [`FormatReader::seek`] the reader simply resumes
/// emitting from the new offset.
pub struct TrackerFormatReader {
    source: MediaSourceStream,
    tracks: Vec<Track>,
    cues: Vec<Cue>,
    metadata: MetadataLog,
    /// Full module bytes, captured once on open.
    bytes: Vec<u8>,
    /// Total number of frames the module plays for.
    n_frames: u64,
    /// Frame offset of the next packet.
    frame_pos: u64,
}

impl TrackerFormatReader {
    /// Open a tracker module from an owned media source.
    pub fn open(source: Box<dyn MediaSource>) -> Result<Self> {
        Self::build(MediaSourceStream::new(source, Default::default()))
    }

    /// Total playtime of the module in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.n_frames.saturating_mul(1000) / TARGET_SAMPLE_RATE as u64
    }

    fn build(mut source: MediaSourceStream) -> Result<Self> {
        if !source.is_seekable() {
            return Err(Error::Unsupported(
                "tracker modules require a seekable source",
            ));
        }
        // The prober may have read ahead while sniffing; start from byte 0 so
        // the whole module is captured.
        source.seek(SeekFrom::Start(0)).map_err(Error::IoError)?;

        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).map_err(Error::IoError)?;

        // Parse up front so an unplayable module fails the load rather than
        // producing a stream of silence.
        let n_frames =
            module_duration_frames(&bytes).map_err(|e| Error::IoError(std::io::Error::other(e)))?;

        let mut params = CodecParameters::new();
        // Placeholder rate/channels: the packets below carry raw module bytes,
        // not timestamped PCM. The codec id selects the tracker decoder.
        params
            .for_codec(CODEC_TYPE_TRACKER)
            .with_sample_rate(TARGET_SAMPLE_RATE)
            .with_channels(Channels::FRONT_LEFT | Channels::FRONT_RIGHT);

        Ok(Self {
            source,
            tracks: vec![Track::new(0, params)],
            cues: Vec::new(),
            metadata: MetadataLog::default(),
            bytes,
            n_frames,
            frame_pos: 0,
        })
    }
}

impl FormatReader for TrackerFormatReader {
    fn try_new(source: MediaSourceStream, _options: &FormatOptions) -> Result<Self> {
        Self::build(source)
    }

    fn cues(&self) -> &[Cue] {
        &self.cues
    }

    fn metadata(&mut self) -> Metadata<'_> {
        self.metadata.metadata()
    }

    fn seek(&mut self, _mode: SeekMode, to: SeekTo) -> Result<SeekedTo> {
        // No byte-level index exists. Translate the request into a frame offset
        // and resume emitting from there; the decoder re-points the player when
        // it sees the new packet timestamp.
        let (track_id, ts) = match to {
            SeekTo::TimeStamp { ts, track_id } => (track_id, ts),
            SeekTo::Time { time, .. } => (
                0,
                seconds_to_ts(time.seconds as f64 + time.frac).min(self.n_frames),
            ),
        };
        self.frame_pos = ts;
        Ok(SeekedTo {
            track_id,
            required_ts: ts,
            actual_ts: ts,
        })
    }

    fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    fn next_packet(&mut self) -> Result<Packet> {
        if self.frame_pos >= self.n_frames {
            return Err(Error::IoError(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "end of tracker module",
            )));
        }

        let ts = self.frame_pos;
        self.frame_pos = self.frame_pos.saturating_add(CHUNK_FRAMES as u64);

        // The module bytes are re-sent every packet; the decoder parses them
        // exactly once. `ts` is the frame offset to render next.
        Ok(Packet::new_from_boxed_slice(
            0,
            ts,
            CHUNK_FRAMES as u64,
            self.bytes.clone().into_boxed_slice(),
        ))
    }

    fn into_inner(self: Box<Self>) -> MediaSourceStream {
        self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::codec::tracker_decoder::tiny_mod_for_tests;

    /// A seekable, in-memory media source for tests.
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
    fn emits_the_module_in_chunks_then_ends() {
        let data = tiny_mod_for_tests();
        let src = Box::new(BytesSource(std::io::Cursor::new(data.clone())));
        let mut reader = TrackerFormatReader::open(src).expect("open");

        let first = reader.next_packet().expect("first packet");
        assert_eq!(first.track_id(), 0);
        assert_eq!(first.ts(), 0);
        assert_eq!(first.data.as_ref(), data.as_slice());

        // The stream advances one chunk at a time and eventually ends.
        let second = reader.next_packet().expect("second packet");
        assert_eq!(second.ts(), CHUNK_FRAMES as u64);

        let mut packets = 2u64;
        while reader.next_packet().is_ok() {
            packets += 1;
            assert!(packets < 100_000, "module must terminate");
        }
        assert!(packets >= 3);
    }

    #[test]
    fn seek_resumes_from_requested_frame() {
        let data = tiny_mod_for_tests();
        let src = Box::new(BytesSource(std::io::Cursor::new(data)));
        let mut reader = TrackerFormatReader::open(src).expect("open");

        let _ = reader.next_packet().expect("first");
        let seeked = reader
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: symphonia::core::units::Time::new(1, 0.5),
                    track_id: Some(0),
                },
            )
            .expect("seek");
        // 1.5s at 48 kHz.
        assert_eq!(seeked.actual_ts, 72_000);
        let resumed = reader.next_packet().expect("after seek");
        assert_eq!(resumed.ts(), 72_000);
    }

    #[test]
    fn rejects_unparseable_module() {
        let src = Box::new(BytesSource(std::io::Cursor::new(vec![0u8; 512])));
        assert!(TrackerFormatReader::open(src).is_err());
    }
}
