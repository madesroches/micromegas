//! Tests for `ingestion_keys.rs` — the ingestion-key management routes
//! (#1458), a straight structural copy of `analytics_keys_tests.rs` retargeted
//! at `ingestion_api_keys`. Extended for the audience column (#1372, AbAC
//! Stage 4): `resolve_audience`'s resolution matrix, and the route-level
//! precedence/400 coverage only a route can add.
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
//! and the `NotConfigured` cases use `IngestionKeysState { pool: None, .. }`,
//! which never touches `state.pool` at all. Live-DB round trips for
//! mint/list/revoke/import are `#[ignore]`d, run manually against a real
//! Postgres per `folders_tests.rs`'s precedent.
//!
//! §5's resolution matrix (explicit / knob / `import`'s `PUBLIC_AUDIENCE`
//! fallback / `mint`'s 400) is tested against `resolve_audience` directly,
//! not through the routes: the helper is sync, takes no pool, and every
//! route-level test in this file that reaches an `INSERT` is `#[ignore]`d, so
//! testing the helper is what keeps the resolution matrix in default
//! `cargo test`.

use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use analytics_web_srv::ingestion_keys::{
    IngestionKeysState, ingestion_keys_router, resolve_audience,
};
use axum::{Extension, Router, body::Body, http::Request, http::StatusCode};
use micromegas::auth::policy::PUBLIC_AUDIENCE;
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

/// Wires `ingestion_keys_router` the same way `build_protected_routes` does
/// (state layered as `Extension<IngestionKeysState>`), with auth bypassed by
/// pre-inserting a synthetic `ValidatedUser` — the same shape `--disable-auth`
/// uses — instead of standing up an OIDC mock and running
/// `cookie_auth_middleware` for real.
fn build_handler_router_with_user(state: IngestionKeysState, user: ValidatedUser) -> Router {
    ingestion_keys_router("")
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
// resolve_audience -- §5's resolution matrix, unit-tested with no pool
// ---------------------------------------------------------------------------

#[test]
fn resolve_audience_uses_the_explicit_request_value_over_the_knob() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: Some("knob-audience".to_string()),
    };
    let resolved = resolve_audience(&state, Some("explicit-audience"), None).expect("resolve");
    assert_eq!(resolved, "explicit-audience");
}

#[test]
fn resolve_audience_falls_back_to_the_knob_when_no_explicit_value() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: Some("knob-audience".to_string()),
    };
    let resolved = resolve_audience(&state, None, None).expect("resolve");
    assert_eq!(resolved, "knob-audience");
}

#[test]
fn resolve_audience_mint_errors_when_neither_explicit_nor_knob_is_set() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: None,
    };
    let result = resolve_audience(&state, None, None);
    assert!(
        result.is_err(),
        "mint's fallback is None: an unresolved audience must be a BadRequest, never a silent default"
    );
}

#[test]
fn resolve_audience_import_falls_back_to_public_when_neither_explicit_nor_knob_is_set() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: None,
    };
    let resolved =
        resolve_audience(&state, None, Some(PUBLIC_AUDIENCE)).expect("import falls back");
    assert_eq!(resolved, PUBLIC_AUDIENCE);
}

/// A missing field or an empty string both count as absent.
#[test]
fn resolve_audience_treats_an_empty_string_request_as_absent() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: None,
    };
    let resolved =
        resolve_audience(&state, Some(""), Some(PUBLIC_AUDIENCE)).expect("import falls back");
    assert_eq!(resolved, PUBLIC_AUDIENCE);
}

/// An explicit audience is taken verbatim -- no case folding -- and validated.
#[test]
fn resolve_audience_rejects_an_invalid_explicit_value() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: None,
    };
    let result = resolve_audience(&state, Some("not valid!"), None);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// The gate: 403 for a non-admin `ValidatedUser`, on every route. The
// `AdminUser` extractor rejects before any handler body runs — never
// touches the pool.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_403_for_non_admin() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_403_for_non_admin() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/ingestion-api-keys"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoke_403_for_non_admin() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/ingestion-api-keys/{}",
            uuid::Uuid::new_v4()
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_403_for_non_admin() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys/import",
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
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": ""}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_400_for_empty_key() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys/import",
            r#"{"name": "legacy", "key": ""}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_400_for_negative_limit() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/ingestion-api-keys?limit=-1"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// An invalid `audience` in the request body is a 400 raised before any DB access -- exercised
