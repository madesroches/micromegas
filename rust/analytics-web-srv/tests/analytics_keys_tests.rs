//! Tests for `analytics_keys.rs` — the analytics-key management routes
//! (#1411).
//!
//! Modeled on `maps_tests.rs`'s `build_handler_router_with_user` +
//! `.oneshot(...)` pattern: routes are wired the same way
//! `build_protected_routes` wires them, but with `cookie_auth_middleware`
//! bypassed by pre-inserting a synthetic `ValidatedUser` extension (the same
//! shape `--disable-auth` uses) — this exercises the real `AdminUser`
//! extractor, unlike `folders_tests.rs`/`screens_tests.rs`'s plain
//! `Extension(test_user())` handler-as-function calls, which never run an
//! extractor at all.
//!
//! Every test here uses a lazily-connected pool (`sqlx::PgPool::connect_lazy`,
//! per `firehose_tests.rs`'s precedent) and never actually reaches the
//! database: the 403 cases are rejected by the `AdminUser` extractor before
//! any handler runs, the 400 cases fail validation before touching the pool,
//! and the `NotConfigured` cases use `AnalyticsKeysState { pool: None }`,
//! which never touches `state.pool` at all. Live-DB round trips for
//! mint/list/revoke/import are `#[ignore]`d, run manually against a real
//! Postgres per `folders_tests.rs`'s precedent.

use analytics_web_srv::analytics_keys::{AnalyticsKeysState, analytics_keys_router};
use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use axum::{Extension, Router, body::Body, http::Request, http::StatusCode};
use tower::ServiceExt;

fn lazy_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "admin".to_string(),
        email: Some("admin@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: true,
    }
}

fn non_admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "reader".to_string(),
        email: Some("reader@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
    }
}

/// Wires `analytics_keys_router` the same way `build_protected_routes` does
/// (state layered as `Extension<AnalyticsKeysState>`), with auth bypassed by
/// pre-inserting a synthetic `ValidatedUser` — the same shape `--disable-auth`
/// uses — instead of standing up an OIDC mock and running
/// `cookie_auth_middleware` for real.
fn build_handler_router_with_user(state: AnalyticsKeysState, user: ValidatedUser) -> Router {
    analytics_keys_router("")
        .layer(Extension(state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
}

fn post_request(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("build request")
}

fn delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .expect("build request")
}

// ---------------------------------------------------------------------------
// The gate: 403 for a non-admin `ValidatedUser`, on every route. The
// `AdminUser` extractor rejects before any handler body runs — never
// touches the pool.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/analytics-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/analytics-api-keys"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoke_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/analytics-api-keys/{}",
            uuid::Uuid::new_v4()
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/analytics-api-keys/import",
            r#"{"name": "legacy", "key": "legacy-secret"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 400 validation — checked before any hashing/DB access.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_400_for_empty_name() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/analytics-api-keys", r#"{"name": ""}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_400_for_empty_key() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/analytics-api-keys/import",
            r#"{"name": "legacy", "key": ""}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_400_for_negative_limit() {
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/analytics-api-keys?limit=-1"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// NotConfigured 503 — `AnalyticsKeysState { pool: None }` never touches the
// pool, so this needs no live DB either, same harness as every other test
// here.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AnalyticsKeysState { pool: None }, admin_user());
    let response = app
        .oneshot(post_request("/api/analytics-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AnalyticsKeysState { pool: None }, admin_user());
    let response = app
        .oneshot(get_request("/api/analytics-api-keys"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn revoke_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AnalyticsKeysState { pool: None }, admin_user());
    let response = app
        .oneshot(delete_request(&format!(
            "/api/analytics-api-keys/{}",
            uuid::Uuid::new_v4()
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn import_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AnalyticsKeysState { pool: None }, admin_user());
    let response = app
        .oneshot(post_request(
            "/api/analytics-api-keys/import",
            r#"{"name": "legacy", "key": "legacy-secret"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// #[ignore], live DB
// ---------------------------------------------------------------------------

async fn live_pool() -> sqlx::PgPool {
    let conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live, migrated telemetry DB");
    sqlx::PgPool::connect(&conn_str)
        .await
        .expect("connecting to telemetry Postgres")
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading response body");
    serde_json::from_slice(&bytes).expect("parsing response body as json")
}

#[ignore]
#[tokio::test]
async fn live_mint_list_revoke_round_trip() {
    let pool = live_pool().await;
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(pool.clone()),
        },
        admin_user(),
    );

    let name = format!("analytics-keys-test-{}", uuid::Uuid::new_v4());
    let response = app
        .clone()
        .oneshot(post_request(
            "/api/analytics-api-keys",
            &format!(r#"{{"name": "{name}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key = body["key"].as_str().expect("key present").to_string();
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();
    assert!(key.starts_with("mmk_"));

    let response = app
        .clone()
        .oneshot(get_request("/api/analytics-api-keys"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let raw = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading body");
    assert!(
        !String::from_utf8_lossy(&raw).contains(&key),
        "response must never include the cleartext key"
    );

    let response = app
        .clone()
        .oneshot(delete_request(&format!("/api/analytics-api-keys/{key_id}")))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["revoked_at"].is_string());

    sqlx::query("DELETE FROM analytics_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[ignore]
#[tokio::test]
async fn live_import_is_idempotent() {
    let pool = live_pool().await;
    let app = build_handler_router_with_user(
        AnalyticsKeysState {
            pool: Some(pool.clone()),
        },
        admin_user(),
    );

    let name = format!("analytics-keys-import-test-{}", uuid::Uuid::new_v4());
    let key = format!("legacy-{}", uuid::Uuid::new_v4());

    let response = app
        .clone()
        .oneshot(post_request(
            "/api/analytics-api-keys/import",
            &format!(r#"{{"name": "{name}", "key": "{key}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["imported"], true);
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    let response = app
        .clone()
        .oneshot(post_request(
            "/api/analytics-api-keys/import",
            &format!(r#"{{"name": "{name}-again", "key": "{key}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["imported"], false);
    assert_eq!(body["key_id"].as_str(), Some(key_id.as_str()));

    sqlx::query("DELETE FROM analytics_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}
