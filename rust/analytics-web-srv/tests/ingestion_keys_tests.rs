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
/// inserts alongside `ValidatedUser` in production (`auth/handlers.rs`), AbAC Stage 6, #1374.
/// Mirrors `auth/tests/policy_tests.rs::caller`'s field defaults (it isn't exported); duplicated
/// verbatim per this crate's existing convention of mirroring rather than sharing such helpers
/// across `tests/*.rs` files, since each file in `tests/` is a separate crate.
fn auth_context_for(user: &ValidatedUser) -> AuthContext {
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
        groups: vec![],
    }
}

/// Wires `ingestion_keys_router` the same way `build_protected_routes` does
/// (state layered as `Extension<IngestionKeysState>`), with auth bypassed by
/// pre-inserting a synthetic `ValidatedUser` — the same shape `--disable-auth`
/// uses — instead of standing up an OIDC mock and running
/// `cookie_auth_middleware` for real. Also layers a matching `AuthContext` (AbAC Stage 6,
/// #1374): `mint_key` now runs through `MintGate`/`AuthenticatedUser`, which reads `AuthContext`,
/// not `ValidatedUser` -- without this, every mint test would hit the `Unauthenticated`
/// rejection instead of the denial/success path it actually means to exercise.
fn build_handler_router_with_user(state: IngestionKeysState, user: ValidatedUser) -> Router {
    let auth_context = auth_context_for(&user);
    ingestion_keys_router("")
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
// resolve_audience -- §5's resolution matrix, unit-tested with no pool
// ---------------------------------------------------------------------------

#[test]
fn resolve_audience_uses_the_explicit_request_value_over_the_knob() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: Some("knob-audience".to_string()),
        self_service_mint_enabled: false,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
    };
    let resolved = resolve_audience(&state, Some("explicit-audience"), None).expect("resolve");
    assert_eq!(resolved, "explicit-audience");
}

#[test]
fn resolve_audience_falls_back_to_the_knob_when_no_explicit_value() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: Some("knob-audience".to_string()),
        self_service_mint_enabled: false,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
    };
    let resolved = resolve_audience(&state, None, None).expect("resolve");
    assert_eq!(resolved, "knob-audience");
}

