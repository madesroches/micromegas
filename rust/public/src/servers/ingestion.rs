use super::write_audience::{StampingConfig, resolve_write_audience};
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use micromegas_auth::types::{AuthContext, AuthProvider};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::{IngestionServiceError, WebIngestionService};
use micromegas_ingestion::write_audience::WriteAudience;
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

    /// The write audience gate rejected this request (AbAC Stage 5, #1373, §5/§6): either
    /// `REQUIRE_WRITE_AUDIENCE` is set and the credential carries no audience, or this is a
    /// conflicting `insert_process` re-registration under a different audience. Maps to 403.
    #[error("Forbidden: {0}")]
    Forbidden(String),
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
            IngestionError::Forbidden(msg) => (StatusCode::FORBIDDEN, "Forbidden", msg),
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
            IngestionServiceError::AudienceConflict { .. } => {
                IngestionError::Forbidden(err.to_string())
            }
        }
    }
}

/// Resolves the write audience for a native-route request, mapping a gate rejection onto
/// [`IngestionError::Forbidden`] with a sanitized body (no internal detail).
fn resolve_native_write_audience(
    ctx: &Option<Extension<AuthContext>>,
    stamping: &StampingConfig,
) -> Result<WriteAudience, IngestionError> {
    resolve_write_audience(ctx.as_ref(), stamping)
        .map_err(|_| IngestionError::Forbidden("write audience required".to_string()))
}

/// Handles requests to insert process information.
///
/// Returns 403 when the write audience gate rejects the request (§5/§6), 400 for malformed
/// CBOR, 500 for database errors.
pub async fn insert_process_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    let audience = resolve_native_write_audience(&ctx, &stamping)?;
    service
        .insert_process(body, &audience)
        .await
        .map_err(Into::into)
}

/// Handles requests to insert stream information.
///
/// Returns 403 when the write audience gate rejects the request (§5), 400 for malformed CBOR,
/// 500 for database errors.
pub async fn insert_stream_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    resolve_native_write_audience(&ctx, &stamping)?;
    service.insert_stream(body).await.map_err(Into::into)
}

/// Handles requests to insert block information.
///
/// Returns 403 when the write audience gate rejects the request (§5), 400 for empty body or
/// malformed CBOR, 500 for database/storage errors.
pub async fn insert_block_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    resolve_native_write_audience(&ctx, &stamping)?;
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
/// Ingestion exposes no key-management HTTP surface of its own (#1458) — keys
/// for both `ingestion_api_keys` and `analytics_api_keys` are administered
/// exclusively through `analytics-web-srv`'s own routes. Ingestion still
/// *validates* incoming API keys via whichever `auth_provider` it was built
/// with (including a `DbApiKeyAuthProvider`, unaffected by this), it just no
/// longer exposes a way to mint/list/revoke/import them.
pub async fn serve_ingestion(
    listen_addr: SocketAddr,
    lake: DataLakeConnection,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    stamping: StampingConfig,
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
    let stamping = Arc::new(stamping);

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
        .layer(Extension(service.clone()))
        .layer(Extension(stamping.clone()));

    let auth_enabled = auth_provider.is_some();
    if let Some(provider) = auth_provider {
        info!("Ingestion: authentication enabled");
        protected_app = protected_app.layer(middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));
    } else {
        warn!("Ingestion: authentication disabled — development mode only");
    }

    // The Firehose routes carry their own auth (Firehose can only send its credential via
    // X-Amz-Firehose-Access-Key, not Authorization: Bearer), so they are merged outside
    // protected_app and never hit the global Bearer auth_middleware. They also take
    // `stamping` as an explicit parameter rather than an ambient extension, since they're
    // built directly (with no parent extension) here and by every test call site.
    let firehose_app =
        super::firehose::firehose_router(service.clone(), firehose_auth, stamping.clone());
    let cw_logs_firehose_app = super::firehose_cloudwatch_logs::firehose_router(
        service.clone(),
        cw_logs_firehose_auth,
        stamping.clone(),
    );

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
