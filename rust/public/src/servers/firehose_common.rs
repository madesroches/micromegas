//! Shared Kinesis Data Firehose HTTP Endpoint Delivery plumbing, reused by every
//! Firehose-backed ingestion route (metrics, CloudWatch Logs, and any future signal).
//!
//! Everything here is signal-agnostic — it only knows about the Firehose transport
//! (access-key header, ack shape), not what's inside a record. Firehose's only credential
//! channel is the non-standard `X-Amz-Firehose-Access-Key` header — it cannot send
//! `Authorization: Bearer`. So a Firehose route cannot sit under the global Bearer
//! `auth_middleware`; it has its own auth step that synthesizes a bearer header from the
//! Firehose header and reuses the same `AuthProvider` (constant-time keyring check)
//! verbatim.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use micromegas_auth::types::{AuthProvider, HttpRequestParts, RequestParts};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

pub(crate) const HEADER_ACCESS_KEY: &str = "X-Amz-Firehose-Access-Key";
pub(crate) const HEADER_REQUEST_ID: &str = "X-Amz-Firehose-Request-Id";

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

pub(crate) fn firehose_response(
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

pub(crate) fn request_id_from(headers: &HeaderMap) -> String {
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
pub(crate) async fn firehose_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    mut req: Request,
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
        Ok(_ctx) => {
            // SECURITY: strip any client-provided auth headers to prevent spoofing, matching
            // the shared `auth_middleware` (auth/src/axum.rs). These headers must only ever be
            // trusted when set by the authentication layer, never by the incoming request.
            req.headers_mut().remove("x-auth-subject");
            req.headers_mut().remove("x-auth-email");
            req.headers_mut().remove("x-auth-issuer");
            req.headers_mut().remove("x-allow-delegation");
            next.run(req).await
        }
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
