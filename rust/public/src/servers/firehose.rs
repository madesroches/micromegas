//! Kinesis Data Firehose HTTP Endpoint Delivery route for `telemetry-ingestion-srv`.
//!
//! Exposes `POST /ingestion/otlp/v1/metrics/firehose` so a CloudWatch Metric Stream can
//! push metrics into micromegas as **Metric Stream → Firehose → micromegas**, with no
//! Lambda, no Kinesis Data Stream, and no collector process in between. Firehose is a
//! dumb managed pipe: it wraps each delivered record (in OpenTelemetry 1.0.0 output mode,
//! one-or-more length-delimited OTLP `ExportMetricsServiceRequest` protobuf messages) in
//! a small JSON envelope and expects a fixed ack shape back.
//!
//! Shared Firehose transport plumbing (auth, ack shape, request-id parsing) lives in
//! `firehose_common` — this module only knows about the metrics-specific decode/ingest
//! calls.
//!
//! Once a record's bytes are extracted from the envelope, `handler::ingest_firehose_metrics`
//! decodes each length-delimited message in turn, runs it through
//! `micromegas_otel_ingestion::cloudwatch_metrics::rewrite_cloudwatch_metric_streams` (a
//! CloudWatch-specific resource rewrite that partitions a matching degenerate resource into
//! one process per CloudWatch namespace — see that module's docs), and reuses the existing
//! split/write logic per message.

use super::firehose_common::{firehose_auth_middleware, firehose_response, request_id_from};
use super::ingestion_limits::apply_ingestion_body_limits;
use super::write_audience::{StampingConfig, resolve_write_audience};
use axum::Extension;
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware;
use axum::response::Response;
use axum::routing::post;
use micromegas_auth::types::{AuthContext, AuthProvider};
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_otel_ingestion::{Signal, handler};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

async fn firehose_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let request_id_header = request_id_from(&headers);

    // AbAC Stage 5 (#1373, §5): resolved from the `AuthContext` `firehose_auth_middleware` now
    // inserts (it used to discard it) -- gated before decoding, so a rejected delivery costs no
    // decode work. Rendered through the Firehose ack shape (non-2xx + `errorMessage`), not a
    // clean 403: Firehose doesn't distinguish 4xx from 5xx, so this is a retry-then-spill, not
    // an immediate rejection.
    let audience = match resolve_write_audience(ctx.as_ref().map(|Extension(c)| c), &stamping) {
        Ok(a) => a,
        Err(_) => {
            return firehose_response(
                StatusCode::FORBIDDEN,
                &request_id_header,
                Some("write audience required"),
            );
        }
    };

    let mut request_id = request_id_header;
    let envelope = match handler::decode_firehose_envelope(&body, Signal::Metrics) {
        Ok(e) => e,
        Err(err) => {
            error!("firehose decode error (request_id={request_id}): {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return firehose_response(status, &request_id, Some(&err.public_message()));
        }
    };
    if request_id.is_empty() {
        request_id = envelope.request_id.clone(); // header preferred; body requestId is fallback
    }
    match handler::ingest_firehose_metrics(service, envelope.records, &audience).await {
        Ok(()) => firehose_response(StatusCode::OK, &request_id, None),
        Err(err) => {
            error!("firehose ingest error (request_id={request_id}): {err}");
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            firehose_response(status, &request_id, Some(&err.public_message()))
        }
    }
}

/// Builds the Firehose sub-router: route + service/stamping extensions + optional Firehose-auth
/// layer + shared ingestion body limits (gzip + 20 MiB wire / 300 MiB decompressed).
///
/// Deliberately not merged into `protected_app` — it must not sit under the global Bearer
/// `auth_middleware`, since Firehose can only send its credential via
/// `X-Amz-Firehose-Access-Key`. Auth is applied only when `auth_provider` is `Some`,
/// matching every other ingestion route's dev-mode-open behavior.
///
/// `stamping` is an explicit parameter, layered here rather than relying on an ambient
/// `Extension<Arc<StampingConfig>>` from a parent router (AbAC Stage 5, #1373, §5): this router
/// is built directly, with no parent extension, both by `serve_ingestion` and by every existing
/// `rust/public/tests/firehose_tests.rs` call site -- a required ambient extractor would 500 at
/// those sites with no compiler signal, the same reasoning that already applies to
/// `auth_provider` here.
pub fn firehose_router(
    service: Arc<WebIngestionService>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    stamping: Arc<StampingConfig>,
) -> Router {
    let mut router = Router::new()
        .route(
            "/ingestion/otlp/v1/metrics/firehose",
            post(firehose_handler),
        )
        .layer(Extension(service))
        .layer(Extension(stamping));
    if let Some(provider) = auth_provider {
        router = router.layer(middleware::from_fn(move |req, next| {
            firehose_auth_middleware(provider.clone(), req, next)
        }));
    }
    apply_ingestion_body_limits(router)
}
