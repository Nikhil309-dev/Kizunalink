// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::{net::SocketAddr, sync::Arc};

use audium::common::types::AnyResult;
use audium_server::{
    api::{rest, ws},
    server::AppState,
};
use axum::{
    Router, extract::DefaultBodyLimit, middleware::from_fn_with_state, routing::get,
    serve::ListenerExt,
};
use dashmap::DashMap;
use tracing::info;

/// Entry point.
///
/// Deliberately *not* `#[tokio::main]`: we must configure the FPU (FTZ/DAZ, P02) on the
/// thread that will later build the runtime, because Linux `clone()` copies the FPU control
/// word into child threads — every tokio worker and every audio thread spawned from this
/// main thread then inherits the denormal-free setting for free.
fn main() -> AnyResult<()> {
    audium::config::AppConfig::reject_unknown_args(std::env::args())?;

    audium::common::tls::install_crypto_provider();

    std::panic::set_hook(Box::new(|info| {
        let msg = format!("panic: {info}");
        if tracing::dispatcher::has_been_set() {
            tracing::error!("{msg}");
        } else {
            eprintln!("[audium] {msg}");
        }
    }));

    audium::engine::disable_denormals();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run())
}

async fn run() -> AnyResult<()> {
    let config = audium::config::AppConfig::load().await?;

    audium::common::logger::init(
        config
            .logging
            .as_ref()
            .unwrap_or(&audium::config::LoggingConfig::default()),
    );

    audium::common::banner::print_banner(&audium::common::banner::BannerInfo::default());

    info!("Audium Server starting...");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));
    info!("Runtime: {}", std::env::consts::OS);
    info!("Configuration loaded successfully");
    info!(
        "Bind address: {}:{}",
        config.server.address, config.server.port
    );

    let routeplanner = if config.route_planner.enabled && !config.route_planner.cidrs.is_empty() {
        Some(Arc::new(
            audium_server::lavalink::routeplanner::BalancingIpRoutePlanner::new(
                config.route_planner.cidrs.clone(),
            )
            .map_err(|e| {
                Box::new(std::io::Error::other(format!(
                    "invalid route planner CIDR: {e}"
                ))) as audium::common::types::AnyError
            })?,
        )
            as Arc<dyn audium::lavalink::routeplanner::RoutePlanner>)
    } else {
        None
    };

    info!("[Sources] Initializing audio source registry");
    let source_manager = Arc::new(audium::media::sources::SourceManager::new(&config));
    source_manager.set_self_weak();
    info!(
        "[Sources] Source registry initialized: {} sources",
        source_manager.sources.len()
    );
    let lyrics_manager = Arc::new(audium::media::lyrics::LyricsManager::new(&config));
    let youtube_ctx = source_manager.youtube_stream_ctx.clone();

    let process_stat = audium_server::monitoring::sysstat::ProcessStat::cur();

    let shared_state = Arc::new(AppState {
        start_time: std::time::Instant::now(),
        sessions: DashMap::new(),
        resumable_sessions: DashMap::new(),
        routeplanner,
        source_manager,
        lyrics_manager,
        config: config.clone(),
        youtube: youtube_ctx,
        system_state: parking_lot::Mutex::new(sysinfo::System::new_all()),
        last_system_refresh: parking_lot::Mutex::new(std::time::Instant::now()),
        process_stat: parking_lot::Mutex::new(process_stat),
        rate_limiter: Arc::new(audium_server::api::RateLimiter::new(
            config.server.rate_limit_per_minute,
        )),
        connection_limit: (config.server.max_connections > 0)
            .then(|| Arc::new(tokio::sync::Semaphore::new(config.server.max_connections))),
        detailed_counters: audium_server::monitoring::detailed::DetailedCounters::new(),
    });

    audium_server::monitoring::prometheus::init(shared_state.clone());

    let mut app = Router::new()
        .route("/health", get(audium_server::health::health_check))
        .route("/v4/websocket", get(ws::websocket_handler))
        .with_state(shared_state.clone())
        .merge(rest::router(shared_state.clone()))
        .layer(DefaultBodyLimit::max(config.server.max_body_bytes))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn_with_state(
            shared_state.clone(),
            audium_server::api::rate_limit::rate_limit,
        ))
        .layer(axum::middleware::from_fn_with_state(
            shared_state.clone(),
            audium_server::api::conn_limit::connection_limit,
        ))
        .layer(axum::middleware::from_fn_with_state(
            shared_state.clone(),
            audium_server::monitoring::prometheus::count_requests,
        ));

    if config.metrics.prometheus.enabled {
        let metrics_router = Router::new()
            .route(
                &config.metrics.prometheus.endpoint,
                get(audium_server::monitoring::prometheus::metrics_handler),
            )
            .layer(from_fn_with_state(
                shared_state.clone(),
                audium_server::api::rest::middleware::check_auth,
            ))
            .layer(from_fn_with_state(
                shared_state.clone(),
                audium_server::api::rate_limit::rate_limit,
            ))
            .with_state(shared_state.clone());
        app = app.merge(metrics_router);

        app = app.layer(axum::middleware::from_fn(
            audium_server::monitoring::prometheus::observe_request_latency,
        ));
    }

    if !shared_state.rate_limiter.disabled() {
        let limiter = shared_state.rate_limiter.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                limiter.cleanup(
                    std::time::Instant::now(),
                    std::time::Duration::from_secs(600),
                );
            }
        });
    }

    let ip: std::net::IpAddr = config.server.address.parse()?;
    let address = SocketAddr::from((ip, config.server.port));

    if config.server.tls.enabled {
        let cert_path = config
            .server
            .tls
            .cert_path
            .as_deref()
            .expect("validated by AppConfig::validate()");
        let key_path = config
            .server
            .tls
            .key_path
            .as_deref()
            .expect("validated by AppConfig::validate()");
        let acceptor = audium_server::tls::load_acceptor(
            std::path::Path::new(cert_path),
            std::path::Path::new(key_path),
        )?;

        let listener = tokio::net::TcpListener::bind(address).await?;

        let tls_listener = audium_server::tls::TlsListener::new(listener, acceptor)
            .tap_io(|_tls: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>| {});
        info!("Audium Server listening on https://{} (TLS)", address);
        info!("[WebSocket] endpoint ready at /v4/websocket");
        info!(
            "[Metrics] endpoint: {}",
            if config.metrics.prometheus.enabled {
                config.metrics.prometheus.endpoint.as_str()
            } else {
                "disabled"
            }
        );
        info!("Audium startup completed successfully");

        axum::serve(
            tls_listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(address).await?;
        info!("Audium Server listening on http://{}", address);
        info!("[WebSocket] endpoint ready at /v4/websocket");
        info!(
            "[Metrics] endpoint: {}",
            if config.metrics.prometheus.enabled {
                config.metrics.prometheus.endpoint.as_str()
            } else {
                "disabled"
            }
        );
        info!("Audium startup completed successfully");

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    }

    let sessions: Vec<_> = shared_state
        .sessions
        .iter()
        .map(|kv| kv.value().clone())
        .collect();
    for session in sessions {
        session.shutdown().await;
    }
    let resumable: Vec<_> = shared_state
        .resumable_sessions
        .iter()
        .map(|kv| kv.value().clone())
        .collect();
    for session in resumable {
        session.shutdown().await;
    }
    shared_state.sessions.clear();
    shared_state.resumable_sessions.clear();

    info!("Audium Server shut down cleanly");
    Ok(())
}

/// Waits for SIGINT (Ctrl-C) or SIGTERM (container stop) then returns.
///
/// A container runtime sends SIGTERM before SIGKILL after the grace period;
/// acknowledging it lets active HTTP requests and in-flight voice frames drain
/// rather than being cut off mid-request.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received; draining in-flight work");
}
