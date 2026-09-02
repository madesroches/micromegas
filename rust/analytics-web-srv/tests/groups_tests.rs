//! Tests for `groups.rs` — the group-admin routes.
//!
//! Modeled on `analytics_keys_tests.rs`'s pattern: routes are wired the same way
//! `build_protected_routes` wires them, but with `cookie_auth_middleware` bypassed by
//! pre-inserting a synthetic `ValidatedUser` extension (the same shape `--disable-auth` uses) --
//! this exercises the real `AdminUser` extractor, not a handler-as-function call.
//!
//! Every test here uses a lazily-connected pool and never actually reaches the database: the
//! 403 cases are rejected by the `AdminUser` extractor before any handler runs, the 400 cases
//! fail validation before touching the pool, and the `admins`-deletion 409 case is refused by a
//! pure name check before the first query. The CRUD round trip, the cycle/delete-while-referenced
//! conflict responses, and the missing-group 404 are verified manually (per the plan's Testing
//! Strategy), the same way `audience_grants_tests.rs`'s held-pair/visibility cases are.

use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use analytics_web_srv::groups::{GroupsState, groups_router};
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

fn build_handler_router_with_user(state: GroupsState, user: ValidatedUser) -> Router {
    groups_router("")
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

fn state_with_pool() -> GroupsState {
    GroupsState {
        pool: Some(lazy_pool()),
    }
}

// ---------------------------------------------------------------------------
// The gate: 403 for a non-admin `ValidatedUser`, on every route. `AdminUser` rejects before any
// handler body runs -- never touches the pool.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_groups_403_for_non_admin() {
    let app = build_handler_router_with_user(state_with_pool(), non_admin_user());
    let response = app
        .oneshot(get_request("/api/groups"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_group_403_for_non_admin() {
    let app = build_handler_router_with_user(state_with_pool(), non_admin_user());
    let response = app
        .oneshot(post_request("/api/groups", r#"{"name": "eng"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_group_403_for_non_admin() {
    let app = build_handler_router_with_user(state_with_pool(), non_admin_user());
    let response = app
        .oneshot(delete_request("/api/groups/eng"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_members_403_for_non_admin() {
    let app = build_handler_router_with_user(state_with_pool(), non_admin_user());
    let response = app
        .oneshot(get_request("/api/groups/eng/members"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn add_member_403_for_non_admin() {
    let app = build_handler_router_with_user(state_with_pool(), non_admin_user());
    let response = app
        .oneshot(post_request(
            "/api/groups/eng/members",
            r#"{"member": "user:alice@example.com"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remove_member_403_for_non_admin() {
    let app = build_handler_router_with_user(state_with_pool(), non_admin_user());
    let response = app
        .oneshot(delete_request(
            "/api/groups/eng/members?member=user:alice@example.com",
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 400 validation -- checked before any DB access.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_group_400_for_bad_name() {
    let app = build_handler_router_with_user(state_with_pool(), admin_user());
    let response = app
        .oneshot(post_request(
            "/api/groups",
            r#"{"name": "not a valid name"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_member_400_for_bad_selector() {
    let app = build_handler_router_with_user(state_with_pool(), admin_user());
    let response = app
        .oneshot(post_request(
            "/api/groups/eng/members",
            r#"{"member": "not-a-selector"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_member_400_for_overlong_selector() {
    let app = build_handler_router_with_user(state_with_pool(), admin_user());
    let overlong = format!("user:{}@example.com", "a".repeat(260));
    let response = app
        .oneshot(post_request(
            "/api/groups/eng/members",
            &format!(r#"{{"member": {overlong:?}}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// The `admins` lockout guard: deleting the group itself is refused by a pure name check, before
// any query runs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_admins_group_409() {
    let app = build_handler_router_with_user(state_with_pool(), admin_user());
    let response = app
        .oneshot(delete_request("/api/groups/admins"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// NotConfigured -- state.pool == None never touches a pool at all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_groups_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(GroupsState { pool: None }, admin_user());
    let response = app
        .oneshot(get_request("/api/groups"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
