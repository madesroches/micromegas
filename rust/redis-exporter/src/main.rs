//! Redis metrics exporter for Micromegas
//!
//! Samples one Redis server over a persistent connection and emits
//! `redis_*` metrics through the telemetry sink.
//!
//! Env vars:
//!  - `MICROMEGAS_TELEMETRY_URL` : where to send the metrics (unset = local sink only)
//!  - `MICROMEGAS_INGESTION_API_KEY` or `MICROMEGAS_OIDC_TOKEN_ENDPOINT`/`MICROMEGAS_OIDC_CLIENT_ID`/`MICROMEGAS_OIDC_CLIENT_SECRET` : ingestion auth
//!  - `MICROMEGAS_REDIS_EXPORTER_REDIS_URL` : redis:// or rediss:// URL (default redis://127.0.0.1:6379)
//!  - `MICROMEGAS_REDIS_EXPORTER_REDIS_PASSWORD` : overrides any password in the URL
//!  - `MICROMEGAS_REDIS_EXPORTER_METRICS` : core | extended | full (default full)
//!  - `MICROMEGAS_REDIS_EXPORTER_SAMPLE_INTERVAL_SECONDS` : default 1
//!  - `MICROMEGAS_REDIS_EXPORTER_TARGET_NAME` : instance name (default host:port)
//!  - `MICROMEGAS_REDIS_EXPORTER_PROPERTIES` : comma-separated key=value pairs added to every metric
//!  - `MICROMEGAS_PROCESS_PROPERTIES` : comma-separated key=value pairs tagging the exporter
//!    process itself (queryable as `process_properties`); prefer it for tags that never change
//!  - `MICROMEGAS_REDIS_EXPORTER_HEALTH_LISTEN_ADDR` : optional /health + /ready listen address

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
micromegas::declare_jemalloc_conf!();

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;
use redis_exporter::cli::Cli;
use redis_exporter::sampler::{build_base_properties, sample_once};

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let password = std::env::var("MICROMEGAS_REDIS_EXPORTER_REDIS_PASSWORD").ok();
    let config = args.into_config(password)?;
    info!(
        "redis exporter starting: target={} preset={:?} interval={}s properties={:?}",
        config.target_name,
        config.preset,
        config.sample_interval.as_secs(),
        config.properties
    );

    let ready = Arc::new(AtomicBool::new(false));
    if let Some(addr) = config.health_listen_addr {
        serve_health_sidecar(addr, ready.clone()).await?;
    }

    let props = build_base_properties(&config.target_name, &config.properties);
    let mut shutdown = Box::pin(wait_for_shutdown());

    // Constraint: redis-rs's ConnectionManager must never block a tick for
    // longer than this. Without it, connecting to a blackholed address hangs
    // for the OS TCP connect timeout (~1-2 min) and a query on a half-open
    // connection can stall for TCP retransmission timescales — during which
    // no redis_up=0 would be emitted, breaking the "unreachable -> redis_up=0
    // every tick" contract. A few seconds is generous for a healthy LAN/DC
    // link and short enough to keep the per-tick contract; it applies to
    // both the initial connect and every query made through the manager.
    //
    // `number_of_retries` is set to 0 (default is 6, with exponential
    // backoff) so a failed connect returns after a single bounded attempt
    // instead of retrying internally for tens of seconds: our own loop
    // already provides the "retry every tick" behavior, one bounded attempt
    // per tick at a time.
    let redis_io_timeout = Duration::from_secs(5);
    let manager_config = redis::aio::ConnectionManagerConfig::new()
        .set_connection_timeout(Some(redis_io_timeout))
        .set_response_timeout(Some(redis_io_timeout))
        .set_number_of_retries(0);

    // Persistent connection: one ConnectionManager for the process
    // lifetime, reconnecting on its own after drops. Creation needs an
    // initial connection, so retry here — the exporter must start (and
    // report redis_up=0) even while Redis is down.
    ready.store(true, Ordering::Relaxed);
    let client =
        redis::Client::open(config.connection_info.clone()).context("opening redis client")?;
    let mut conn: Option<redis::aio::ConnectionManager> = None;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(config.sample_interval) => {}
            _ = &mut shutdown => {
                info!("shutdown signal received, exiting");
                return Ok(());
            }
        }
        if conn.is_none() {
            tokio::select! {
                result = redis::aio::ConnectionManager::new_with_config(client.clone(), manager_config.clone()) => {
                    match result {
                        Ok(manager) => {
                            info!("connected to redis at {}", config.target_name);
                            conn = Some(manager);
                        }
                        Err(e) => {
                            warn!("redis connection failed: {e:#}");
                            imetric!("redis_up", "count", props, 0u64);
                            continue;
                        }
                    }
                }
                _ = &mut shutdown => {
                    info!("shutdown signal received, exiting");
                    return Ok(());
                }
            }
        }
        let start = Instant::now();
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_secs();
        let manager = conn.as_mut().expect("connection manager set above");
        tokio::select! {
            result = sample_once(manager, config.preset, props, now_unix_secs) => {
                match result {
                    Ok(()) => {
                        imetric!("redis_up", "count", props, 1u64);
                    }
                    Err(e) => {
                        // Don't rely on the ConnectionManager's own background
                        // reconnect: with number_of_retries(0) it has been
                        // observed to wedge permanently after a mid-run
                        // outage (its internal shared-future swap never
                        // recovers, and no further errors or connects are
                        // ever attempted again). Drop it and let the top of
                        // the loop rebuild a fresh manager next tick through
                        // our own bounded connect path instead.
                        warn!("redis sample failed: {e:#}");
                        imetric!("redis_up", "count", props, 0u64);
                        conn = None;
                    }
                }
                fmetric!(
                    "redis_scrape_duration_ms",
                    "ms",
                    props,
                    start.elapsed().as_secs_f64() * 1000.0
                );
            }
            _ = &mut shutdown => {
                info!("shutdown signal received, exiting");
                return Ok(());
            }
        }
    }
}

/// Opt-in k8s-style probe endpoints, following the flight-sql-srv health
/// sidecar precedent. `/health` = process alive; `/ready` = sampling loop
/// started. A Redis outage does NOT flip readiness: the exporter's job
/// during an outage is to keep reporting redis_up=0.
async fn serve_health_sidecar(addr: SocketAddr, ready: Arc<AtomicBool>) -> Result<()> {
    use axum::Extension;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;

    async fn ready_handler(Extension(ready): Extension<Arc<AtomicBool>>) -> StatusCode {
        if ready.load(Ordering::Relaxed) {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding health sidecar to {addr}"))?;
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/ready", get(ready_handler))
        .layer(Extension(ready));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("health sidecar stopped: {e:#}");
        }
    });
    info!("health sidecar listening on {addr}");
    Ok(())
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("installing SIGTERM handler");
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("installing ctrl-c handler");
    }
}
