use axum::routing::post;
use axum::Extension;
use axum::Router;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Handles requests to insert process information.
pub async fn insert_process_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) {
    if let Err(e) = service.insert_process(body).await {
        error!("Error in insert_process_request: {:?}", e);
    }
}

/// Handles requests to insert stream information.
pub async fn insert_stream_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) {
    if let Err(e) = service.insert_stream(body).await {
        error!("Error in insert_stream_request: {:?}", e);
    }
}

/// Handles requests to insert block information.
pub async fn insert_block_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    body: bytes::Bytes,
) {
    if body.is_empty() {
        error!("insert_block_request: empty body");
        return;
    }
    if let Err(e) = service.insert_block(body).await {
        error!("Error in insert_block_request: {:?}", e);
    }
}

/// Registers the ingestion routes with the given Axum `Router`.
pub fn register_routes(router: Router) -> Router {
    router
        .route("/ingestion/insert_process", post(insert_process_request))
        .route("/ingestion/insert_stream", post(insert_stream_request))
        .route("/ingestion/insert_block", post(insert_block_request))
}
