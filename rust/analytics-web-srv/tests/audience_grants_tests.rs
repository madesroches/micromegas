//! Tests for `audience_grants.rs` — the audience-grant admin routes (#1489, AbAC Stage 6a).
//!
//! Modeled on `ingestion_keys_tests.rs`'s pattern exactly: routes are wired the same way
//! `build_protected_routes` wires them, but with `cookie_auth_middleware` bypassed by
//! pre-inserting a synthetic `ValidatedUser` extension (the same shape `--disable-auth` uses) --
//! this exercises the real `AdminUser` extractor, unlike a plain `Extension(test_user())`
//! handler-as-function call, which never runs an extractor at all.
//!
//! Every non-`#[ignore]`d test here uses a lazily-connected pool (`sqlx::PgPool::connect_lazy`)
//! and never actually reaches the database: the 403 cases are rejected by the `AdminUser`
//! extractor before any handler runs, the 400 cases fail validation before touching the pool, and
//! the `NotConfigured` cases use `AudienceGrantsState { pool: None }`, which never touches
//! `state.pool` at all. Live-DB round trips are `#[ignore]`d, run manually against a real,
//! v7-migrated Postgres, per `ingestion_keys_tests.rs`'s precedent.

use analytics_web_srv::audience_grants::{AudienceGrantsState, audience_grants_router};
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

/// Wires `audience_grants_router` the same way `build_protected_routes` does (state layered as
/// `Extension<AudienceGrantsState>`), with auth bypassed by pre-inserting a synthetic
/// `ValidatedUser` -- the same shape `--disable-auth` uses -- instead of standing up an OIDC mock
/// and running `cookie_auth_middleware` for real.
fn build_handler_router_with_user(state: AudienceGrantsState, user: ValidatedUser) -> Router {
    audience_grants_router("")
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
// `AdminUser` extractor rejects before any handler body runs -- never
// touches the pool.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "read", "selector": "*"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_grants_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_grant_403_for_non_admin() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(
            "/api/audience-grants?audience=team-alpha&axis=read&selector=%2A",
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 400 validation -- checked before any DB access.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_400_for_invalid_axis() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "write", "selector": "*"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_grant_400_for_invalid_audience() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "not valid!", "axis": "read", "selector": "*"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// `valid_selector` places no charset/length bound on a `group:<id>` selector, but the
/// `selector` column is `VARCHAR(255)` -- an over-long selector must be a `400`, not a `500` at
/// the `INSERT`.
#[tokio::test]
async fn create_grant_400_for_overlong_selector() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let overlong_selector = format!("group:{}", "x".repeat(255));
    let body = format!(
        r#"{{"audience": "team-alpha", "axis": "read", "selector": "{overlong_selector}"}}"#
    );
    let response = app
        .oneshot(post_request("/api/audience-grants", &body))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_grants_400_for_zero_limit() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants?limit=0"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_grants_400_for_negative_limit() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants?limit=-1"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_grants_400_for_negative_offset() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants?offset=-1"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// NotConfigured 503 -- `AudienceGrantsState { pool: None }` never touches the
// pool, so this needs no live DB either, same harness as every other test
// here.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AudienceGrantsState { pool: None }, admin_user());
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "read", "selector": "*"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_grants_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AudienceGrantsState { pool: None }, admin_user());
    let response = app
        .oneshot(get_request("/api/audience-grants"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn delete_grant_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(AudienceGrantsState { pool: None }, admin_user());
    let response = app
        .oneshot(delete_request(
            "/api/audience-grants?audience=team-alpha&axis=read&selector=%2A",
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
async fn live_create_list_delete_round_trip() {
    let pool = live_pool().await;
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
        },
        admin_user(),
    );

    let audience = format!("audience-grants-test-{}", uuid::Uuid::new_v4());
    let create_body =
        format!(r#"{{"audience": "{audience}", "axis": "read", "selector": "group:eng"}}"#);

    let response = app
        .clone()
        .oneshot(post_request("/api/audience-grants", &create_body))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["audience"].as_str(), Some(audience.as_str()));
    assert_eq!(body["axis"].as_str(), Some("read"));
    assert_eq!(body["selector"].as_str(), Some("group:eng"));

    // Re-creating the same triple reports the pre-existing row instead of erroring.
    let response = app
        .clone()
        .oneshot(post_request("/api/audience-grants", &create_body))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(get_request(&format!(
            "/api/audience-grants?audience={audience}"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let rows = json_body(response).await;
    let rows = rows.as_array().expect("array of grants");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["selector"].as_str(), Some("group:eng"));

    let response = app
        .clone()
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=group%3Aeng"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // A second delete of the now-gone row is a 404.
    let response = app
        .clone()
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=group%3Aeng"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
