//! Tests for `audience_grants.rs` — the audience-grant routes (#1489, AbAC Stage 6a; self-service
//! write policy #1510, AbAC Stage 6c).
//!
//! Modeled on `ingestion_keys_tests.rs`'s pattern exactly: routes are wired the same way
//! `build_protected_routes` wires them, but with `cookie_auth_middleware` bypassed by
//! pre-inserting a synthetic `ValidatedUser` extension (the same shape `--disable-auth` uses) --
//! this exercises the real `GrantGate`/`AuthenticatedUser` extractors, unlike a plain
//! `Extension(test_user())` handler-as-function call, which never runs an extractor at all.
//!
//! Every non-`#[ignore]`d test here uses a lazily-connected pool (`sqlx::PgPool::connect_lazy`)
//! and never actually reaches the database: the knob-off 403 and the non-admin `*`-selector 403
//! are both rejected before any query runs -- the former by `GrantGate` itself before the
//! handler body starts, the latter by `create_grant`'s own check, which runs before the
//! per-pair hold check that would otherwise touch the pool. The 400 cases fail validation before
//! touching the pool, and the `NotConfigured` cases use `AudienceGrantsState { pool: None,
//! self_service_mint_enabled: false }`, which never touches `state.pool` at all. Every other §3
//! branch -- held-pair create, own-row/created-by delete, `GET .../visible`'s three visibility
//! cases -- reaches Postgres and is `#[ignore]`d, run manually against a real, v7-migrated
//! Postgres, per `ingestion_keys_tests.rs`'s precedent.

use analytics_web_srv::audience_grants::{
    AudienceGrantsState, audience_grants_router, mint_prefix_for,
};
use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use axum::{Extension, Router, body::Body, http::Request, http::StatusCode};
use micromegas::auth::types::{AuthContext, AuthType};
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

