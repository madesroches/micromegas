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

pub async fn query_processes_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_processes(body)
            .await
            .with_context(|| "query_processes"),
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

pub async fn query_spans_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_spans(body)
            .await
            .with_context(|| "query_spans"),
    )
}

pub async fn query_thread_events_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_thread_events(body)
            .await
            .with_context(|| "query_thread_events"),
    )
}

pub async fn query_log_entries_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_log_entries(body)
            .await
            .with_context(|| "query_log_entries"),
    )
}

pub async fn query_metrics_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(
        service
            .query_metrics(body)
            .await
            .with_context(|| "query_metrics"),
    )
}

pub async fn query_view_request(
    Extension(service): Extension<Arc<AnalyticsService>>,
    body: bytes::Bytes,
) -> Response {
    bytes_response(service.query_view(body).await.with_context(|| "query_view"))
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
        .route("/analytics/query_processes", post(query_processes_request))
        .route("/analytics/query_streams", post(query_streams_request))
        .route("/analytics/query_blocks", post(query_blocks_request))
        .route("/analytics/query_spans", post(query_spans_request))
        .route(
            "/analytics/query_log_entries",
            post(query_log_entries_request),
        )
        .route("/analytics/query_metrics", post(query_metrics_request))
        .route("/analytics/query_view", post(query_view_request))
        .route(
            "/analytics/query_thread_events",
            post(query_thread_events_request),
        )
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
