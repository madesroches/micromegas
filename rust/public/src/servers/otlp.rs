//! OTLP/HTTP route registration for `telemetry-ingestion-srv`.
//!
//! Exposes three routes that match the OTLP/HTTP spec:
//!  - `POST /ingestion/otlp/v1/logs`
//!  - `POST /ingestion/otlp/v1/metrics`
//!  - `POST /ingestion/otlp/v1/traces`
//!
//! The OTLP sub-router applies its own 20 MiB body limit (matching the OTel Collector
//! `confighttp.max_request_body_size` default) plus gzip request decompression,
//! independent of the parent router's 100 MiB limit on `/ingestion/insert_block`.
//!
//! Per OTLP/HTTP spec, success responses are 2xx with an `Export*ServiceResponse` proto;
//! 4xx/5xx responses are protobuf-encoded `google.rpc.Status` messages, with a
//! `Retry-After` header on retryable 503 responses.

use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_otel_ingestion::error::OtelError;
use micromegas_otel_ingestion::handler;
use micromegas_tracing::prelude::*;
use prost::Message;
use std::sync::Arc;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

/// 20 MiB matches the OTel Collector `confighttp.max_request_body_size` default —
/// anything an SDK is willing to send under the conventional Collector cap fits here too.
/// Applies to compressed wire bytes (the `RequestBodyLimitLayer` runs outside the
/// decompression layer).
const OTLP_BODY_LIMIT_BYTES: usize = 20 * 1024 * 1024;

/// Cap on the decompressed body size the handler will materialize. Without this,
/// a malicious gzip payload up to `OTLP_BODY_LIMIT_BYTES` could expand at gzip's
/// worst-case ratio (~1000×) and OOM the server. Sized at 15× the wire cap to
/// cover legitimate protobuf compression (commonly observed up to 10×) with
/// headroom, while still bounding the worst case to a survivable allocation.
const OTLP_DECOMPRESSED_BODY_LIMIT_BYTES: usize = 300 * 1024 * 1024;

/// `Retry-After` value (in seconds) on retryable 503 responses. Conservative default —
/// tune based on observed recovery times.
const RETRY_AFTER_SECONDS: u32 = 30;

const CONTENT_TYPE_PROTOBUF: &str = "application/x-protobuf";

/// Examines the `Content-Type` header. The spec allows parameters
/// (e.g. `application/x-protobuf; charset=utf-8`), so we parse rather than string-compare.
fn check_content_type(headers: &HeaderMap) -> Result<(), OtlpHttpError> {
    let Some(ct) = headers.get(header::CONTENT_TYPE) else {
        return Err(OtlpHttpError::WrongContentType);
    };
    let Ok(ct) = ct.to_str() else {
        return Err(OtlpHttpError::WrongContentType);
    };
    let media = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if media == CONTENT_TYPE_PROTOBUF {
        Ok(())
    } else {
        Err(OtlpHttpError::WrongContentType)
    }
}

/// Internal error type covering both pre-handler validation failures (415) and
/// post-handler `OtelError`s. Each variant maps to a single HTTP response shape
/// (status code, optional `Retry-After`, `google.rpc.Status` body).
enum OtlpHttpError {
    WrongContentType,
    Otel(OtelError),
}

impl OtlpHttpError {
    fn into_otlp_response(self) -> Response {
        match self {
            OtlpHttpError::WrongContentType => build_error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                3, // INVALID_ARGUMENT
                "Content-Type must be application/x-protobuf",
                false,
            ),
            OtlpHttpError::Otel(err) => {
                let retryable = err.is_retryable();
                let status = match err.http_status() {
                    400 => StatusCode::BAD_REQUEST,
                    503 => StatusCode::SERVICE_UNAVAILABLE,
                    other => {
                        StatusCode::from_u16(other).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
                    }
                };
                let code = err.grpc_code();
                // Detailed error (includes raw sqlx / object-store messages) is
                // logged server-side; only the sanitized public form goes to the
                // client to avoid leaking backend internals.
                error!("OTLP error: {}", err);
                build_error_response(status, code, &err.public_message(), retryable)
            }
        }
    }
}

fn build_error_response(status: StatusCode, code: i32, message: &str, retryable: bool) -> Response {
    let proto_status = micromegas_otel_ingestion::proto::Status {
        code,
        message: message.to_string(),
    };
    let body = proto_status.encode_to_vec();
    let mut response = Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static(CONTENT_TYPE_PROTOBUF),
        )
        .body(Body::from(body))
        .expect("building OTLP error response");
    if retryable && let Ok(value) = HeaderValue::from_str(&RETRY_AFTER_SECONDS.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

fn proto_response<M: Message>(msg: M) -> Response {
    let body = msg.encode_to_vec();
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static(CONTENT_TYPE_PROTOBUF),
        )
        .body(Body::from(body))
        .expect("building OTLP success response")
}

async fn logs_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    if let Err(e) = check_content_type(&headers) {
        return e.into_otlp_response();
    }
    match handler::ingest_logs(service, body).await {
        Ok(resp) => proto_response(resp),
        Err(e) => OtlpHttpError::Otel(e).into_otlp_response(),
    }
}

async fn metrics_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    if let Err(e) = check_content_type(&headers) {
        return e.into_otlp_response();
    }
    match handler::ingest_metrics(service, body).await {
        Ok(resp) => proto_response(resp),
        Err(e) => OtlpHttpError::Otel(e).into_otlp_response(),
    }
}

async fn traces_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    if let Err(e) = check_content_type(&headers) {
        return e.into_otlp_response();
    }
    match handler::ingest_traces(service, body).await {
        Ok(resp) => proto_response(resp),
        Err(e) => OtlpHttpError::Otel(e).into_otlp_response(),
    }
}

/// Builds a sub-Router carrying the three OTLP routes plus the body-limit and
/// gzip-decompression layers scoped to those routes.
///
/// Layer order: `RequestBodyLimitLayer` (outer, wire bytes) →
/// `RequestDecompressionLayer` (inner, gzip expansion) → handler. The handler's
/// `Bytes` extractor consults `DefaultBodyLimit` to cap the post-decompression
/// payload, defending against gzip-bomb expansion that the wire-byte limit can't see.
pub fn otlp_router() -> Router {
    Router::new()
        .route("/ingestion/otlp/v1/logs", post(logs_handler))
        .route("/ingestion/otlp/v1/metrics", post(metrics_handler))
        .route("/ingestion/otlp/v1/traces", post(traces_handler))
        .layer(RequestDecompressionLayer::new().gzip(true))
        .layer(RequestBodyLimitLayer::new(OTLP_BODY_LIMIT_BYTES))
        .layer(DefaultBodyLimit::max(OTLP_DECOMPRESSED_BODY_LIMIT_BYTES))
}