#[test]
fn resolve_audience_mint_errors_when_neither_explicit_nor_knob_is_set() {
    let state = IngestionKeysState {
        pool: None,
        default_audience: None,
        self_service_mint_enabled: false,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
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
        self_service_mint_enabled: false,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
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
        self_service_mint_enabled: false,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
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
        self_service_mint_enabled: false,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// The central ordering claim §4 makes: `MintGate` (a `FromRequestParts` extractor) runs, and
/// rejects a knob-off non-admin, *before* `Json<MintRequest>` ever parses the request body.
/// Malformed JSON as a knob-off non-admin must still be a 403, never axum's 422 for unparseable
/// JSON -- needs no DB, same as every other 400/403 case in this file.
#[tokio::test]
async fn mint_403_for_non_admin_before_body_is_parsed_even_with_malformed_json() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", "{not valid json"))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// With neither an explicit `audience` nor `default_audience` configured, a non-admin caller is
/// still a plain 400 from `resolve_audience` (§4) -- not the claim path (§4a), since there is no
/// explicit audience to claim. `self_service_mint_enabled: true` here so `MintGate` lets the
/// request past the knob check and actually reach `resolve_audience`; no DB access either way,
/// since `resolve_audience` fails before any query runs.
#[tokio::test]
async fn mint_400_for_non_admin_with_no_requested_audience_and_no_default() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request("/api/ingestion-api-keys", r#"{"name": "x"}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_403_for_non_admin() {
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(lazy_pool()),
            default_audience: None,
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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
            self_service_mint_enabled: false,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
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

// ---------------------------------------------------------------------------
// #[ignore], live DB -- self-service mint (AbAC Stage 6, #1374, Design §4/§4a)
// ---------------------------------------------------------------------------

async fn insert_mint_grant(pool: &sqlx::PgPool, audience: &str, selector: &str) {
    sqlx::query(
        "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
         VALUES ($1, 'mint', $2, now(), 'test')",
    )
    .bind(audience)
    .bind(selector)
    .execute(pool)
    .await
    .expect("insert mint grant");
}

async fn cleanup_audience(pool: &sqlx::PgPool, audience: &str) {
    sqlx::query("DELETE FROM ingestion_api_keys WHERE audience = $1")
        .bind(audience)
        .execute(pool)
        .await
        .expect("cleanup keys");
    sqlx::query("DELETE FROM audience_grants WHERE audience = $1")
        .bind(audience)
        .execute(pool)
        .await
        .expect("cleanup grants");
}

/// A non-admin caller with a real, already-committed `mint` grant for the requested audience
/// mints successfully via `MintPolicy::resolve_audience`'s per-request point query (§4) -- no
/// lazy claim attempted, so no extra `read` grant row is written alongside the pre-existing
/// `mint` one.
#[ignore]
#[tokio::test]
async fn live_mint_succeeds_for_non_admin_with_a_matching_grant_no_claim_attempted() {
    let pool = live_pool().await;
    let audience = format!("self-service-grant-test-{}", uuid::Uuid::new_v4());
    insert_mint_grant(&pool, &audience, "user:reader@example.com").await;

    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let name = format!("self-service-key-{}", uuid::Uuid::new_v4());
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "{name}", "audience": "{audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["audience"].as_str(), Some(audience.as_str()));

    let axes: Vec<String> = sqlx::query_scalar(
        "SELECT axis FROM audience_grants WHERE audience = $1 AND selector = 'user:reader@example.com'",
    )
    .bind(&audience)
    .fetch_all(&pool)
    .await
    .expect("query grants");
    assert_eq!(
        axes,
        vec!["mint".to_string()],
        "expected only the pre-existing mint grant -- no claim-written read grant"
    );

    cleanup_audience(&pool, &audience).await;
}

/// A non-admin caller with no grant at all, naming a brand-new audience explicitly, claims it
/// (§4a): both a `mint` and a `read` row for `user:<email>` land in `audience_grants`, and the
/// mint itself succeeds. A second, different non-admin caller with no grant then requesting the
/// same, now-claimed audience gets the ordinary "no grant" 403 -- ordinary denial, no second
/// claim attempted.
#[ignore]
#[tokio::test]
async fn live_mint_claims_a_fresh_audience_then_denies_a_second_caller() {
    let pool = live_pool().await;
    let audience = format!("self-service-claim-test-{}", uuid::Uuid::new_v4());

    let state = IngestionKeysState {
        pool: Some(pool.clone()),
        default_audience: None,
        self_service_mint_enabled: true,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
    };
    let app = build_handler_router_with_user(state.clone(), non_admin_user());
    let name = format!("self-service-claim-key-{}", uuid::Uuid::new_v4());
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "{name}", "audience": "{audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["audience"].as_str(), Some(audience.as_str()));

    let mut axes: Vec<String> = sqlx::query_scalar(
        "SELECT axis FROM audience_grants WHERE audience = $1 AND selector = 'user:reader@example.com'",
    )
    .bind(&audience)
    .fetch_all(&pool)
    .await
    .expect("query grants");
    axes.sort();
    assert_eq!(
        axes,
        vec!["mint".to_string(), "read".to_string()],
        "expected the claim to write both a mint and a read row"
    );

    let other = ValidatedUser {
        subject: "other".to_string(),
        email: Some("other@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
    };
    let app = build_handler_router_with_user(state, other);
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "other-attempt", "audience": "{audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a second caller with no grant on the now-claimed audience must get the ordinary denial"
    );

    cleanup_audience(&pool, &audience).await;
}

/// Two non-admin claims for the *same* fresh audience name, issued concurrently: the per-audience
/// advisory lock (§4a) lets exactly one proceed. The other gets either the retry-contention 409
/// (`CLAIM_CONTENDED`, if it loses the lock race) or the ordinary "no grant" 403 (if it observes
/// the winner's already-committed row) -- never a duplicate-owner outcome and never a 500.
#[ignore]
#[tokio::test]
async fn live_concurrent_claims_for_the_same_fresh_audience_never_double_claim() {
    let pool = live_pool().await;
    let audience = format!("self-service-concurrent-test-{}", uuid::Uuid::new_v4());

    let state = IngestionKeysState {
        pool: Some(pool.clone()),
        default_audience: None,
        self_service_mint_enabled: true,
        max_claims_per_caller: 25,
        max_keys_per_caller: 100,
    };
    let app1 = build_handler_router_with_user(state.clone(), non_admin_user());
    let app2 = build_handler_router_with_user(state, non_admin_user());

    let body = format!(r#"{{"name": "concurrent-key", "audience": "{audience}"}}"#);
    let (r1, r2) = tokio::join!(
        app1.oneshot(post_request("/api/ingestion-api-keys", &body)),
        app2.oneshot(post_request("/api/ingestion-api-keys", &body)),
    );
    let statuses = [
        r1.expect("call service").status(),
        r2.expect("call service").status(),
    ];
    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(created, 1, "exactly one concurrent claim should succeed");
    for status in &statuses {
        if *status != StatusCode::CREATED {
            assert!(
                *status == StatusCode::CONFLICT || *status == StatusCode::FORBIDDEN,
                "expected CONFLICT (lock contention) or FORBIDDEN (ordinary denial), got {status}"
            );
        }
    }

    cleanup_audience(&pool, &audience).await;
}

/// `public` is never claimable by a non-admin: the reserved-name check runs inside
/// `try_claim_and_mint`'s fresh-audience branch, after its own existence check (§4a) -- needs the
/// same live DB as the other claim tests, not the non-live-DB harness (since `public` typically
/// already carries rows in a real deployment, the denial can come from either check, but it is
/// always a 403 either way).
#[ignore]
#[tokio::test]
async fn live_mint_rejects_a_non_admin_claim_of_the_public_audience() {
    let pool = live_pool().await;
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(pool),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 25,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "x", "audience": "{PUBLIC_AUDIENCE}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// `max_claims_per_caller` (§3, §4a): a caller who has already claimed the configured limit gets
/// a `Forbidden` naming the limit on a claim of one more, distinct fresh audience; a caller one
/// below the limit still succeeds. Best-effort under sequential use, per that knob's own doc
/// comment.
#[ignore]
#[tokio::test]
async fn live_claims_limit_denies_at_the_bound_and_allows_one_below_it() {
    let pool = live_pool().await;
    let preclaimed_audience = format!("self-service-preclaimed-{}", uuid::Uuid::new_v4());
    for axis in ["mint", "read"] {
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ($1, $2, 'user:reader@example.com', now(), 'reader@example.com')",
        )
        .bind(&preclaimed_audience)
        .bind(axis)
        .execute(&pool)
        .await
        .expect("seed pre-existing claim");
    }

    // At the limit (1 already-claimed audience, limit 1): one more fresh claim is denied.
    let over_limit_audience = format!("self-service-over-claims-limit-{}", uuid::Uuid::new_v4());
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 1,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "x", "audience": "{over_limit_audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_body(response).await;
    let message = body["message"].as_str().expect("message present");
    assert!(
        message.contains('1'),
        "expected the 403 body to name the limit, got: {message}"
    );

    // One below the limit (limit 2): the same shape of claim succeeds.
    let under_limit_audience = format!("self-service-under-claims-limit-{}", uuid::Uuid::new_v4());
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 2,
            max_keys_per_caller: 100,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "y", "audience": "{under_limit_audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);

    cleanup_audience(&pool, &preclaimed_audience).await;
    cleanup_audience(&pool, &under_limit_audience).await;
}

/// `max_keys_per_caller` (§3, §4): a caller who already holds the configured limit of live keys
/// gets a `Forbidden` naming the limit on the next mint; a caller one below the limit still
/// succeeds. Both mints target an audience the caller already has a `mint` grant for, so the
/// per-caller key bound is exercised in isolation from the claim path.
#[ignore]
#[tokio::test]
async fn live_keys_limit_denies_at_the_bound_and_allows_one_below_it() {
    let pool = live_pool().await;
    let audience = format!("self-service-keys-limit-{}", uuid::Uuid::new_v4());
    insert_mint_grant(&pool, &audience, "user:reader@example.com").await;

    let seed_key_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
         VALUES ($1, $2, $3, now(), 'reader@example.com', $4)",
    )
    .bind(seed_key_id)
    .bind(vec![0u8; 32])
    .bind(format!("seed-key-{seed_key_id}"))
    .bind(&audience)
    .execute(&pool)
    .await
    .expect("seed live key");

    // At the limit (1 live key, limit 1): one more mint is denied.
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 25,
            max_keys_per_caller: 1,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "over-keys-limit", "audience": "{audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_body(response).await;
    let message = body["message"].as_str().expect("message present");
    assert!(
        message.contains('1'),
        "expected the 403 body to name the limit, got: {message}"
    );

    // One below the limit (limit 2): the same mint succeeds.
    let app = build_handler_router_with_user(
        IngestionKeysState {
            pool: Some(pool.clone()),
            default_audience: None,
            self_service_mint_enabled: true,
            max_claims_per_caller: 25,
            max_keys_per_caller: 2,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/ingestion-api-keys",
            &format!(r#"{{"name": "under-keys-limit", "audience": "{audience}"}}"#),
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::CREATED);

    cleanup_audience(&pool, &audience).await;
}
