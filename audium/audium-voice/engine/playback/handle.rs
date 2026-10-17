// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
};

use crate::engine::{constants::OPUS_SAMPLE_RATE, processor::DecoderCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaybackState {
    Playing = 0,
    Paused = 1,
    Stopped = 2,
    Stopping = 3,
    Starting = 4,
}

impl From<u8> for PlaybackState {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Playing,
            1 => Self::Paused,
            3 => Self::Stopping,
            4 => Self::Starting,
            _ => Self::Stopped,
        }
    }
}

#[derive(Clone)]
pub struct TrackHandle {
    state: Arc<AtomicU8>,
    volume: Arc<AtomicU32>,
    position: Arc<AtomicU64>,
    command_tx: flume::Sender<DecoderCommand>,
    tape_stop_enabled: Arc<AtomicBool>,
    is_buffering: Arc<AtomicBool>,
}

impl TrackHandle {
    pub fn new(
        command_tx: flume::Sender<DecoderCommand>,
        tape_stop_enabled: Arc<AtomicBool>,
    ) -> (
        Self,
        Arc<AtomicU8>,
        Arc<AtomicU32>,
        Arc<AtomicU64>,
        Arc<AtomicBool>,
    ) {
        let state = Arc::new(AtomicU8::new(PlaybackState::Playing as u8));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let position = Arc::new(AtomicU64::new(0));
        let is_buffering = Arc::new(AtomicBool::new(false));

        (
            Self {
                state: state.clone(),
                volume: volume.clone(),
                position: position.clone(),
                command_tx,
                tape_stop_enabled,
                is_buffering: is_buffering.clone(),
            },
            state,
            volume,
            position,
            is_buffering,
        )
    }

    pub fn pause(&self) {
        let next_state = if self.tape_stop_enabled.load(Ordering::Acquire) {
            PlaybackState::Stopping
        } else {
            PlaybackState::Paused
        };
        self.state.store(next_state as u8, Ordering::Release);
    }

    pub fn play(&self) {
        let next_state = if self.tape_stop_enabled.load(Ordering::Acquire) {
            PlaybackState::Starting
        } else {
            PlaybackState::Playing
        };
        self.state.store(next_state as u8, Ordering::Release);
    }

    pub fn stop(&self) {
        self.state
            .store(PlaybackState::Stopped as u8, Ordering::SeqCst);
    }

    pub fn set_volume(&self, vol: f32) {
        self.volume.store(vol.to_bits(), Ordering::Release);
    }

    pub fn get_state(&self) -> PlaybackState {
        let s = self.state.load(Ordering::Acquire);
        let mut state = PlaybackState::from(s);

        if state != PlaybackState::Stopped && self.command_tx.is_disconnected() {
            state = PlaybackState::Stopped;
            self.state.store(state as u8, Ordering::Release);
        }
        state
    }

    pub fn get_position(&self) -> u64 {
        let samples = self.position.load(Ordering::Acquire);
        samples.saturating_mul(1000) / OPUS_SAMPLE_RATE
    }

    pub fn is_buffering(&self) -> bool {
        self.is_buffering.load(Ordering::Acquire)
    }

    pub fn seek(&self, position_ms: u64) {
        let samples = position_ms.saturating_mul(OPUS_SAMPLE_RATE) / 1000;
        self.position.store(samples, Ordering::Release);
        let _ = self.command_tx.send(DecoderCommand::Seek(position_ms));
    }

    pub fn is_same(&self, other: &Self) -> bool {
        self.command_tx.same_channel(&other.command_tx)
    }

    /// Shared state cell for this track; used to identify the track inside the
    /// mixer (the mixer holds a clone of the same `Arc`).
    pub fn state_arc(&self) -> Arc<AtomicU8> {
        self.state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> (TrackHandle, flume::Receiver<DecoderCommand>) {
        let (tx, rx) = flume::bounded(8);
        let (h, ..) = TrackHandle::new(tx, Arc::new(AtomicBool::new(false)));
        (h, rx)
    }

    #[test]
    fn seek_saturates_instead_of_wrapping() {
        let (h, _rx) = handle();
        h.seek(u64::MAX);
        let pos = h.get_position();

        assert!(
            pos > 1_000_000_000_000,
            "seek(u64::MAX) wrapped to {pos} ms"
        );
    }

    #[test]
    fn seek_round_trips_normal_positions() {
        let (h, _rx) = handle();
        h.seek(90_000);
        assert_eq!(h.get_position(), 90_000);
    }
}
