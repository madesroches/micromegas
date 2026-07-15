//! Generic header-described webhook ingestion route for `telemetry-ingestion-srv`.
//!
//! Exposes `POST /ingestion/webhook`. Any header-capable webhook producer (GitLab,
//! GitHub, generic SaaS) can report directly to micromegas: three `X-Micromegas-*`
//! request headers synthesize an OTLP `Resource` + scope name, and the request body
//! becomes a single log record's body, stored verbatim when it is valid UTF-8 (the
//! common case: JSON payloads from GitLab/GitHub/etc.). There is no header to describe
//! an alternate codec, so a non-UTF8 body is stored via lossy UTF-8 conversion (invalid
//! byte sequences become U+FFFD) rather than rejected or stored as opaque binary. The
//! synthetic request is fed into the existing OTLP logs split/write path
//! (`handler::ingest_webhook`) — no new identity, block, or write logic.
//!
//! The body is opaque JSON/text: this endpoint does not negotiate `Content-Type` or
//! parse the body at all (contrast with `otlp_router`, which must switch proto/JSON
//! decoding).

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
    header_name: &str,
    otel_key: &str,
) {
    let Some(value) = headers.get(header_name) else {
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

/// Canonicalizes the full incoming header set into stable bytes for `block_id` hashing
/// (`handler::ingest_webhook`'s `header_hash_input`).
///
/// Only the 3 recognized `X-Micromegas-*` headers become OTel resource attrs, so without
/// this, any other header a producer sends (a delivery-id, a signature, an event-type
/// header) would have zero influence on `block_id` — two deliveries with the same body but
/// different unrecognized headers would collide and dedup as if they were retries of the
/// same delivery. Folding in the raw header set closes that gap, at the cost of also
/// widening it the other way: a genuine retry of the same delivery that picks up a new
/// hop-by-hop header along the way (e.g. a proxy stamping a fresh `Date` or request-id) no
/// longer dedups. That tradeoff is accepted here — see the "Webhook ingestion" docs section.
///
/// Headers are lowercased and sorted by (name, value) so the hash doesn't depend on wire
/// order, which servers/proxies are free to reshuffle.
fn canonical_header_bytes(headers: &HeaderMap) -> Vec<u8> {
    let mut pairs: Vec<(String, &[u8])> = headers
        .iter()
        .map(|(name, value)| (name.as_str().to_ascii_lowercase(), value.as_bytes()))
        .collect();
    pairs.sort();

    let mut buf = Vec::new();
    for (name, value) in pairs {
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(value);
        buf.push(0);
    }
    buf
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
    let header_hash_input = canonical_header_bytes(&headers);

    match handler::ingest_webhook(service, resource_attrs, target, body, &header_hash_input).await {
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
