//! Unit tests for `resolve_write_audience`, plus HTTP-level pass-through cases for the native,
//! OTLP, and webhook ingestion routes (AbAC Stage 5, #1373, §5).
//!
//! HTTP cases use `tower::ServiceExt::oneshot` against a lazily-connected Postgres pool +
//! in-memory object store (never actually touched), matching
//! `rust/public/tests/firehose_tests.rs:1-7`'s own constraint: every case here must be a
//! request that does zero database work.

use axum::Extension;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use micromegas::servers::ingestion::register_routes;
use micromegas::servers::otlp::otlp_router;
use micromegas::servers::webhook::webhook_router;
use micromegas::servers::write_audience::resolve_write_audience;
use micromegas_auth::types::{AuthContext, AuthType};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::memory::InMemory;
use object_store::path::Path;
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
// resolve_write_audience: its one remaining branch.
// ---------------------------------------------------------------------------

#[test]
fn bound_audience_stamps() {
    let ctx = Extension(ctx_with_bound_audience("team-a"));
    let audience = resolve_write_audience(Some(&ctx));
    assert_eq!(audience, WriteAudience::new(Some("team-a")).unwrap());
}

#[test]
fn audience_less_credential_is_unstamped() {
    let ctx = Extension(ctx_without_bound_audience());
    let audience = resolve_write_audience(Some(&ctx));
    assert_eq!(audience, WriteAudience::none());
}

#[test]
fn no_extension_is_unstamped() {
    let audience = resolve_write_audience(None);
    assert_eq!(audience, WriteAudience::none());
}

// ---------------------------------------------------------------------------
// HTTP-level: OTLP
// ---------------------------------------------------------------------------

fn otlp_app(ctx: Option<AuthContext>) -> axum::Router {
    let mut router = otlp_router().layer(Extension(make_test_service()));
    if let Some(ctx) = ctx {
        router = router.layer(Extension(ctx));
    }
    router
}

#[tokio::test]
async fn otlp_bound_audience_credential_passes_through() {
    // A bound-audience credential's request reaches the handler and gets a clean 200 --
    // `ingest_logs` returns `Ok` on an empty `resourceLogs` before touching the database
    // (`handler.rs`), so this is proof the request reaches all the way through.
    let ctx = ctx_with_bound_audience("team-a");
    let app = otlp_app(Some(ctx));
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

fn native_app(ctx: Option<AuthContext>) -> axum::Router {
    let mut router = register_routes(axum::Router::new()).layer(Extension(make_test_service()));
    if let Some(ctx) = ctx {
        router = router.layer(Extension(ctx));
    }
    router
}

/// Deliberately malformed CBOR: `insert_process` reaches this only *after*
/// `resolve_write_audience` runs -- so this is a differential proof of pass-through, not just a
/// parse-error smoke test. Never touches the (unreachable) database.
const MALFORMED_CBOR: &[u8] = b"this is not valid CBOR";

#[tokio::test]
async fn native_bound_audience_credential_reaches_the_parse_boundary() {
    let ctx = ctx_with_bound_audience("team-a");
    let app = native_app(Some(ctx));
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/insert_process")
        .body(Body::from(MALFORMED_CBOR))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// HTTP-level: webhook
// ---------------------------------------------------------------------------

fn webhook_app(ctx: Option<AuthContext>) -> axum::Router {
    let mut router = webhook_router().layer(Extension(make_test_service()));
    if let Some(ctx) = ctx {
        router = router.layer(Extension(ctx));
    }
    router
}

#[tokio::test]
async fn webhook_empty_body_is_rejected_before_any_db_work() {
    // The only zero-DB HTTP case available for this route (`webhook.rs:123-125`): an empty
    // body is rejected before `resolve_write_audience` is even reached. Not differential on
    // the credential -- a bound-audience and an audience-less credential would both hit the
    // identical 503 further in (this harness's lazy pool is never actually reachable), so
    // there is no zero-DB way to distinguish them here.
    let app = webhook_app(None);
    let request = Request::builder()
        .method("POST")
        .uri("/ingestion/webhook")
        .body(Body::empty())
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
