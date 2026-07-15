//! Generic header-described webhook ingestion route for `telemetry-ingestion-srv`.
//!
//! Exposes `POST /ingestion/webhook`. Any header-capable webhook producer (GitLab,
//! GitHub, generic SaaS) can report directly to micromegas: three `X-Micromegas-*`
//! request headers synthesize an OTLP `Resource` + scope name, and the verbatim
//! request body becomes a single log record's body. The synthetic request is fed
//! into the existing OTLP logs split/write path (`handler::ingest_webhook`) — no
//! new identity, block, or write logic.
//!
//! The body is opaque: this endpoint does not negotiate `Content-Type` or parse the
//! body at all (contrast with `otlp_router`, which must switch proto/JSON decoding).

use super::ingestion_limits::{RETRY_AFTER_SECONDS, apply_ingestion_body_limits};
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_otel_ingestion::handler;
use micromegas_otel_ingestion::proto::{AnyValue, KeyValue, any_value};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

const HEADER_SERVICE_NAME: &str = "X-Micromegas-Service-Name";
const HEADER_SERVICE_NAMESPACE: &str = "X-Micromegas-Service-Namespace";
const HEADER_TARGET: &str = "X-Micromegas-Target";

/// Reads one header and, if present and decodable as ASCII/UTF-8, pushes a
/// `KeyValue { key: otel_key, value: StringValue(header value) }` onto `attrs`.
/// Missing or non-decodable headers are silently skipped — same as an OTLP resource
/// that omits an attribute.
fn push_attr_from_header(
    attrs: &mut Vec<KeyValue>,
    headers: &HeaderMap,
    header: &str,
    otel_key: &str,
) {
    let Some(value) = headers.get(header) else {
        return;
    };
    let Ok(value) = value.to_str() else {
        return;
    };
    attrs.push(KeyValue {
        key: otel_key.to_string(),
        key_strindex: 0,
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    });
}

/// Reads `X-Micromegas-Target`, returning an empty string when absent or non-decodable.
fn target_from_header(headers: &HeaderMap) -> String {
    headers
        .get(HEADER_TARGET)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn build_error_response(status: StatusCode, message: &str, retryable: bool) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )
        .body(Body::from(message.to_string()))
        .expect("building webhook error response");
    if retryable && let Ok(value) = HeaderValue::from_str(&RETRY_AFTER_SECONDS.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

async fn webhook_handler(
    Extension(service): Extension<Arc<WebIngestionService>>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    if body.is_empty() {
        return build_error_response(StatusCode::BAD_REQUEST, "empty body", false);
    }

    let mut resource_attrs = Vec::new();
    push_attr_from_header(
        &mut resource_attrs,
        &headers,
        HEADER_SERVICE_NAME,
        "service.name",
    );
    push_attr_from_header(
        &mut resource_attrs,
        &headers,
        HEADER_SERVICE_NAMESPACE,
        "service.namespace",
    );
    let target = target_from_header(&headers);

    match handler::ingest_webhook(service, resource_attrs, target, body).await {
        Ok(()) => Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .expect("building webhook success response"),
        Err(err) => {
            let retryable = err.is_retryable();
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            // Detailed error (includes raw sqlx / object-store messages) is logged
            // server-side; only the sanitized public form goes to the client.
            error!("webhook error: {err}");
            build_error_response(status, &err.public_message(), retryable)
        }
    }
}

/// Builds a sub-Router carrying the webhook route plus the shared body-limit and
/// gzip-decompression layers scoped to it (see `ingestion_limits`).
pub fn webhook_router() -> Router {
    apply_ingestion_body_limits(Router::new().route("/ingestion/webhook", post(webhook_handler)))
}
