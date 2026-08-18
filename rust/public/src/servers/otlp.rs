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
//! Per OTLP/HTTP spec, success responses mirror the request encoding (JSON in → JSON out,
//! proto in → proto out). Error responses (4xx/5xx) carry a `google.rpc.Status` body
//! encoded in the same way, except 415 responses which always use protobuf because the
//! request encoding is unknown at that point.

use super::ingestion_limits::{RETRY_AFTER_SECONDS, apply_ingestion_body_limits};
use super::write_audience::{StampingConfig, resolve_write_audience};
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use micromegas_auth::types::AuthContext;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_otel_ingestion::Encoding;
use micromegas_otel_ingestion::error::{OtelError, Signal};
use micromegas_otel_ingestion::handler;
use micromegas_tracing::prelude::*;
use prost::Message;
use std::sync::Arc;

const CONTENT_TYPE_PROTOBUF: &str = "application/x-protobuf";
const CONTENT_TYPE_JSON: &str = "application/json";

/// Examines the `Content-Type` header and maps it to an `Encoding`. The spec allows
/// parameters (e.g. `application/json; charset=utf-8`), so we parse rather than
/// string-compare. Returns `Err(OtlpHttpError::WrongContentType)` for unknown types.
fn content_type_encoding(headers: &HeaderMap) -> Result<Encoding, OtlpHttpError> {
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
    match media.as_str() {
        CONTENT_TYPE_PROTOBUF => Ok(Encoding::Protobuf),
        CONTENT_TYPE_JSON => Ok(Encoding::Json),
        _ => Err(OtlpHttpError::WrongContentType),
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
    fn into_otlp_response(self, encoding: Encoding) -> Response {
        match self {
            OtlpHttpError::WrongContentType => build_error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                3, // INVALID_ARGUMENT
                "Content-Type must be application/x-protobuf or application/json",
                false,
                // encoding is unknown for 415; always emit proto Status (OTLP/HTTP default)
                Encoding::Protobuf,
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
                build_error_response(status, code, &err.public_message(), retryable, encoding)
            }
        }
    }
}

fn build_error_response(
    status: StatusCode,
    code: i32,
    message: &str,
    retryable: bool,
    encoding: Encoding,
) -> Response {
    let proto_status = micromegas_otel_ingestion::proto::Status {
        code,
        message: message.to_string(),
    };
    let (body, content_type) = match encoding {
        Encoding::Protobuf => (proto_status.encode_to_vec(), CONTENT_TYPE_PROTOBUF),
        Encoding::Json => (
            serde_json::to_vec(&proto_status).expect("serializing Status to JSON"),
            CONTENT_TYPE_JSON,
        ),
    };
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, HeaderValue::from_static(content_type))
        .body(Body::from(body))
        .expect("building OTLP error response");
    if retryable && let Ok(value) = HeaderValue::from_str(&RETRY_AFTER_SECONDS.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

fn success_response<M: Message + serde::Serialize>(msg: M, encoding: Encoding) -> Response {
    let (body, content_type) = match encoding {
        Encoding::Protobuf => (msg.encode_to_vec(), CONTENT_TYPE_PROTOBUF),
        Encoding::Json => (
            serde_json::to_vec(&msg).expect("serializing OTLP response to JSON"),
            CONTENT_TYPE_JSON,
        ),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_static(content_type))
        .body(Body::from(body))
        .expect("building OTLP success response")
}

/// Resolves the write audience for an OTLP request, mapping a gate rejection onto
/// `OtelError::Denied` (AbAC Stage 5, #1373, §5) -- a sanitized `write audience required`
/// message via `public_message()`, never internal detail.
fn resolve_otlp_write_audience(
    ctx: &Option<Extension<AuthContext>>,
    stamping: &StampingConfig,
    signal: Signal,
) -> Result<WriteAudience, OtelError> {
    resolve_write_audience(ctx.as_ref(), stamping).map_err(|_| OtelError::Denied {
        signal,
        message: "write audience required".to_string(),
        public_message: "write audience required",
    })
}

async fn logs_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let encoding = match content_type_encoding(&headers) {
        Ok(enc) => enc,
        Err(e) => return e.into_otlp_response(Encoding::Protobuf),
    };
    let audience = match resolve_otlp_write_audience(&ctx, &stamping, Signal::Logs) {
        Ok(a) => a,
        Err(e) => return OtlpHttpError::Otel(e).into_otlp_response(encoding),
    };
    match handler::ingest_logs(service, body, encoding, &audience).await {
        Ok(resp) => success_response(resp, encoding),
        Err(e) => OtlpHttpError::Otel(e).into_otlp_response(encoding),
    }
}

async fn metrics_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let encoding = match content_type_encoding(&headers) {
        Ok(enc) => enc,
        Err(e) => return e.into_otlp_response(Encoding::Protobuf),
    };
    let audience = match resolve_otlp_write_audience(&ctx, &stamping, Signal::Metrics) {
        Ok(a) => a,
        Err(e) => return OtlpHttpError::Otel(e).into_otlp_response(encoding),
    };
    match handler::ingest_metrics(service, body, encoding, &audience).await {
        Ok(resp) => success_response(resp, encoding),
        Err(e) => OtlpHttpError::Otel(e).into_otlp_response(encoding),
    }
}

async fn traces_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    Extension(stamping): Extension<Arc<StampingConfig>>,
    ctx: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let encoding = match content_type_encoding(&headers) {
        Ok(enc) => enc,
        Err(e) => return e.into_otlp_response(Encoding::Protobuf),
    };
    let audience = match resolve_otlp_write_audience(&ctx, &stamping, Signal::Traces) {
        Ok(a) => a,
        Err(e) => return OtlpHttpError::Otel(e).into_otlp_response(encoding),
    };
    match handler::ingest_traces(service, body, encoding, &audience).await {
        Ok(resp) => success_response(resp, encoding),
        Err(e) => OtlpHttpError::Otel(e).into_otlp_response(encoding),
    }
}

/// Builds a sub-Router carrying the three OTLP routes plus the shared body-limit and
/// gzip-decompression layers scoped to those routes (see `ingestion_limits`).
pub fn otlp_router() -> Router {
    apply_ingestion_body_limits(
        Router::new()
            .route("/ingestion/otlp/v1/logs", post(logs_handler))
            .route("/ingestion/otlp/v1/metrics", post(metrics_handler))
            .route("/ingestion/otlp/v1/traces", post(traces_handler)),
    )
}
