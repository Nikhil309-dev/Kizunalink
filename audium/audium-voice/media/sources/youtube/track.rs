// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::{net::IpAddr, sync::Arc, time::Duration};

use tracing::{debug, error, info, warn};

use crate::{
    common::types::AnyError,
    config::sources::HttpProxyConfig,
    engine::{AudioFrame, processor::DecoderCommand},
    media::sources::{
        plugin::{DecoderOutput, PlayableTrack},
        youtube::{
            cipher::YouTubeCipherManager,
            clients::{
                YouTubeClient,
                common::{FailureClass, YoutubeFailure},
            },
            oauth::YouTubeOAuth,
            utils::{create_reader, detect_audio_kind},
        },
    },
};

/// Classify an error bubbling out of a client or the reader into a
/// [`YoutubeFailure`]. Errors already carrying a [`YoutubeFailure`] keep their
/// class; everything else is heuristically classified from its message so the
/// playback loop never blindly retries a request that cannot succeed.
fn classify_error(e: &AnyError) -> YoutubeFailure {
    if let Some(y) = e.downcast_ref::<YoutubeFailure>() {
        return y.clone();
    }

    let msg = e.to_string();
    let low = msg.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| low.contains(n));

    if has(&["not a bot", "sign in", "login required", "bot verification"]) {
        return YoutubeFailure::bot_check(msg);
    }
    if has(&["403", "429", "rate limit"]) {
        return YoutubeFailure::transient(msg);
    }
    if has(&[
        "timed out",
        "timeout",
        "connection",
        "connect",
        "dns",
        "tls",
        "reset",
        "broken pipe",
        "unreachable",
        "502",
        "503",
        "504",
    ]) {
        return YoutubeFailure::transient(msg);
    }
    YoutubeFailure::new(FailureClass::Unknown, msg)
}

/// What the playback loop should do after every client has failed to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureAction {
    /// At least one client reported a definitive failure: stop now, no retry.
    StopDefinitive,
    /// Every failure was transient/unknown and the retry budget allows one more
    /// backed-off sweep of the client list.
    RetryTransient,
    /// Out of retries, or a bot-check / unclassified failure that waiting cannot
    /// fix: surface the accumulated reason.
    GiveUp,
}

fn decide_after_all_clients_failed(
    has_definitive: bool,
    all_transient: bool,
    transient_retries: u32,
) -> FailureAction {
    if has_definitive {
        FailureAction::StopDefinitive
    } else if all_transient && transient_retries < 2 {
        FailureAction::RetryTransient
    } else {
        FailureAction::GiveUp
    }
}

pub struct YoutubeTrack {
    pub identifier: String,
    pub clients: Vec<Arc<dyn YouTubeClient>>,
    pub oauth: Arc<YouTubeOAuth>,
    pub cipher_manager: Arc<YouTubeCipherManager>,
    pub visitor_data: Option<String>,
    pub local_addr: Option<IpAddr>,
    pub proxy: Option<HttpProxyConfig>,
}

