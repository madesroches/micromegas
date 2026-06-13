use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
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
pub async fn serve_ingestion(
    listen_addr: SocketAddr,
    lake: DataLakeConnection,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> anyhow::Result<()> {
    use axum::extract::DefaultBodyLimit;
    use axum::middleware;
    use axum::routing::get;
    use micromegas_auth::axum::auth_middleware;
    use tower_http::limit::RequestBodyLimitLayer;

    use super::axum_utils::observability_middleware;
    use super::shutdown::serve_axum_with_graceful_shutdown;

    let service = Arc::new(WebIngestionService::new(lake));

    let health_router =
        Router::new().route("/health", get(|| async { axum::http::StatusCode::OK }));

    let mut protected_app = register_routes(Router::new())
        .merge(super::otlp::otlp_router())
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .layer(Extension(service));

    let auth_enabled = auth_provider.is_some();
    if let Some(provider) = auth_provider {
        info!("Ingestion: authentication enabled");
        protected_app = protected_app.layer(middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));
    } else {
        warn!("Ingestion: authentication disabled — development mode only");
    }

    let app = health_router
        .merge(protected_app)
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
