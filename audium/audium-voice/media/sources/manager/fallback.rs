// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Transparent stream-time source fallback.
//!
//! Some sources resolve metadata successfully but fail when the decoder opens
//! the media stream — most notably YouTube, where a datacenter egress resolves
//! fine and then gets `HTTP 403` from the CDN. Lavalink surfaces that as a
//! `TrackException` and the user hears nothing.
//!
//! [`TransparentFallbackTrack`] wraps the primary track so that, if it fails
//! *before delivering any audio*, playback continues from a mirror of the same
//! metadata (e.g. JioSaavn/Gaana). The player keeps the original `TrackInfo`,
//! so clients still see the original source (title, author, artwork, URI) even
//! though the audio is coming from a mirror.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use parking_lot::Mutex;

use tracing::{info, warn};

use crate::{
    engine::{AudioFrame, processor::DecoderCommand},
    lavalink::protocol::tracks::TrackInfo,
    lavalink::routeplanner::RoutePlanner,
    media::sources::{
        manager::SourceManager,
        plugin::{BoxedTrack, DecoderOutput, PlayableTrack},
    },
};

pub struct TransparentFallbackTrack {
    primary: BoxedTrack,
    track_info: TrackInfo,
    source_manager: Weak<SourceManager>,
    routeplanner: Option<Arc<dyn RoutePlanner>>,
}

impl TransparentFallbackTrack {
    pub fn wrap(
        primary: BoxedTrack,
        track_info: TrackInfo,
        source_manager: Weak<SourceManager>,
        routeplanner: Option<Arc<dyn RoutePlanner>>,
    ) -> BoxedTrack {
        Box::new(Self {
            primary,
            track_info,
            source_manager,
            routeplanner,
        })
    }
}

impl PlayableTrack for TransparentFallbackTrack {
    fn start_decoding(&self, config: crate::config::player::PlayerConfig) -> DecoderOutput {
        let (p_frames, p_cmd, p_err) = self.primary.start_decoding(config.clone());

        let (f_tx, f_rx) =
            flume::bounded::<AudioFrame>((config.buffer_duration_ms / 20).max(8) as usize);
        let (c_tx, c_rx) = flume::bounded::<DecoderCommand>(8);
        let (e_tx, e_rx) = flume::bounded::<String>(1);

        let got_audio = Arc::new(AtomicBool::new(false));

        let f_tx_primary = f_tx.clone();
        let got_audio_for_frames = got_audio.clone();
        tokio::spawn(async move {
            while let Ok(frame) = p_frames.recv_async().await {
                got_audio_for_frames.store(true, Ordering::Release);
                if f_tx_primary.send_async(frame).await.is_err() {
                    return;
                }
            }
        });

        let cmd_target = Arc::new(Mutex::new(p_cmd));
        let cmd_target_for_router = cmd_target.clone();
        tokio::spawn(async move {
            while let Ok(cmd) = c_rx.recv_async().await {
                let tx = cmd_target_for_router.lock().clone();
                if tx.send_async(cmd).await.is_err() {}
            }
        });

        let info = self.track_info.clone();
        let weak = self.source_manager.clone();
        let rp = self.routeplanner.clone();
        let f_tx_fb = f_tx.clone();
        let e_tx_fb = e_tx.clone();
        let cmd_target_for_fb = cmd_target;
        let got_audio_for_err = got_audio.clone();

        tokio::spawn(async move {
            if got_audio_for_err.load(Ordering::Acquire) {
                if let Ok(msg) = p_err.recv_async().await {
                    let _ = e_tx_fb.send(msg);
                }
                return;
            }

            tokio::select! {
                biased;
                _ = async {

                    while !got_audio_for_err.load(Ordering::Acquire) {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                } => {


                    if let Ok(msg) = p_err.recv_async().await {
                        let _ = e_tx_fb.send(msg);
                    }
                }
                msg = p_err.recv_async() => {
                    let msg = match msg {
                        Ok(m) => m,
                        Err(_) => return,
                    };
                    if got_audio_for_err.load(Ordering::Acquire) {

                        let _ = e_tx_fb.send(msg);
                        return;
                    }
                    let Some(manager) = weak.upgrade() else {
                        let _ = e_tx_fb.send(msg);
                        return;
                    };
                    warn!(
                        "Transparent fallback: '{}' failed before audio ({}); resolving a mirror",
                        info.title, msg
                    );
                    match manager.resolve_mirror(&info, rp.clone()).await {
                        Ok(mirror) => {
                            let (m_frames, m_cmd, m_err) = mirror.start_decoding(config.clone());
                            *cmd_target_for_fb.lock() = m_cmd;
                            info!(
                                "Transparent fallback: '{}' now streaming from a mirror",
                                info.title
                            );

                            let f_tx_inner = f_tx_fb.clone();
                            tokio::spawn(async move {
                                while let Ok(frame) = m_frames.recv_async().await {
                                    if f_tx_inner.send_async(frame).await.is_err() {
                                        return;
                                    }
                                }
                            });
                            if let Ok(m) = m_err.recv_async().await {
                                let _ = e_tx_fb.send(m);
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Transparent fallback: no mirror for '{}': {}",
                                info.title, e
                            );
                            let _ = e_tx_fb.send(msg);
                        }
                    }
                }
            }
        });

        (f_rx, c_tx, e_rx)
    }
}
