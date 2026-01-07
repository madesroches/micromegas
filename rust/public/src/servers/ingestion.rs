use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use micromegas_ingestion::web_ingestion_service::{IngestionServiceError, WebIngestionService};
use micromegas_tracing::prelude::*;
use std::sync::Arc;
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
        let (status, message) = match self {
            IngestionError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            IngestionError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        error!("{status}: {message}");
        (status, message).into_response()
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
/// Returns 200 OK on success, 400 Bad Request for malformed input,
/// or 500 Internal Server Error for database failures.
pub async fn insert_process_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    service.insert_process(body).await.map_err(Into::into)
}

/// Handles requests to insert stream information.
///
/// Returns 200 OK on success, 400 Bad Request for malformed input,
/// or 500 Internal Server Error for database failures.
pub async fn insert_stream_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    service.insert_stream(body).await.map_err(Into::into)
}

/// Handles requests to insert block information.
///
/// Returns 200 OK on success, 400 Bad Request for malformed input or empty body,
/// or 500 Internal Server Error for database/storage failures.
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