/// Builds an `AuthContext` mirroring a `ValidatedUser` -- the same shape `cookie_auth_middleware`
/// inserts alongside `ValidatedUser` in production (`auth/handlers.rs`). Mirrors
/// `auth/tests/policy_tests.rs::caller`'s field defaults (it isn't exported); duplicated verbatim
/// per this crate's existing convention of mirroring rather than sharing such helpers across
/// `tests/*.rs` files, since each file in `tests/` is a separate crate.
fn auth_context_for(user: &ValidatedUser, groups: Vec<String>) -> AuthContext {
    AuthContext {
        subject: user.subject.clone(),
        email: user.email.clone(),
        issuer: user.issuer.clone(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        is_admin: user.is_admin,
        allow_delegation: false,
        bound_audience: None,
        read_audiences: vec![],
        groups,
    }
}

/// Wires `audience_grants_router` the same way `build_protected_routes` does (state layered as
/// `Extension<AudienceGrantsState>`), with auth bypassed by pre-inserting a synthetic
/// `ValidatedUser` -- the same shape `--disable-auth` uses -- instead of standing up an OIDC mock
/// and running `cookie_auth_middleware` for real. Also layers a matching `AuthContext` (AbAC
/// Stage 6, #1374): `AuthenticatedUser` (used by `/my-audiences`) reads `AuthContext`, not
/// `ValidatedUser`, so without this every `/my-audiences` test would otherwise hit the
/// `Unauthenticated` rejection.
fn build_handler_router_with_user(state: AudienceGrantsState, user: ValidatedUser) -> Router {
    build_handler_router_with_user_and_groups(state, user, vec![])
}

/// Same as [`build_handler_router_with_user`], with an explicit `groups` list on the layered
/// `AuthContext` -- needed only by `/my-audiences` tests exercising a `group:<g>` mint selector.
fn build_handler_router_with_user_and_groups(
    state: AudienceGrantsState,
    user: ValidatedUser,
    groups: Vec<String>,
) -> Router {
    let auth_context = auth_context_for(&user, groups);
    audience_grants_router("")
        .layer(Extension(state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
        .layer(Extension(auth_context))
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
// The gate: 403 for a non-admin `ValidatedUser` when the self-service knob is off, on both
// write routes. `GrantGate` rejects before the handler body runs -- never touches the pool.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_403_for_non_admin_when_knob_disabled() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: false,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "read", "selector": "user:reader@example.com"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_grant_403_for_non_admin_when_knob_disabled() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: false,
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
// A non-admin caller may never grant `*` -- rejected by `create_grant`'s own check, before the
// per-pair hold check that would otherwise touch the pool (Design §3).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_403_for_non_admin_star_selector() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: true,
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

// ---------------------------------------------------------------------------
// 400 validation -- checked before any DB access.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_400_for_invalid_axis() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: false,
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
            self_service_mint_enabled: false,
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
            self_service_mint_enabled: false,
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

// ---------------------------------------------------------------------------
// /my-audiences -- AbAC Stage 6, #1374, Design §5. The knob-off-non-admin case is rejected by
// `my_audiences`'s own gate check before `require_pool`/any DB access, so it needs no live DB,
// same harness as every 403 case above; a knob-off *admin* is exempt from that gate and does
// reach the DB query, so that case is a live-DB test alongside the others, below.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn my_audiences_403_for_non_admin_when_knob_disabled() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: false,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/my-audiences"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn my_audiences_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: None,
            self_service_mint_enabled: true,
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/my-audiences"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// mint_prefix_for -- pure, sync (AbAC Stage 6, #1374, Design §5); no DB or `AuthContext` needed.
// ---------------------------------------------------------------------------

#[test]
fn mint_prefix_for_a_plain_local_part() {
    assert_eq!(
        mint_prefix_for(&Some("alice@example.com".to_string())),
        Some("alice-".to_string())
    );
}

#[test]
fn mint_prefix_for_sanitizes_dots_and_pluses() {
    assert_eq!(
        mint_prefix_for(&Some("alice.smith+ci@example.com".to_string())),
        Some("alice-smith-ci-".to_string())
    );
}

#[test]
fn mint_prefix_for_lowercases_a_mixed_case_address() {
    assert_eq!(
        mint_prefix_for(&Some("Alice.Smith@Example.com".to_string())),
        Some("alice-smith-".to_string())
    );
}

#[test]
fn mint_prefix_for_none_when_local_part_sanitizes_to_empty() {
    assert_eq!(mint_prefix_for(&Some("+++@example.com".to_string())), None);
}

#[test]
fn mint_prefix_for_none_when_email_is_none() {
    assert_eq!(mint_prefix_for(&None), None);
}

// ---------------------------------------------------------------------------
// NotConfigured 503 -- `AudienceGrantsState { pool: None, self_service_mint_enabled: false }`
// never touches the pool, so this needs no live DB either, same harness as
// every other test here.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_grant_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: None,
            self_service_mint_enabled: false,
        },
        admin_user(),
    );
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
async fn delete_grant_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: None,
            self_service_mint_enabled: false,
        },
        admin_user(),
    );
    let response = app
        .oneshot(delete_request(
            "/api/audience-grants?audience=team-alpha&axis=read&selector=%2A",
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn visible_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: None,
            self_service_mint_enabled: false,
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/visible"))
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

/// Reads the rows for one audience directly via `sqlx` -- the round trip below no longer has a
/// list route to read back through (`list_grants` is deleted, Design §3); ad-hoc auditing now
/// goes through `micromegas-query`/`list_audience_grants()` instead.
async fn direct_read(pool: &sqlx::PgPool, audience: &str) -> Vec<(String, String, String)> {
    sqlx::query_as::<_, (String, String, String)>(
        "SELECT audience, axis, selector FROM audience_grants WHERE audience = $1 \
         ORDER BY axis, selector",
    )
    .bind(audience)
    .fetch_all(pool)
    .await
    .expect("direct read")
}

#[ignore]
#[tokio::test]
async fn live_create_delete_round_trip() {
    let pool = live_pool().await;
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: false,
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

    let rows = direct_read(&pool, &audience).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2, "group:eng");

    let response = app
        .clone()
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=group%3Aeng"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert!(direct_read(&pool, &audience).await.is_empty());

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

// ---------------------------------------------------------------------------
// #[ignore], live DB -- non-admin write policy (#1510, AbAC Stage 6c, Design §3)
// ---------------------------------------------------------------------------

async fn insert_grant(
    pool: &sqlx::PgPool,
    audience: &str,
    axis: &str,
    selector: &str,
    created_by: &str,
) {
    sqlx::query(
        "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
         VALUES ($1, $2, $3, now(), $4)",
    )
    .bind(audience)
    .bind(axis)
    .bind(selector)
    .bind(created_by)
    .execute(pool)
    .await
    .expect("insert grant");
}

async fn cleanup_grants(pool: &sqlx::PgPool, audience: &str) {
    let _ = sqlx::query("DELETE FROM audience_grants WHERE audience = $1")
        .bind(audience)
        .execute(pool)
        .await;
}

/// A non-admin with no matching grant on the pair at all is refused.
#[ignore]
#[tokio::test]
async fn live_create_grant_403_when_caller_does_not_hold_the_pair() {
    let pool = live_pool().await;
    let audience = format!("audience-grants-hold-test-{}", uuid::Uuid::new_v4());
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            &format!(
                r#"{{"audience": "{audience}", "axis": "read", "selector": "user:someone-else@example.com"}}"#
            ),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    cleanup_grants(&pool, &audience).await;
}

/// A non-admin who holds `(audience, axis)` only via a `group:` row may still share it with a
/// `user:`/`group:` selector -- holding through a group counts (Design §3).
#[ignore]
#[tokio::test]
async fn live_create_grant_201_when_caller_holds_the_pair_via_a_group() {
    let pool = live_pool().await;
    let audience = format!("audience-grants-group-hold-test-{}", uuid::Uuid::new_v4());
    insert_grant(&pool, &audience, "read", "group:eng", "test").await;

    let app = build_handler_router_with_user_and_groups(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
        vec!["eng".to_string()],
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            &format!(
                r#"{{"audience": "{audience}", "axis": "read", "selector": "user:teammate@example.com"}}"#
            ),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);

    cleanup_grants(&pool, &audience).await;
}

/// Removing one's own direct `user:<email>` row succeeds (204).
#[ignore]
#[tokio::test]
async fn live_delete_grant_204_for_own_row() {
    let pool = live_pool().await;
    let audience = format!("audience-grants-own-delete-test-{}", uuid::Uuid::new_v4());
    insert_grant(
        &pool,
        &audience,
        "read",
        "user:reader@example.com",
        "someone-else",
    )
    .await;

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=user%3Areader%40example.com"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    cleanup_grants(&pool, &audience).await;
}

/// Removing a row the caller created (but whose selector is a `group:`, not their own) succeeds
/// (204) -- the revoke-a-share counterpart of sharing.
#[ignore]
#[tokio::test]
async fn live_delete_grant_204_for_a_row_the_caller_created() {
    let pool = live_pool().await;
    let audience = format!(
        "audience-grants-created-by-delete-test-{}",
        uuid::Uuid::new_v4()
    );
    insert_grant(&pool, &audience, "read", "group:eng", "reader@example.com").await;

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=group%3Aeng"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    cleanup_grants(&pool, &audience).await;
}

/// A row that exists but is neither the caller's own nor one they created is a 403, not a 404.
#[ignore]
#[tokio::test]
async fn live_delete_grant_403_for_someone_elses_row() {
    let pool = live_pool().await;
    let audience = format!("audience-grants-others-row-test-{}", uuid::Uuid::new_v4());
    insert_grant(
        &pool,
        &audience,
        "read",
        "user:other@example.com",
        "other@example.com",
    )
    .await;

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=user%3Aother%40example.com"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    cleanup_grants(&pool, &audience).await;
}

/// A row that doesn't exist at all is a 404, for a non-admin exactly as for an admin.
#[ignore]
#[tokio::test]
async fn live_delete_grant_404_for_a_nonexistent_row() {
    let pool = live_pool().await;
    let audience = format!("audience-grants-absent-test-{}", uuid::Uuid::new_v4());

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/audience-grants?audience={audience}&axis=read&selector=user%3Areader%40example.com"
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// #[ignore], live DB -- GET .../visible (#1510, Design §3)
// ---------------------------------------------------------------------------

#[ignore]
#[tokio::test]
async fn live_visible_admin_sees_every_row() {
    let pool = live_pool().await;
    let audience = format!(
        "audience-grants-visible-admin-test-{}",
        uuid::Uuid::new_v4()
    );
    insert_grant(&pool, &audience, "read", "group:eng", "test").await;
    insert_grant(&pool, &audience, "mint", "user:carol@example.com", "test").await;

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: false,
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/visible"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let rows = json_body(response).await;
    let rows = rows.as_array().expect("array of grants");
    let matching = rows
        .iter()
        .filter(|r| r["audience"].as_str() == Some(audience.as_str()))
        .count();
    assert_eq!(matching, 2);

    cleanup_grants(&pool, &audience).await;
}

/// Non-admin, knob on: sees every grant on a pair it holds, including a sibling row it did not
/// create, but not a different pair.
#[ignore]
#[tokio::test]
async fn live_visible_non_admin_knob_on_sees_held_pairs() {
    let pool = live_pool().await;
    let audience = format!("audience-grants-visible-held-test-{}", uuid::Uuid::new_v4());
    insert_grant(&pool, &audience, "read", "user:reader@example.com", "test").await;
    insert_grant(&pool, &audience, "read", "user:sibling@example.com", "test").await;
    insert_grant(&pool, &audience, "mint", "user:other@example.com", "test").await;

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/visible"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let rows = json_body(response).await;
    let rows = rows.as_array().expect("array of grants");
    let selectors: Vec<&str> = rows
        .iter()
        .filter(|r| r["audience"].as_str() == Some(audience.as_str()))
        .map(|r| r["selector"].as_str().expect("selector"))
        .collect();
    assert_eq!(
        selectors.len(),
        2,
        "expected both rows on the held (audience, read) pair, got {selectors:?}"
    );
    assert!(selectors.contains(&"user:reader@example.com"));
    assert!(selectors.contains(&"user:sibling@example.com"));

    cleanup_grants(&pool, &audience).await;
}

/// Non-admin, knob off: narrows to the caller's own rows only -- never a sibling's, even on a
/// pair the caller holds.
#[ignore]
#[tokio::test]
async fn live_visible_non_admin_knob_off_sees_own_rows_only() {
    let pool = live_pool().await;
    let audience = format!(
        "audience-grants-visible-knob-off-test-{}",
        uuid::Uuid::new_v4()
    );
    insert_grant(&pool, &audience, "read", "user:reader@example.com", "test").await;
    insert_grant(&pool, &audience, "read", "user:sibling@example.com", "test").await;

    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: false,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/visible"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let rows = json_body(response).await;
    let rows = rows.as_array().expect("array of grants");
    let selectors: Vec<&str> = rows
        .iter()
        .filter(|r| r["audience"].as_str() == Some(audience.as_str()))
        .map(|r| r["selector"].as_str().expect("selector"))
        .collect();
    assert_eq!(selectors, vec!["user:reader@example.com"]);

    cleanup_grants(&pool, &audience).await;
}

#[ignore]
#[tokio::test]
async fn live_my_audiences_filters_by_selector_match_and_reports_is_admin() {
    let pool = live_pool().await;
    let audience = format!("my-audiences-test-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
         VALUES ($1, 'mint', 'user:reader@example.com', now(), 'test')",
    )
    .bind(&audience)
    .execute(&pool)
    .await
    .expect("insert grant");

    // The matching caller sees the audience, with `is_admin: false`.
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/my-audiences"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["is_admin"], false);
    let audiences = body["audiences"].as_array().expect("audiences array");
    assert!(
        audiences.iter().any(|a| a == &audience),
        "expected {audience:?} in {audiences:?}"
    );

    // A caller with no matching selector doesn't see it.
    let other = ValidatedUser {
        subject: "other".to_string(),
        email: Some("other@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
    };
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: true,
        },
        other,
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/my-audiences"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let audiences = body["audiences"].as_array().expect("audiences array");
    assert!(
        !audiences.iter().any(|a| a == &audience),
        "unexpected {audience:?} in {audiences:?}"
    );

    sqlx::query("DELETE FROM audience_grants WHERE audience = $1")
        .bind(&audience)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[ignore]
#[tokio::test]
async fn live_my_audiences_admin_gets_a_normal_response_regardless_of_knob() {
    let pool = live_pool().await;
    let app = build_handler_router_with_user(
        AudienceGrantsState {
            pool: Some(pool.clone()),
            self_service_mint_enabled: false,
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/audience-grants/my-audiences"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["is_admin"], true);
}
