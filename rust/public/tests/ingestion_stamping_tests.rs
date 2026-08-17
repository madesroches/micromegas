//! Unit tests for `resolve_write_audience` / `StampingConfig::from_env`, plus HTTP-level denial
//! cases for the native and OTLP ingestion routes (AbAC Stage 5, #1373, §5).
//!
//! HTTP cases use `tower::ServiceExt::oneshot` against a lazily-connected Postgres pool +
//! in-memory object store (never actually touched), matching
//! `rust/public/tests/firehose_tests.rs:1-7`'s own constraint: every case here must be a
//! request that either stops at the gate or does zero database work.

use axum::Extension;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use micromegas::servers::ingestion::register_routes;
use micromegas::servers::otlp::otlp_router;
use micromegas::servers::webhook::webhook_router;
use micromegas::servers::write_audience::{StampingConfig, resolve_write_audience};
use micromegas_auth::types::{AuthContext, AuthType};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::memory::InMemory;
use object_store::path::Path;
use serial_test::serial;
use std::sync::Arc;
use tower::ServiceExt;

fn make_test_service() -> Arc<WebIngestionService> {
    let blob_store = Arc::new(InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(blob_store, Path::default()));
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible");
    Arc::new(WebIngestionService::new(DataLakeConnection::new(
        pool,
        blob_storage,
    )))
}

fn ctx_with_bound_audience(audience: &str) -> AuthContext {
    AuthContext {
        subject: "bound-audience-test".to_string(),
        email: None,
        issuer: "api_key".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::ApiKey,
        is_admin: false,
        allow_delegation: false,
        bound_audience: Some(audience.to_string()),
        read_audiences: vec![],
        groups: vec![],
    }
}

fn ctx_without_bound_audience() -> AuthContext {
    AuthContext {
        subject: "env-keyring-test".to_string(),
        email: None,
        issuer: "api_key".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::ApiKey,
        is_admin: false,
        allow_delegation: false,
        bound_audience: None,
        read_audiences: vec![],
        groups: vec![],
    }
}

// ---------------------------------------------------------------------------
// resolve_write_audience: the full 3x2 table from §5.
// ---------------------------------------------------------------------------

#[test]
fn bound_audience_stamps_regardless_of_the_knob() {
    let ctx = ctx_with_bound_audience("team-a");
    for require in [false, true] {
        let cfg = StampingConfig::new(require);
        let audience =
            resolve_write_audience(Some(&ctx), &cfg).expect("a bound audience always stamps");
        assert_eq!(audience, WriteAudience::new(Some("team-a")).unwrap());
    }
}

#[test]
fn audience_less_credential_is_unstamped_when_knob_off() {
    let ctx = ctx_without_bound_audience();
    let cfg = StampingConfig::new(false);
    let audience = resolve_write_audience(Some(&ctx), &cfg).expect("off -> unstamped, not Err");
    assert_eq!(audience, WriteAudience::none());
}

#[test]
fn audience_less_credential_is_rejected_when_knob_on() {
    let ctx = ctx_without_bound_audience();
    let cfg = StampingConfig::new(true);
    assert!(resolve_write_audience(Some(&ctx), &cfg).is_err());
}

#[test]
fn no_extension_is_unstamped_when_knob_off() {
    let cfg = StampingConfig::new(false);
    let audience = resolve_write_audience(None, &cfg).expect("off -> unstamped, not Err");
    assert_eq!(audience, WriteAudience::none());
}

#[test]
fn no_extension_is_rejected_when_knob_on() {
    let cfg = StampingConfig::new(true);
    assert!(
        resolve_write_audience(None, &cfg).is_err(),
        "no auth provider configured must never silently pass once the knob is set"
    );
}

// ---------------------------------------------------------------------------
// StampingConfig::from_env
// ---------------------------------------------------------------------------

const RWA_PREFIX: &str = "MICROMEGAS_1373_STAMPING_TESTS";
const RWA_PREFIXED_VAR: &str = "MICROMEGAS_1373_STAMPING_TESTS_REQUIRE_WRITE_AUDIENCE";
const RWA_UNPREFIXED_VAR: &str = "MICROMEGAS_REQUIRE_WRITE_AUDIENCE";

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(RWA_PREFIXED_VAR);
            std::env::remove_var(RWA_UNPREFIXED_VAR);
        }
    }
}

#[test]
#[serial]
fn stamping_config_from_env_unset_is_off() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(RWA_PREFIXED_VAR);
        std::env::remove_var(RWA_UNPREFIXED_VAR);
    }
    let cfg = StampingConfig::from_env(RWA_PREFIX).expect("from_env");
    let ctx = ctx_without_bound_audience();
    assert!(resolve_write_audience(Some(&ctx), &cfg).is_ok());
}

#[test]
#[serial]
fn stamping_config_from_env_reads_unprefixed_fallback() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(RWA_PREFIXED_VAR);
        std::env::set_var(RWA_UNPREFIXED_VAR, "true");
    }
    let cfg = StampingConfig::from_env(RWA_PREFIX).expect("from_env");
    let ctx = ctx_without_bound_audience();
    assert!(resolve_write_audience(Some(&ctx), &cfg).is_err());
}

