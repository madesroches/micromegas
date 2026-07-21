//! Kinesis Data Firehose HTTP Endpoint Delivery route for CloudWatch Logs subscription
//! filters.
//!
//! Exposes `POST /ingestion/cloudwatch/v1/logs/firehose` so CloudWatch Logs can push logs
//! into micromegas as **CloudWatch Logs → subscription filter → Firehose → micromegas**,
//! with no intermediate consumer. Unlike the metrics Firehose route, CloudWatch Logs
//! subscription-filter delivery has exactly one proprietary record format — there is no
//! OTLP framing on the wire, only in how micromegas happens to store the result. The route
//! is therefore named `cloudwatch/...` rather than `otlp/...`, to avoid misleadingly
//! implying the client sends OTLP.
//!
//! Shared Firehose transport plumbing (auth, ack shape, request-id parsing) lives in
//! `firehose_common`.

use super::firehose_common::{firehose_auth_middleware, firehose_response, request_id_from};
use super::ingestion_limits::apply_ingestion_body_limits;
use axum::Extension;
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware;
use axum::response::Response;
use axum::routing::post;
use micromegas_auth::types::AuthProvider;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_otel_ingestion::{Signal, cloudwatch_logs, handler};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

async fn cloudwatch_logs_firehose_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let mut request_id = request_id_from(&headers);
    let envelope = match handler::decode_firehose_envelope(&body, Signal::Logs) {
        Ok(e) => e,
        Err(err) => {
            error!("cloudwatch logs firehose decode error: {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return firehose_response(status, &request_id, Some(&err.public_message()));
        }
    };
    if request_id.is_empty() {
        request_id = envelope.request_id.clone();
    }
    match cloudwatch_logs::ingest_cloudwatch_logs_firehose(service, envelope.records).await {
        Ok(()) => firehose_response(StatusCode::OK, &request_id, None),
        Err(err) => {
            error!("cloudwatch logs firehose ingest error: {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            firehose_response(status, &request_id, Some(&err.public_message()))
        }
    }
}

/// Builds the CloudWatch Logs Firehose sub-router: route + service extension + optional
/// Firehose-auth layer + shared ingestion body limits (gzip + 20 MiB wire / 300 MiB
/// decompressed).
///
/// Deliberately not merged into `protected_app` — same reasoning as the metrics Firehose
/// route: Firehose can only send its credential via `X-Amz-Firehose-Access-Key`, not
/// `Authorization: Bearer`. Auth is applied only when `auth_provider` is `Some`, matching
/// every other ingestion route's dev-mode-open behavior.
pub fn firehose_router(
    service: Arc<WebIngestionService>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
) -> Router {
    let mut router = Router::new()
        .route(
            "/ingestion/cloudwatch/v1/logs/firehose",
            post(cloudwatch_logs_firehose_handler),
        )
        .layer(Extension(service));
    if let Some(provider) = auth_provider {
        router = router.layer(middleware::from_fn(move |req, next| {
            firehose_auth_middleware(provider.clone(), req, next)
        }));
    }
    apply_ingestion_body_limits(router)
}
