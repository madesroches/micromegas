//! Kinesis Data Firehose HTTP Endpoint Delivery route for `telemetry-ingestion-srv`.
//!
//! Exposes `POST /ingestion/otlp/v1/metrics/firehose` so a CloudWatch Metric Stream can
//! push metrics into micromegas as **Metric Stream → Firehose → micromegas**, with no
//! Lambda, no Kinesis Data Stream, and no collector process in between. Firehose is a
//! dumb managed pipe: it wraps each delivered record (in OpenTelemetry 1.0.0 output mode,
//! an OTLP `ExportMetricsServiceRequest` protobuf) in a small JSON envelope and expects a
//! fixed ack shape back.
//!
//! Shared Firehose transport plumbing (auth, ack shape, request-id parsing) lives in
//! `firehose_common` — this module only knows about the metrics-specific decode/ingest
//! calls.
//!
//! Once a record's bytes are extracted from the envelope, they are the exact protobuf the
//! existing `handler::ingest_metrics` already handles — no new identity, block, split, or
//! write logic.

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
use micromegas_otel_ingestion::{Signal, handler};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

async fn firehose_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let mut request_id = request_id_from(&headers);
    let envelope = match handler::decode_firehose_envelope(&body, Signal::Metrics) {
        Ok(e) => e,
        Err(err) => {
            error!("firehose decode error: {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return firehose_response(status, &request_id, Some(&err.public_message()));
        }
    };
    if request_id.is_empty() {
        request_id = envelope.request_id.clone(); // header preferred; body requestId is fallback
    }
    match handler::ingest_firehose_metrics(service, envelope.records).await {
        Ok(()) => firehose_response(StatusCode::OK, &request_id, None),
        Err(err) => {
            error!("firehose ingest error: {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            firehose_response(status, &request_id, Some(&err.public_message()))
        }
    }
}

/// Builds the Firehose sub-router: route + service extension + optional Firehose-auth
/// layer + shared ingestion body limits (gzip + 20 MiB wire / 300 MiB decompressed).
///
/// Deliberately not merged into `protected_app` — it must not sit under the global Bearer
/// `auth_middleware`, since Firehose can only send its credential via
/// `X-Amz-Firehose-Access-Key`. Auth is applied only when `auth_provider` is `Some`,
/// matching every other ingestion route's dev-mode-open behavior.
pub fn firehose_router(
    service: Arc<WebIngestionService>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
) -> Router {
    let mut router = Router::new()
        .route(
            "/ingestion/otlp/v1/metrics/firehose",
            post(firehose_handler),
        )
        .layer(Extension(service));
    if let Some(provider) = auth_provider {
        router = router.layer(middleware::from_fn(move |req, next| {
            firehose_auth_middleware(provider.clone(), req, next)
        }));
    }
    apply_ingestion_body_limits(router)
}
