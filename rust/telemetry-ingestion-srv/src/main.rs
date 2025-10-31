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

use anyhow::{Context, Result};
use axum::Extension;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use clap::Parser;
use micromegas::ingestion::data_lake_connection::DataLakeConnection;
use micromegas::ingestion::remote_data_lake::connect_to_remote_data_lake;
use micromegas::ingestion::web_ingestion_service::WebIngestionService;
use micromegas::micromegas_main;
use micromegas::servers;
use micromegas::servers::axum_utils::observability_middleware;
use micromegas::tracing::prelude::*;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::axum::auth_middleware;
use micromegas_auth::multi::MultiAuthProvider;
use micromegas_auth::oidc::{OidcAuthProvider, OidcConfig};
use micromegas_auth::types::AuthProvider;
use std::net::SocketAddr;
use std::sync::Arc;
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
    let mut protected_app = servers::ingestion::register_routes(Router::new())
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
        .unwrap();
    info!(
        "serving on {} with authentication={}",
        args.listen_endpoint_http, auth_enabled
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();

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
        // Initialize API key provider if configured
        let api_key_provider = match std::env::var("MICROMEGAS_API_KEYS") {
            Ok(keys_json) => {
                let keyring = parse_key_ring(&keys_json)?;
                info!("API key authentication enabled");
                Some(Arc::new(ApiKeyAuthProvider::new(keyring)))
            }
            Err(_) => {
                info!("MICROMEGAS_API_KEYS not set - API key auth disabled");
                None
            }
        };

        // Initialize OIDC provider if configured
        let oidc_provider = match OidcConfig::from_env() {
            Ok(config) => {
                info!("Initializing OIDC authentication");
                Some(Arc::new(OidcAuthProvider::new(config).await?))
            }
            Err(e) => {
                info!("OIDC not configured ({e}) - OIDC auth disabled");
                None
            }
        };

        // Create multi-provider if either is configured
        if api_key_provider.is_some() || oidc_provider.is_some() {
            Some(Arc::new(MultiAuthProvider {
                api_key_provider,
                oidc_provider,
            }) as Arc<dyn AuthProvider>)
        } else {
            return Err("Authentication required but no auth providers configured. \
                 Set MICROMEGAS_API_KEYS or MICROMEGAS_OIDC_CONFIG, \
                 or use --disable-auth for development"
                .into());
        }
    } else {
        info!("Authentication disabled (--disable_auth)");
        None
    };

    serve_http(&args, data_lake, auth_provider).await?;
    Ok(())
}