/// with a merely-lazy pool, same as every other 400 case in this file.
#[tokio::test]
async fn mint_400_for_invalid_audience() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            r#"{"name": "x", "audience": "not valid!"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// With neither an explicit `audience` nor `MICROMEGAS_DEFAULT_KEY_AUDIENCE` configured, `mint`
/// is a 400 whose body names the knob an operator needs to set.
#[tokio::test]
async fn mint_400_names_the_default_audience_knob() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["message"].as_str().expect("message present");
    assert!(
        message.contains("MICROMEGAS_DEFAULT_KEY_AUDIENCE"),
        "expected the 400 body to name the knob, got: {message}"
    );
}

// ---------------------------------------------------------------------------
// NotConfigured 503 — `IngestionKeysState { pool: None, .. }` never touches
// the pool, so this needs no live DB either, same harness as every other
// test here. These are also the regression test for the
// `require_pool` → `validate_name` → `resolve_audience` precedence: neither
// request below carries an `audience`, and both must still 503, not the new
// 400 `resolve_audience` could otherwise raise.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: None,
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: None,
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(get_request("/api/ingestion-api-keys"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn revoke_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: None,
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(delete_request(&format!(
            "/api/ingestion-api-keys/{}",
            uuid::Uuid::new_v4()
        )))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn import_503_when_pool_unconfigured() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: None,
            default_audience: None,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys/import",
            r#"{"name": "legacy", "key": "legacy-secret"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// Source-scan regression guard
// ---------------------------------------------------------------------------

/// Mirrors `analytics_keys_tests.rs`'s
/// `mint_statement_names_analytics_table_only`: `ingestion_keys_router` takes
/// no table parameter, so no route in this module can insert into
/// `analytics_api_keys` by construction. This unit test asserts the mint
/// statement names `ingestion_api_keys` to keep it that way under
/// refactoring.
#[test]
fn mint_statement_names_ingestion_table_only() {
    let src = include_str!("../src/ingestion_keys.rs");
    assert!(
        src.contains("INSERT INTO ingestion_api_keys"),
        "expected the mint statement to name ingestion_api_keys"
    );
    // Doc comments are allowed to *mention* analytics_api_keys (to explain why
    // it's out of scope); no SQL statement in this module may write to it.
    assert!(
        !src.contains("INTO analytics_api_keys") && !src.contains("UPDATE analytics_api_keys"),
        "this module must never write to analytics_api_keys"
    );
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
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
        },
        admin_user(),
    );

    let name = format!("ingestion-keys-test-{}", uuid::Uuid::new_v4());
    let response = app
        .clone()
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "{name}", "audience": "team-alpha"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key = body["key"].as_str().expect("key present").to_string();
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();
    assert!(key.starts_with("mmk_"));
    assert_eq!(body["audience"].as_str(), Some("team-alpha"));

    let response = app
        .clone()
        .oneshot(get_request("/api/ingestion-api-keys"))
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

    // Attribution fix regression: the direct-write path must record the
    // acting admin's own identity, not a shared service credential.
    assert!(
        String::from_utf8_lossy(&raw).contains("admin@example.com"),
        "listed row should attribute created_by to the acting admin"
    );
    // The audience round-trips through `KeyListEntry`.
    assert!(
        String::from_utf8_lossy(&raw).contains("team-alpha"),
        "listed row should carry the audience it was minted with"
    );

    let response = app
        .clone()
        .oneshot(delete_request(&format!("/api/ingestion-api-keys/{key_id}")))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["revoked_at"].is_string());

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
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
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
        },
        admin_user(),
    );

    let name = format!("ingestion-keys-import-test-{}", uuid::Uuid::new_v4());
    let key = format!("legacy-{}", uuid::Uuid::new_v4());

    let response = app
        .clone()
        .oneshot(post_request(
            "/api/ingestion-api-keys/import",
            &format!(r#"{{"name": "{name}", "key": "{key}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["imported"], true);
    assert_eq!(body["audience"].as_str(), Some(PUBLIC_AUDIENCE));
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    // Same key, a different name AND a different audience this time: the binding is
    // immutable, so the already-present row's original audience must survive, never the
    // second request's.
    let response = app
        .clone()
        .oneshot(post_request(
            "/api/ingestion-api-keys/import",
            &format!(r#"{{"name": "{name}-again", "key": "{key}", "audience": "team-alpha"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["imported"], false);
    assert_eq!(body["key_id"].as_str(), Some(key_id.as_str()));
    assert_eq!(
        body["audience"].as_str(),
        Some(PUBLIC_AUDIENCE),
        "an import of an already-present key must report the existing audience, never the \
         request's"
    );

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}
