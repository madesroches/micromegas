use std::sync::Arc;

use anyhow::{Context, Result};
use axum::response::Response;
use axum::routing::post;
use axum::{Extension, Router};
use micromegas_analytics::analytics_service::AnalyticsService;
use micromegas_tracing::prelude::*;

use crate::axum_utils::stream_request;

pub fn bytes_response(result: Result<bytes::Bytes>) -> Response {
    match result {
        Err(e) => {
            error!("Error in request: {e:?}");
            Response::builder()
                .status(500)
                .body(format!("{e:?}").into())
                .unwrap()
        }
        Ok(bytes) => Response::builder().status(200).body(bytes.into()).unwrap(),
    }
}

pub async fn find_process_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .find_process(body)
            .await
            .with_context(|| "find_process"),
    )
}

pub async fn find_stream_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .find_stream(body)
            .await
            .with_context(|| "find_stream"),
    )
}

pub async fn query_streams_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_streams(body)
            .await
            .with_context(|| "query_streams"),
    )
}

pub async fn query_blocks_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_blocks(body)
            .await
            .with_context(|| "query_blocks"),
    )
}

pub async fn query_view_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(service.query_view(body).await.with_context(|| "query_view"))
}

pub async fn query_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(service.query(body).await.with_context(|| "query"))
}

pub async fn query_partitions_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
) -> Response {
    bytes_response(
        service
            .query_partitions()
            .await
            .with_context(|| "query_partitions"),
    )
}

pub async fn materialize_partitions_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    stream_request(|writer| async move {
        service
            .materialize_partition_range(body, writer)
            .await
            .with_context(|| "materialize_partitions")
    })
}

pub async fn retire_partitions_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    stream_request(|writer| async move {
        service
            .retire_partitions(body, writer)
            .await
            .with_context(|| "retire_partitions")
    })
}

pub fn register_routes(router: Router) -> Router {
    router
        .route("/analytics/find_process", post(find_process_request))
        .route("/analytics/find_stream", post(find_stream_request))
        .route("/analytics/query_streams", post(query_streams_request))
        .route("/analytics/query_blocks", post(query_blocks_request))
        .route("/analytics/query_view", post(query_view_request))
        .route("/analytics/query", post(query_request))
        .route(
            "/analytics/query_partitions",
            post(query_partitions_request),
        )
        .route(
            "/analytics/materialize_partitions",
            post(materialize_partitions_request),
        )
        .route(
            "/analytics/retire_partitions",
            post(retire_partitions_request),
        )
}