impl PlayableTrack for YoutubeTrack {
    fn start_decoding(&self, config: crate::config::player::PlayerConfig) -> DecoderOutput {
        let (tx, rx) = flume::bounded::<AudioFrame>((config.buffer_duration_ms / 20) as usize);
        let (cmd_tx, cmd_rx) = flume::bounded(8);
        let (err_tx, err_rx) = flume::bounded(1);

        let identifier_async = self.identifier.clone();
        let cipher_manager_async = self.cipher_manager.clone();
        let oauth_async = self.oauth.clone();
        let clients_async = self.clients.clone();
        let visitor_data_for_task = self.visitor_data.clone();
        let proxy_bg = self.proxy.clone();
        let local_addr_bg = self.local_addr;

        tokio::spawn(async move {
            let context = serde_json::json!({ "visitorData": visitor_data_for_task });

            let mut current_seek_ms = 0u64;
            let mut current_client_index = 0;
            // Number of whole-client-list retries already performed for a purely
            // transient failure. Definitive/BotCheck failures never consume this.
            let mut transient_retries = 0u32;

            'playback_loop: loop {
                // Resolve a playback URL using any available client.
                let mut resolved_url: Option<(String, String)> = None;
                let mut failures: Vec<String> = Vec::new();
                // True if every client failed for a reason that another attempt or
                // another client might plausibly overcome.
                let mut all_transient = true;
                let mut first_definitive: Option<YoutubeFailure> = None;

                for (idx, client) in clients_async.iter().enumerate().skip(current_client_index) {
                    current_client_index = idx;
                    let client_name = client.name().to_string();

                    debug!(
                        "YoutubeTrack: Resolving '{}' using {}",
                        identifier_async, client_name
                    );
                    match client
                        .get_track_url(
                            &identifier_async,
                            &context,
                            cipher_manager_async.clone(),
                            oauth_async.clone(),
                        )
                        .await
                    {
                        Ok(Some(url)) => {
                            info!(
                                "YoutubeTrack: resolved track URL for '{}' using client '{}'",
                                identifier_async, client_name
                            );
                            resolved_url = Some((url, client_name));
                            break;
                        }
                        Ok(None) => {
                            debug!(
                                "YoutubeTrack: client {} returned no URL for {}",
                                client_name, identifier_async
                            );
                            failures.push(format!("{client_name}: no playable format"));
                        }
                        Err(e) => {
                            let failure = classify_error(&e);
                            warn!(
                                "YoutubeTrack: client {} failed to resolve {} [{}]: {}",
                                client_name, identifier_async, failure.class, failure.reason
                            );
                            match failure.class {
                                FailureClass::Definitive => {
                                    // No client on any IP can play this; stop now.
                                    first_definitive.get_or_insert(failure.clone());
                                    all_transient = false;
                                }
                                FailureClass::BotCheck => {
                                    // Another client may have laxer requirements, but a
                                    // blind retry of the same request will not help.
                                    all_transient = false;
                                }
                                FailureClass::Transient | FailureClass::Unknown => {}
                            }
                            failures.push(format!("{client_name}: {}", failure.reason));
                        }
                    }
                }

                let (url, client_name) = match resolved_url {
                    Some(r) => r,
                    None => {
                        let action = decide_after_all_clients_failed(
                            first_definitive.is_some(),
                            all_transient,
                            transient_retries,
                        );

                        match action {
                            FailureAction::StopDefinitive => {
                                // Private/removed/region-locked: more clients or a
                                // retry cannot fix it, so preserve the real reason.
                                // `decide_after_all_clients_failed` only returns this
                                // when a definitive failure was recorded, but bind it
                                // explicitly so a future refactor can't turn this into
                                // a panic.
                                let def = match first_definitive {
                                    Some(def) => def,
                                    None => YoutubeFailure::new(
                                        FailureClass::Unknown,
                                        format!("all clients failed: {}", failures.join("; ")),
                                    ),
                                };
                                let msg = format!(
                                    "YoutubeTrack: cannot play '{}': {} (clients tried: {})",
                                    identifier_async,
                                    def.reason,
                                    failures.join("; ")
                                );
                                error!("{msg}");
                                let _ = err_tx.send(msg);
                                return;
                            }
                            FailureAction::RetryTransient => {
                                transient_retries += 1;
                                current_client_index = 0;
                                debug!(
                                    "YoutubeTrack: all clients transiently failed for '{}', \
                                     retry {}/2 after backoff",
                                    identifier_async, transient_retries
                                );
                                tokio::time::sleep(Duration::from_millis(
                                    750 * u64::from(transient_retries),
                                ))
                                .await;
                                continue 'playback_loop;
                            }
                            FailureAction::GiveUp => {}
                        }

                        let msg = format!(
                            "YoutubeTrack: All clients failed to resolve '{}': {}. If every \
                             client reports a sign-in / bot check, the egress IP is blocked — \
                             configure `sources.youtube.proxy` (clean/residential egress) \
                             and/or enable a PoToken provider under `sources.youtube.pot`.",
                            identifier_async,
                            failures.join("; ")
                        );
                        error!("{msg}");
                        let _ = err_tx.send(msg);
                        return;
                    }
                };

                let is_hls = url.contains(".m3u8") || url.contains("/playlist");
                let url_clone = url.clone();
                let cipher_clone = cipher_manager_async.clone();
                let proxy_clone = proxy_bg.clone();
                let client_name_inner = client_name.clone();

                let reader_res_task = tokio::task::spawn_blocking(move || {
                    create_reader(
                        &url_clone,
                        &client_name_inner,
                        local_addr_bg,
                        proxy_clone,
                        cipher_clone,
                    )
                })
                .await;

                let reader_res = match reader_res_task {
                    Ok(res) => res,
                    Err(e) => {
                        error!("YoutubeTrack: spawn_blocking failed: {}", e);
                        let _ = err_tx.send(format!("Failed to spawn blocking task: {e}"));
                        return;
                    }
                };

                let reader = match reader_res {
                    Ok(r) => r,
                    Err(e) => {
                        let failure = classify_error(&e);
                        warn!(
                            "YoutubeTrack: Reader initialization failed for '{}' with client {} [{}]: {}. Attempting fallback...",
                            identifier_async, client_name, failure.class, failure.reason
                        );
                        current_client_index += 1;
                        if current_client_index < clients_async.len() {
                            continue 'playback_loop;
                        } else if failure.class != FailureClass::Definitive && transient_retries < 2
                        {
                            transient_retries += 1;
                            current_client_index = 0;
                            debug!(
                                "YoutubeTrack: all clients failed to initialize reader for '{}', retry {}/2 after backoff",
                                identifier_async, transient_retries
                            );
                            tokio::time::sleep(Duration::from_millis(
                                750 * u64::from(transient_retries),
                            ))
                            .await;
                            continue 'playback_loop;
                        } else {
                            error!(
                                "YoutubeTrack: All clients failed to initialize reader for '{}': {}",
                                identifier_async, failure.reason
                            );
                            let _ = err_tx.send(failure.reason);
                            return;
                        }
                    }
                };

                let kind = detect_audio_kind(&url, is_hls);

                let (inner_cmd_tx, inner_cmd_rx) = flume::bounded(8);
                let tx_clone = tx.clone();
                let err_tx_clone = err_tx.clone();

                let config_for_processor = config.clone();
                let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
                let identifier_for_thread = identifier_async.clone();

                if let Err(e) = std::thread::Builder::new()
                    .stack_size(crate::engine::constants::DECODE_THREAD_STACK_SIZE)
                    .name(format!("youtube-decoder-{}", identifier_async))
                    .spawn(move || {
                        let result = match crate::engine::processor::AudioProcessor::new(
                            reader,
                            Some(kind),
                            tx_clone,
                            inner_cmd_rx,
                            Some(err_tx_clone.clone()),
                            config_for_processor,
                        ) {
                            Ok(mut processor) => processor.run_guarded().map_err(|e| e.to_string()),
                            Err(e) => {
                                error!(
                                    "YoutubeTrack: AudioProcessor initialization failed for {}: {}",
                                    identifier_for_thread, e
                                );
                                Err(format!("Failed to initialize processor: {}", e))
                            }
                        };
                        let _ = done_tx.send(result);
                    })
                {
                    error!("failed to spawn youtube decoder thread: {e}");
                    let _ = err_tx.send(format!("Failed to spawn decoder thread: {e}"));
                }

                if current_seek_ms > 0 {
                    let _ = inner_cmd_tx.send(DecoderCommand::Seek(current_seek_ms));
                }

                loop {
                    tokio::select! {
                        cmd_res = cmd_rx.recv_async() => {
                            match cmd_res {
                                Ok(DecoderCommand::Seek(ms)) => {
                                    current_seek_ms = ms;
                                    let _ = inner_cmd_tx.send(DecoderCommand::Seek(ms));
                                }
                                Ok(DecoderCommand::Stop) | Err(_) => {
                                    let _ = inner_cmd_tx.send(DecoderCommand::Stop);
                                    return;
                                }
                            }
                        }
                        res = &mut done_rx => {
                            let res = match res {
                                Ok(r) => Ok(r),
                                Err(_) => Err("decoder thread dropped".to_string()),
                            };
                            match res {
                                Ok(Err(e)) => {
                                    let failure = classify_error(&AnyError::from(e));
                                    warn!(
                                        "YoutubeTrack: Playback failed for '{}' with client {} [{}]: {}. Attempting fallback...",
                                        identifier_async, clients_async[current_client_index].name(),
                                        failure.class, failure.reason
                                    );
                                    // A definitive stream error (the CDN says this
                                    // media will never play, e.g. a hard 404/410) is
                                    // not worth replaying on every other client.
                                    if failure.class == FailureClass::Definitive {
                                        let msg = format!(
                                            "YoutubeTrack: playback failed for '{}': {}",
                                            identifier_async, failure.reason
                                        );
                                        error!("{msg}");
                                        let _ = err_tx.send(msg);
                                        return;
                                    }
                                    current_client_index += 1;
                                    if current_client_index < clients_async.len() {
                                        continue 'playback_loop;
                                    } else {
                                        error!(
                                            "YoutubeTrack: All clients failed for '{}'",
                                            identifier_async
                                        );
                                        let _ = err_tx.send(format!("All clients failed: {}", failure.reason));
                                    }
                                }
                                Ok(Ok(())) => {
                                    debug!(
                                        "YoutubeTrack: Playback finished for '{}'",
                                        identifier_async
                                    );
                                }
                                Err(e) => {
                                    error!(
                                        "YoutubeTrack: Join error for '{}': {}",
                                        identifier_async, e
                                    );
                                }
                            }
                            return;
                        }
                    }
                }
            }
        });

