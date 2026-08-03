use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use micromegas_auth::db_api_key::DbApiKeyConfig;
use micromegas_auth::types::AuthProvider;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::{IngestionServiceError, WebIngestionService};
use micromegas_tracing::prelude::*;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IngestionError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal server error: {0}")]
    Internal(String),
}

impl IntoResponse for IngestionError {
    fn into_response(self) -> Response<Body> {
        let (status, category, detail) = match self {
            IngestionError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "Bad request", msg),
            IngestionError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error",
                msg,
            ),
        };
        error!("{status}: {detail}");
        (status, category).into_response()
    }
}

impl From<IngestionServiceError> for IngestionError {
    fn from(err: IngestionServiceError) -> Self {
        match err {
            IngestionServiceError::ParseError(msg) => IngestionError::BadRequest(msg),
            IngestionServiceError::DatabaseError(msg) => IngestionError::Internal(msg),
            IngestionServiceError::StorageError(msg) => IngestionError::Internal(msg),
        }
    }
}

/// Handles requests to insert process information.
///
/// Returns 400 for malformed CBOR, 500 for database errors.
pub async fn insert_process_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    service.insert_process(body).await.map_err(Into::into)
}

/// Handles requests to insert stream information.
///
/// Returns 400 for malformed CBOR, 500 for database errors.
pub async fn insert_stream_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    service.insert_stream(body).await.map_err(Into::into)
}

/// Handles requests to insert block information.
///
/// Returns 400 for empty body or malformed CBOR, 500 for database/storage errors.
pub async fn insert_block_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    if body.is_empty() {
        return Err(IngestionError::BadRequest("empty body".to_string()));
    }
    service.insert_block(body).await.map_err(Into::into)
}

async fn ready_handler(Extension(service): Extension<Arc<WebIngestionService>>) -> StatusCode {
    if service.check_ready().await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Registers the ingestion routes with the given Axum `Router`.
///
/// This function adds routes for `/ingestion/insert_process`,
/// `/ingestion/insert_stream`, and `/ingestion/insert_block`.
pub fn register_routes(router: Router) -> Router {
    router
        .route("/ingestion/insert_process", post(insert_process_request))
        .route("/ingestion/insert_stream", post(insert_stream_request))
        .route("/ingestion/insert_block", post(insert_block_request))
}

/// Assemble and serve the HTTP ingestion endpoint.
///
/// Binds `listen_addr`, wires the ingestion routes + OTLP routes, applies
/// the supplied `auth_provider` (or runs open when `None`), and shuts down
/// gracefully when `shutdown` resolves.
///
/// Thin wrapper around [`serve_ingestion_with_api_key_config`], mirroring the
/// `ProviderBuilder`/`provider()` split in `micromegas_auth::default_provider`:
/// this keeps `serve_ingestion`'s published signature (the `server`-feature
/// crate) unchanged. Uses `DbApiKeyConfig::from_env_with_prefix("")` — the
/// unprefixed default, correct for `telemetry-ingestion-srv`.
pub async fn serve_ingestion(
    listen_addr: SocketAddr,
    lake: DataLakeConnection,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> anyhow::Result<()> {
    serve_ingestion_with_api_key_config(
        listen_addr,
        lake,
        auth_provider,
        shutdown,
        grace,
        DbApiKeyConfig::from_env_with_prefix(""),
    )
    .await
}

/// Like [`serve_ingestion`], but also takes the [`DbApiKeyConfig`] the caller's
/// auth provider was built with, so the key-management routes'
/// `effective_within_seconds` (in the `DELETE` response) matches the TTL the
/// running provider actually uses. The caller must build this with the same
/// prefix it gave `ProviderBuilder::with_db_key_store` (empty for
/// `telemetry-ingestion-srv`, `MICROMEGAS_INGESTION` for the monolith) so the
/// two cannot silently disagree.
pub async fn serve_ingestion_with_api_key_config(
    listen_addr: SocketAddr,
    lake: DataLakeConnection,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
    api_key_config: DbApiKeyConfig,
) -> anyhow::Result<()> {
    use axum::extract::DefaultBodyLimit;
    use axum::middleware;
    use axum::routing::get;
    use micromegas_auth::axum::auth_middleware;
    use tower_http::limit::RequestBodyLimitLayer;

    use super::axum_utils::observability_middleware;
    use super::shutdown::serve_axum_with_graceful_shutdown;

    let key_store_pool = lake.db_pool.clone();
    let service = Arc::new(WebIngestionService::new(lake));

    let health_router = Router::new()
        .route("/health", get(|| async { axum::http::StatusCode::OK }))
        .route("/ready", get(ready_handler))
        .layer(Extension(service.clone()));

    let firehose_auth = auth_provider.clone();
    let cw_logs_firehose_auth = auth_provider.clone();

    let mut protected_app = register_routes(Router::new())
        .merge(super::otlp::otlp_router())
        .merge(super::webhook::webhook_router())
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .layer(Extension(service.clone()));

    let auth_enabled = auth_provider.is_some();
    if let Some(provider) = auth_provider {
        info!("Ingestion: authentication enabled");
        // Merged before the auth_middleware layer below, so it reuses the same
        // middleware rather than re-implementing auth: handlers read
        // `Extension<AuthContext>`, inserted by `auth_middleware`.
        protected_app = protected_app.merge(super::api_keys::api_keys_router(
            key_store_pool,
            api_key_config,
        ));
        protected_app = protected_app.layer(middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));
    } else {
        // With --disable-auth there is no AuthContext in extensions and the
        // key-management handlers' Extension<AuthContext> extractor would 500 —
        // there is nothing to authenticate in that mode, so skip registration
        // entirely rather than exposing a route that always fails.
        warn!(
            "Ingestion: authentication disabled — development mode only; /auth/api_keys routes not registered"
        );
    }

    // The Firehose routes carry their own auth (Firehose can only send its credential via
    // X-Amz-Firehose-Access-Key, not Authorization: Bearer), so they are merged outside
    // protected_app and never hit the global Bearer auth_middleware.
    let firehose_app = super::firehose::firehose_router(service.clone(), firehose_auth);
    let cw_logs_firehose_app =
        super::firehose_cloudwatch_logs::firehose_router(service.clone(), cw_logs_firehose_auth);

    let app = health_router
        .merge(protected_app)
        .merge(firehose_app)
        .merge(cw_logs_firehose_app)
        .layer(middleware::from_fn(observability_middleware));

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .map_err(|e| anyhow::anyhow!("ingestion: binding to {listen_addr}: {e}"))?;
    info!("Ingestion serving on {listen_addr} authentication={auth_enabled}");

    serve_axum_with_graceful_shutdown(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
        shutdown,
        grace,
    )
    .await?;

    Ok(())
}
