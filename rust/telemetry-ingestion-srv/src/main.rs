//! Telemetry Ingestion Server
//!
//! Accepts telemetry data through http, stores the metadata in postgresql and the
//! raw event payload in the object store.
//!
//! Env variables:
//!  - `MICROMEGAS_SQL_CONNECTION_STRING` : to connect to postgresql
//!  - `MICROMEGAS_OBJECT_STORE_URI` : to write the payloads
//!  - `MICROMEGAS_API_KEYS` : (optional) JSON array of API keys
//!  - `MICROMEGAS_OIDC_CONFIG` : (optional) OIDC configuration JSON

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::{Context, Result};
use axum::Extension;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use clap::Parser;
use micromegas::auth::axum::auth_middleware;
use micromegas::auth::types::AuthProvider;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::ingestion::remote_data_lake::connect_to_remote_data_lake;
use micromegas::ingestion::web_ingestion_service::WebIngestionService;
use micromegas::micromegas_main;
use micromegas::servers;
use micromegas::servers::axum_utils::observability_middleware;
use micromegas::servers::shutdown::{serve_axum_with_graceful_shutdown, wait_for_sigterm};
use micromegas::tracing::prelude::*;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::limit::RequestBodyLimitLayer;

#[derive(Parser, Debug)]
#[clap(name = "Telemetry Ingestion Server")]
#[clap(about = "Telemetry Ingestion Server", version, author)]
struct Cli {
    #[clap(long, default_value = "127.0.0.1:8081")]
    listen_endpoint_http: SocketAddr,

    /// Disable authentication (development mode only)
    #[clap(long)]
    disable_auth: bool,

    /// Seconds to wait for in-flight requests to complete after SIGTERM
    #[clap(long, default_value = "25")]
    shutdown_grace_period_seconds: u64,
}

/// Serves the HTTP ingestion service.
///
/// This function sets up the Axum router, applies middleware, and starts the HTTP server.
async fn serve_http(
    args: &Cli,
    lake: DataLakeConnection,
    auth_provider: Option<Arc<dyn AuthProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use axum::routing::get;

    let service = Arc::new(WebIngestionService::new(lake));

    // Health check endpoint (no auth required)
    let health_router =
        Router::new().route("/health", get(|| async { axum::http::StatusCode::OK }));

    // Protected routes (require auth)
    //
    // OTLP routes ride on a separate sub-Router that carries its own 20 MiB body limit
    // plus gzip request decompression. We `.merge()` it BEFORE applying the outer
    // 100 MiB limit so per-route layers stay scoped — the outer limit applies to
    // `/ingestion/insert_block` and friends; OTLP routes keep the tighter cap.
    let mut protected_app = servers::ingestion::register_routes(Router::new())
        .merge(servers::otlp::otlp_router())
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .layer(Extension(service));

    // Add authentication middleware if enabled
    let auth_enabled = auth_provider.is_some();
    if let Some(provider) = auth_provider {
        info!("Authentication enabled - API key and/or OIDC");
        protected_app = protected_app.layer(middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));
    } else {
        warn!("Authentication disabled - development mode only!");
    }

    // Merge health check (public) with protected routes
    let mut app = health_router.merge(protected_app);

    // Add observability middleware last (outer layer)
    app = app.layer(middleware::from_fn(observability_middleware));

    let listener = tokio::net::TcpListener::bind(args.listen_endpoint_http)
        .await
        .with_context(|| format!("binding to {}", args.listen_endpoint_http))?;
    info!(
        "serving on {} with authentication={}",
        args.listen_endpoint_http, auth_enabled
    );
    let grace = Duration::from_secs(args.shutdown_grace_period_seconds);
    serve_axum_with_graceful_shutdown(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
        wait_for_sigterm(),
        grace,
    )
    .await?;

    Ok(())
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let data_lake = connect_to_remote_data_lake(&connection_string, &object_store_uri).await?;

    // Initialize authentication providers (same pattern as flight-sql-srv)
    let auth_required = !args.disable_auth;
    let auth_provider: Option<Arc<dyn AuthProvider>> = if auth_required {
        match micromegas::auth::default_provider::provider().await? {
            Some(provider) => Some(provider),
            None => {
                return Err("Authentication required but no auth providers configured. \
                     Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG, \
                     or use --disable-auth for development"
                    .into());
            }
        }
    } else {
        info!("Authentication disabled (--disable_auth)");
        None
    };

    serve_http(&args, data_lake, auth_provider).await?;
    Ok(())
}