#[test]
#[serial]
fn stamping_config_from_env_prefixed_wins_over_unprefixed() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(RWA_PREFIXED_VAR, "false");
        std::env::set_var(RWA_UNPREFIXED_VAR, "true");
    }
    let cfg = StampingConfig::from_env(RWA_PREFIX).expect("from_env");
    let ctx = ctx_without_bound_audience();
    assert!(resolve_write_audience(Some(&ctx), &cfg).is_ok());
}

#[test]
#[serial]
fn stamping_config_from_env_rejects_a_malformed_boolean() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(RWA_PREFIXED_VAR);
        std::env::set_var(RWA_UNPREFIXED_VAR, "not-a-bool");
    }
    assert!(StampingConfig::from_env(RWA_PREFIX).is_err());
}

// ---------------------------------------------------------------------------
// HTTP-level: OTLP
// ---------------------------------------------------------------------------

fn otlp_app(stamping: StampingConfig, ctx: Option<AuthContext>) -> axum::Router {
    let mut router = otlp_router()
        .layer(Extension(make_test_service()))
        .layer(Extension(Arc::new(stamping)));
    if let Some(ctx) = ctx {
        router = router.layer(Extension(ctx));
    }
    router
}

#[tokio::test]
async fn otlp_knob_off_empty_resource_logs_passes_the_gate_and_returns_ok() {
    // No auth extension at all (the disabled-auth dev-mode path) -- with the knob off this must
    // stay unstamped, not rejected. `ingest_logs` returns `Ok` on an empty `resource_logs`
    // before touching the database (`handler.rs`), so a 200 here is proof the gate let the
    // request through all the way to the handler.
    let app = otlp_app(StampingConfig::new(false), None);
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/otlp/v1/logs")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"resourceLogs":[]}"#))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn otlp_knob_on_no_extension_is_denied_with_grpc_code_7_in_json() {
    // No auth extension (no auth provider configured) + the knob on -> 403, `google.rpc.Status`
    // code 7 (PERMISSION_DENIED), encoded in the request's own encoding (JSON in -> JSON out).
    let app = otlp_app(StampingConfig::new(true), None);
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/otlp/v1/logs")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"resourceLogs":[]}"#))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading response body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parsing json body");
    assert_eq!(json["code"], 7);
}

#[tokio::test]
async fn otlp_knob_on_bound_audience_credential_passes_the_gate() {
    // A credential carrying a bound audience must never be rejected by this gate, knob on or
    // off -- differential proof the gate discriminates on the credential, not just the knob.
    let ctx = ctx_with_bound_audience("team-a");
    let app = otlp_app(StampingConfig::new(true), Some(ctx));
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/otlp/v1/logs")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"resourceLogs":[]}"#))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// HTTP-level: native routes (insert_process/insert_stream/insert_block)
// ---------------------------------------------------------------------------

fn native_app(stamping: StampingConfig, ctx: Option<AuthContext>) -> axum::Router {
    let mut router = register_routes(axum::Router::new())
        .layer(Extension(make_test_service()))
        .layer(Extension(Arc::new(stamping)));
    if let Some(ctx) = ctx {
        router = router.layer(Extension(ctx));
    }
    router
}

/// Deliberately malformed CBOR: `insert_process` reaches this only *after*
/// `resolve_write_audience` returns `Ok` -- so this is a differential proof of pass-through,
/// not just a parse-error smoke test. Never touches the (unreachable) database.
const MALFORMED_CBOR: &[u8] = b"this is not valid CBOR";

#[tokio::test]
async fn native_knob_off_malformed_body_reaches_the_parse_boundary_as_400() {
    let app = native_app(StampingConfig::new(false), None);
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/insert_process")
        .body(Body::from(MALFORMED_CBOR))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "the knob-off gate must pass through to the parser, which then rejects malformed CBOR"
    );
}

#[tokio::test]
async fn native_knob_on_no_extension_is_denied_before_parsing() {
    let app = native_app(StampingConfig::new(true), None);
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/insert_process")
        .body(Body::from(MALFORMED_CBOR))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "the gate must reject before the malformed body is ever parsed"
    );
}

#[tokio::test]
async fn native_knob_on_bound_audience_credential_reaches_the_parse_boundary() {
    let ctx = ctx_with_bound_audience("team-a");
    let app = native_app(StampingConfig::new(true), Some(ctx));
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/insert_process")
        .body(Body::from(MALFORMED_CBOR))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// HTTP-level: webhook (reuses OtelError::Denied, rendered through build_error_response)
// ---------------------------------------------------------------------------

fn webhook_app(stamping: StampingConfig, ctx: Option<AuthContext>) -> axum::Router {
    let mut router = webhook_router()
        .layer(Extension(make_test_service()))
        .layer(Extension(Arc::new(stamping)));
    if let Some(ctx) = ctx {
        router = router.layer(Extension(ctx));
    }
    router
}

#[tokio::test]
async fn webhook_knob_on_no_extension_is_denied_with_plain_text_403() {
    let app = webhook_app(StampingConfig::new(true), None);
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/webhook")
        .body(Body::from("some webhook payload"))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // Rendered via webhook.rs's own build_error_response -- text/plain, not google.rpc.Status.
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("content-type header")
        .to_str()
        .expect("valid header value");
    assert!(content_type.starts_with("text/plain"));
    // Non-retryable: no Retry-After header on a denial.
    assert!(response.headers().get(header::RETRY_AFTER).is_none());
}
