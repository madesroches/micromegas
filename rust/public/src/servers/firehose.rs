//! Kinesis Data Firehose HTTP Endpoint Delivery route for `telemetry-ingestion-srv`.
//!
//! Exposes `POST /ingestion/otlp/v1/metrics/firehose` so a CloudWatch Metric Stream can
//! push metrics into micromegas as **Metric Stream → Firehose → micromegas**, with no
//! Lambda, no Kinesis Data Stream, and no collector process in between. Firehose is a
//! dumb managed pipe: it wraps each delivered record (in OpenTelemetry 1.0.0 output mode,
//! an OTLP `ExportMetricsServiceRequest` protobuf) in a small JSON envelope and expects a
//! fixed ack shape back.
//!
//! Firehose's only credential channel is the non-standard `X-Amz-Firehose-Access-Key`
//! header — it cannot send `Authorization: Bearer`. So this route cannot sit under the
//! global Bearer `auth_middleware`; it has its own auth step that synthesizes a bearer
//! header from the Firehose header and reuses the same `AuthProvider` (constant-time
//! keyring check) verbatim.
//!
//! Once a record's bytes are extracted from the envelope, they are the exact protobuf the
//! existing `handler::ingest_metrics` already handles — no new identity, block, split, or
//! write logic.

use super::ingestion_limits::apply_ingestion_body_limits;
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::post;
use chrono::Utc;
use micromegas_auth::types::{AuthProvider, HttpRequestParts, RequestParts};
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_otel_ingestion::handler;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

const HEADER_ACCESS_KEY: &str = "X-Amz-Firehose-Access-Key";
const HEADER_REQUEST_ID: &str = "X-Amz-Firehose-Request-Id";

/// Ack/error response body per the Firehose HTTP Endpoint Delivery contract:
/// `{requestId, timestamp}` on success, `{requestId, timestamp, errorMessage}` on failure.
#[derive(serde::Serialize)]
struct FirehoseResponseBody<'a> {
    #[serde(rename = "requestId")]
    request_id: &'a str,
    timestamp: i64,
    #[serde(rename = "errorMessage", skip_serializing_if = "Option::is_none")]
    error_message: Option<&'a str>,
}

fn firehose_response(
    status: StatusCode,
    request_id: &str,
    error_message: Option<&str>,
) -> Response {
    let body = FirehoseResponseBody {
        request_id,
        timestamp: Utc::now().timestamp_millis(),
        error_message,
    };
    Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(Body::from(
            serde_json::to_vec(&body).expect("serializing firehose response"),
        ))
        .expect("building firehose response")
}

fn request_id_from(headers: &HeaderMap) -> String {
    headers
        .get(HEADER_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// Firehose-specific auth: read `X-Amz-Firehose-Access-Key`, synthesize an
/// `Authorization: Bearer <key>` header, and validate via the same `AuthProvider` the rest
/// of the ingestion service uses (reuses the constant-time keyring check verbatim). On
/// failure, return the Firehose error shape (non-200 JSON) so Firehose retries/spills
/// rather than dropping data.
async fn firehose_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    req: Request,
    next: Next,
) -> Response {
    let request_id = request_id_from(req.headers());
    let Some(access_key) = req
        .headers()
        .get(HEADER_ACCESS_KEY)
        .and_then(|v| v.to_str().ok())
    else {
        return firehose_response(
            StatusCode::UNAUTHORIZED,
            &request_id,
            Some("missing X-Amz-Firehose-Access-Key"),
        );
    };
    let mut headers = req.headers().clone();
    if let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {access_key}")) {
        headers.insert(header::AUTHORIZATION, bearer);
    }
    let parts = HttpRequestParts {
        headers,
        method: req.method().clone(),
        uri: req.uri().clone(),
    };
    match provider.validate_request(&parts as &dyn RequestParts).await {
        Ok(_ctx) => next.run(req).await,
        Err(e) => {
            warn!("[firehose auth_failure] {e}");
            firehose_response(
                StatusCode::UNAUTHORIZED,
                &request_id,
                Some("invalid access key"),
            )
        }
    }
}

async fn firehose_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let mut request_id = request_id_from(&headers);
    let envelope = match handler::decode_firehose_envelope(&body) {
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