        (rx, cmd_tx, err_rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_error_preserves_typed_failure() {
        let e: AnyError = Box::new(YoutubeFailure::definitive("this video is private"));
        let f = classify_error(&e);
        assert_eq!(f.class, FailureClass::Definitive);
        assert!(f.reason.contains("private"));
    }

    #[test]
    fn classify_error_detects_bot_check_from_message() {
        let e: AnyError =
            "Player request failed (status=403): sign in to confirm you're not a bot".into();
        assert_eq!(classify_error(&e).class, FailureClass::BotCheck);
    }

    #[test]
    fn classify_error_treats_network_errors_as_transient() {
        let e: AnyError = "error sending request: connection reset by peer (503)".into();
        assert_eq!(classify_error(&e).class, FailureClass::Transient);
    }

    #[test]
    fn classify_error_unknown_falls_back_to_unknown() {
        let e: AnyError = "something totally unexpected".into();
        assert_eq!(classify_error(&e).class, FailureClass::Unknown);
    }

    #[test]
    fn definitive_failure_never_retries() {
        assert_eq!(
            decide_after_all_clients_failed(true, false, 0),
            FailureAction::StopDefinitive
        );
        // Even when a peer's failure was transient, a definitive failure wins.
        assert_eq!(
            decide_after_all_clients_failed(true, true, 0),
            FailureAction::StopDefinitive
        );
    }

    #[test]
    fn transient_failure_retries_then_gives_up() {
        assert_eq!(
            decide_after_all_clients_failed(false, true, 0),
            FailureAction::RetryTransient
        );
        assert_eq!(
            decide_after_all_clients_failed(false, true, 1),
            FailureAction::RetryTransient
        );
        // Budget exhausted.
        assert_eq!(
            decide_after_all_clients_failed(false, true, 2),
            FailureAction::GiveUp
        );
    }

    #[test]
    fn bot_check_does_not_retry() {
        // all_transient is false whenever a bot check was seen, so no retry.
        assert_eq!(
            decide_after_all_clients_failed(false, false, 0),
            FailureAction::GiveUp
        );
    }
}
