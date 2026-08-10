// HTTP-level tests for `micromegas::servers::api_keys` — the ingestion
// key-management routes (#1383). Uses `tower::ServiceExt::oneshot` against a
// lazily-connected Postgres pool (never actually touched by the 403/400 cases,
// which all fail before any DB access), matching `public/tests/firehose_tests.rs`.
// The `AuthContext` is injected directly as a request extension (no middleware
// needed — that's `micromegas_auth::axum::auth_middleware`'s job in production).

use axum::body::Body;
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use micromegas::servers::api_keys::{ON_BEHALF_OF_HEADER, OnBehalfOfTrust, api_keys_router};
use micromegas_auth::db_api_key::DbApiKeyConfig;
use micromegas_auth::types::{AuthContext, AuthType};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;
use tower::ServiceExt;

/// Never actually reachable; a short `acquire_timeout` (the `firehose_tests.rs`
/// trick) keeps the `limit=1000` clamp test — which does reach the query, since
/// validation alone doesn't reject it — fast regardless of environment.
fn lazy_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn test_config() -> DbApiKeyConfig {
    DbApiKeyConfig {
        cache_size: 10,
        cache_ttl_secs: 60,
        unknown_cache_ttl_secs: 10,
        unknown_cache_size: 10,
    }
}

fn api_key_ctx() -> AuthContext {
    AuthContext {
        subject: "some-ingestion-key".to_string(),
        email: None,
        issuer: "api_key".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::ApiKey,
        is_admin: false,
        allow_delegation: true,
    }
}

fn non_admin_oidc_ctx() -> AuthContext {
    AuthContext {
        subject: "user".to_string(),
        email: Some("user@example.com".to_string()),
        issuer: "https://issuer.example.com".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        is_admin: false,
        allow_delegation: false,
    }
}

fn admin_oidc_ctx() -> AuthContext {
    AuthContext {
        subject: "admin".to_string(),
        email: Some("admin@example.com".to_string()),
        issuer: "https://issuer.example.com".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        is_admin: true,
        allow_delegation: false,
    }
}

/// No identity is trusted to set [`ON_BEHALF_OF_HEADER`] — the safe default,
/// and correct for every test in this file that never sends the header, or
/// that asserts the header is (still) ignored.
fn empty_trust() -> OnBehalfOfTrust {
    OnBehalfOfTrust::default()
}

/// Trusts exactly `admin_oidc_ctx()`'s own email — standing in for
/// `analytics-web-srv`'s proxy having been added to
/// `MICROMEGAS_INGESTION_ON_BEHALF_OF_TRUSTED_SUBJECTS` alongside the plain
/// admin list it must also be a member of.
fn trust_containing_admin_ctx() -> OnBehalfOfTrust {
    OnBehalfOfTrust::new(HashSet::from(["admin@example.com".to_string()]))
}

async fn call(
    router: axum::Router,
    ctx: AuthContext,
    mut request: Request<Body>,
) -> axum::response::Response {
    request.extensions_mut().insert(ctx);
    router.oneshot(request).await.expect("call service")
}

fn post_request(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
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

fn post_request_with_on_behalf_of(uri: &str, body: &str, on_behalf_of: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .header(ON_BEHALF_OF_HEADER, on_behalf_of)
        .body(Body::from(body.to_string()))
        .expect("build request")
}

fn delete_request_with_on_behalf_of(uri: &str, on_behalf_of: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(ON_BEHALF_OF_HEADER, on_behalf_of)
        .body(Body::empty())
        .expect("build request")
}

// ---------------------------------------------------------------------------
// The gate: 403 for a non-OIDC (API key) context and for a non-admin OIDC
// context, on every route. Both directions, since these are the whole gate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_403_for_api_key_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        api_key_ctx(),
        post_request("/auth/api_keys", r#"{"name": "x"}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_403_for_non_admin_oidc_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        non_admin_oidc_ctx(),
        post_request("/auth/api_keys", r#"{"name": "x"}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_403_for_api_key_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(router, api_key_ctx(), get_request("/auth/api_keys")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_403_for_non_admin_oidc_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(router, non_admin_oidc_ctx(), get_request("/auth/api_keys")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_403_for_api_key_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        api_key_ctx(),
        delete_request(&format!("/auth/api_keys/{}", uuid::Uuid::new_v4())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_403_for_non_admin_oidc_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        non_admin_oidc_ctx(),
        delete_request(&format!("/auth/api_keys/{}", uuid::Uuid::new_v4())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// `ON_BEHALF_OF_HEADER` is only ever *read*, never treated as its own
/// authentication signal — a non-admin OIDC caller that sets it is still
/// rejected before `actor()` (and therefore the header) is ever consulted.
/// This is what stops an arbitrary caller from spoofing an identity by
/// simply setting the header themselves.
#[tokio::test]
async fn mint_403_for_non_admin_oidc_context_even_with_on_behalf_of_header() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        non_admin_oidc_ctx(),
        post_request_with_on_behalf_of(
            "/auth/api_keys",
            r#"{"name": "x"}"#,
            "someone-else@example.com",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_403_for_non_admin_oidc_context_even_with_on_behalf_of_header() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        non_admin_oidc_ctx(),
        delete_request_with_on_behalf_of(
            &format!("/auth/api_keys/{}", uuid::Uuid::new_v4()),
            "someone-else@example.com",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Same for a non-OIDC (API key) caller: even an `is_admin`-adjacent-looking
/// header is irrelevant once `auth_type != Oidc` fails the gate first.
#[tokio::test]
async fn mint_403_for_api_key_context_even_with_on_behalf_of_header() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        api_key_ctx(),
        post_request_with_on_behalf_of(
            "/auth/api_keys",
            r#"{"name": "x"}"#,
            "someone-else@example.com",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_403_for_api_key_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        api_key_ctx(),
        post_request(
            "/auth/api_keys/import",
            r#"{"name": "legacy", "key": "legacy-secret"}"#,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_403_for_non_admin_oidc_context() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        non_admin_oidc_ctx(),
        post_request(
            "/auth/api_keys/import",
            r#"{"name": "legacy", "key": "legacy-secret"}"#,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// §4's 400 validation — checked before any hashing or DB access, so these run
// against the same never-really-touched pool as the 403 cases above.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_400_for_empty_name() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        post_request("/auth/api_keys", r#"{"name": ""}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_400_for_name_over_255_bytes() {
    let long_name = "a".repeat(256);
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        post_request("/auth/api_keys", &format!(r#"{{"name": "{long_name}"}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_400_for_zero_limit() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        get_request("/auth/api_keys?limit=0"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_400_for_negative_limit() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        get_request("/auth/api_keys?limit=-1"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// `limit=1000` must *not* 400 — the clamp accepts and rewrites the value
/// rather than rejecting it. This does reach the (unreachable) pool, so the
/// response here is some other status (a 500, since the DB is unreachable);
/// the clamp's effect on the resulting row count is covered by the live-DB
/// test below.
#[tokio::test]
async fn get_1000_limit_is_not_bad_request() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        get_request("/auth/api_keys?limit=1000"),
    )
    .await;
    assert_ne!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_400_for_empty_name() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        post_request(
            "/auth/api_keys/import",
            r#"{"name": "", "key": "legacy-secret"}"#,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_400_for_empty_key() {
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        post_request("/auth/api_keys/import", r#"{"name": "legacy", "key": ""}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_400_for_name_over_255_bytes() {
    let long_name = "a".repeat(256);
    let router = api_keys_router(lazy_pool(), test_config(), empty_trust());
    let response = call(
        router,
        admin_oidc_ctx(),
        post_request(
            "/auth/api_keys/import",
            &format!(r#"{{"name": "{long_name}", "key": "legacy-secret"}}"#),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// `api_keys_router` takes no table parameter, so no route in this module can
/// insert into `analytics_api_keys` by construction. This unit test asserts the
/// mint statement names `ingestion_api_keys` to keep it that way under
/// refactoring.
#[test]
fn mint_statement_names_ingestion_table_only() {
    let src = include_str!("../src/servers/api_keys.rs");
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
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live, migrated Postgres");
    sqlx::PgPool::connect(&conn_str)
        .await
        .expect("connecting to metadata Postgres")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading response body");
    serde_json::from_slice(&bytes).expect("parsing response body as json")
}

#[ignore]
#[tokio::test]
async fn live_mint_authenticates_and_list_hides_hash() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), empty_trust());

    let name = format!("api-keys-test-mint-{}", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request("/auth/api_keys", &format!(r#"{{"name": "{name}"}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key = body["key"].as_str().expect("key present").to_string();
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();
    assert!(key.starts_with("mmk_"));

    // The minted key authenticates through a DbApiKeyAuthProvider.
    let provider = micromegas_auth::db_api_key::DbApiKeyAuthProvider::new(
        pool.clone(),
        micromegas_auth::db_api_key::ApiKeyTable::Ingestion,
        test_config(),
    );
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {key}").parse().expect("valid header"),
    );
    let parts = micromegas_auth::types::HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().expect("valid uri"),
    };
    use micromegas_auth::types::AuthProvider as _;
    provider
        .validate_request(&parts as &dyn micromegas_auth::types::RequestParts)
        .await
        .expect("minted key should authenticate");

    // GET lists the key without key_hash or the cleartext key.
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        get_request("/auth/api_keys"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let raw = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading body");
    let raw_str = String::from_utf8_lossy(&raw);
    assert!(
        !raw_str.contains(&key),
        "response must never include the cleartext key"
    );
    let list: Value = serde_json::from_slice(&raw).expect("parsing response body as json");
    let list = list.as_array().expect("array").clone();
    let entry = list
        .iter()
        .find(|e| e["key_id"].as_str() == Some(key_id.as_str()))
        .expect("minted key present in listing");
    assert!(entry.get("key_hash").is_none());
    assert!(entry.get("key").is_none());

    // Clean up.
    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// The proxy calls in under its own service-credential identity
/// (`admin_oidc_ctx()` here stands in for that, and `trust_containing_admin_ctx()`
/// stands in for that identity having been added to
/// `MICROMEGAS_INGESTION_ON_BEHALF_OF_TRUSTED_SUBJECTS`), so without honoring
/// `ON_BEHALF_OF_HEADER`, `created_by` would be `admin_oidc_ctx()`'s own
/// `admin@example.com` for every proxied mint — collapsing the audit trail
/// onto one constant identity. Asserts `created_by` is the header's value
/// instead, once the caller has independently passed `require_key_admin` *and*
/// is a member of the trust set.
#[ignore]
#[tokio::test]
async fn live_mint_with_on_behalf_of_header_attributes_created_by_to_header_value() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), trust_containing_admin_ctx());

    let name = format!("api-keys-test-obo-mint-{}", uuid::Uuid::new_v4());
    let operator_email = format!("operator-{}@example.com", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request_with_on_behalf_of(
            "/auth/api_keys",
            &format!(r#"{{"name": "{name}"}}"#),
            &operator_email,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        get_request("/auth/api_keys"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let list = json_body(response).await;
    let entry = list
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key_id"].as_str() == Some(key_id.as_str()))
        .expect("minted key present in listing")
        .clone();
    assert_eq!(
        entry["created_by"].as_str(),
        Some(operator_email.as_str()),
        "created_by must reflect ON_BEHALF_OF_HEADER, not admin_oidc_ctx()'s own identity, \
         since admin_oidc_ctx() is a member of the trust set"
    );

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// The security-fix counterpart to the test above: `admin_oidc_ctx()` is an
/// ingestion-key admin (passes `require_key_admin`) but, with `empty_trust()`,
/// is *not* in the on-behalf-of trust set — the header must be silently
/// ignored, not honored and not rejected. `created_by` falls back to
/// `admin_oidc_ctx()`'s own identity, exactly as if the header were absent.
/// This is what stops any admin who isn't a specifically-provisioned trusted
/// forwarder (e.g. a colleague also holding an admin OIDC session) from
/// planting a key attributed to an arbitrary other identity.
#[ignore]
#[tokio::test]
async fn live_mint_with_on_behalf_of_header_is_ignored_for_untrusted_admin() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), empty_trust());

    let name = format!("api-keys-test-obo-untrusted-mint-{}", uuid::Uuid::new_v4());
    let spoofed_email = format!("spoofed-{}@example.com", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request_with_on_behalf_of(
            "/auth/api_keys",
            &format!(r#"{{"name": "{name}"}}"#),
            &spoofed_email,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        get_request("/auth/api_keys"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let list = json_body(response).await;
    let entry = list
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key_id"].as_str() == Some(key_id.as_str()))
        .expect("minted key present in listing")
        .clone();
    assert_eq!(
        entry["created_by"].as_str(),
        Some("admin@example.com"),
        "created_by must fall back to the caller's own identity — an admin outside the \
         trust set must never be able to set created_by via the header"
    );
    assert_ne!(
        entry["created_by"].as_str(),
        Some(spoofed_email.as_str()),
        "the header must not be honored for a caller outside the trust set"
    );

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// Same attribution fix, for `revoked_by` via `revoke_key`.
#[ignore]
#[tokio::test]
async fn live_revoke_with_on_behalf_of_header_attributes_revoked_by_to_header_value() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), trust_containing_admin_ctx());

    let name = format!("api-keys-test-obo-revoke-{}", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request("/auth/api_keys", &format!(r#"{{"name": "{name}"}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    let operator_email = format!("operator-{}@example.com", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        delete_request_with_on_behalf_of(&format!("/auth/api_keys/{key_id}"), &operator_email),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        get_request("/auth/api_keys"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let list = json_body(response).await;
    let entry = list
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key_id"].as_str() == Some(key_id.as_str()))
        .expect("revoked key present in listing")
        .clone();
    assert_eq!(
        entry["revoked_by"].as_str(),
        Some(operator_email.as_str()),
        "revoked_by must reflect ON_BEHALF_OF_HEADER, not admin_oidc_ctx()'s own identity, \
         since admin_oidc_ctx() is a member of the trust set"
    );

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// The security-fix counterpart, for `revoked_by`: `admin_oidc_ctx()` is an
/// admin but not a trusted forwarder, so the header must be ignored and
/// `revoked_by` must fall back to their own identity — a colleague's revoke
/// action must never be attributable to an arbitrary other identity that an
/// admin merely typed into the header.
#[ignore]
#[tokio::test]
async fn live_revoke_with_on_behalf_of_header_is_ignored_for_untrusted_admin() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), empty_trust());

    let name = format!(
        "api-keys-test-obo-untrusted-revoke-{}",
        uuid::Uuid::new_v4()
    );
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request("/auth/api_keys", &format!(r#"{{"name": "{name}"}}"#)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    let spoofed_email = format!("spoofed-{}@example.com", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        delete_request_with_on_behalf_of(&format!("/auth/api_keys/{key_id}"), &spoofed_email),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        get_request("/auth/api_keys"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let list = json_body(response).await;
    let entry = list
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key_id"].as_str() == Some(key_id.as_str()))
        .expect("revoked key present in listing")
        .clone();
    assert_eq!(
        entry["revoked_by"].as_str(),
        Some("admin@example.com"),
        "revoked_by must fall back to the caller's own identity — an admin outside the \
         trust set must never be able to set revoked_by via the header"
    );
    assert_ne!(
        entry["revoked_by"].as_str(),
        Some(spoofed_email.as_str()),
        "the header must not be honored for a caller outside the trust set"
    );

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[ignore]
#[tokio::test]
async fn live_get_limit_clamp_returns_exactly_500() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), empty_trust());

    let prefix = format!("api-keys-test-clamp-{}", uuid::Uuid::new_v4());
    let mut key_ids = Vec::new();
    for i in 0..501 {
        let response = call(
            router.clone(),
            admin_oidc_ctx(),
            post_request("/auth/api_keys", &format!(r#"{{"name": "{prefix}-{i}"}}"#)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        key_ids.push(body["key_id"].as_str().expect("key_id").to_string());
    }

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        get_request("/auth/api_keys?limit=1000"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let list = json_body(response).await;
    let count = list.as_array().expect("array").len();
    assert_eq!(count, 500, "limit=1000 must clamp to exactly 500 rows");

    for key_id in key_ids {
        sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
            .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
            .execute(&pool)
            .await
            .expect("cleanup");
    }
}

#[ignore]
#[tokio::test]
async fn live_import_inserts_then_idempotent_reimport() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), empty_trust());

    let name = format!("api-keys-test-import-{}", uuid::Uuid::new_v4());
    let key = format!("legacy-{}", uuid::Uuid::new_v4());

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request(
            "/auth/api_keys/import",
            &format!(r#"{{"name": "{name}", "key": "{key}"}}"#),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["imported"], true);
    assert_eq!(body["revoked_at"], Value::Null);
    let key_id = body["key_id"].as_str().expect("key_id present").to_string();

    // The imported key authenticates, same as a minted key.
    let provider = micromegas_auth::db_api_key::DbApiKeyAuthProvider::new(
        pool.clone(),
        micromegas_auth::db_api_key::ApiKeyTable::Ingestion,
        test_config(),
    );
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {key}").parse().expect("valid header"),
    );
    let parts = micromegas_auth::types::HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().expect("valid uri"),
    };
    use micromegas_auth::types::AuthProvider as _;
    provider
        .validate_request(&parts as &dyn micromegas_auth::types::RequestParts)
        .await
        .expect("imported key should authenticate");

    // Re-importing the same key string is idempotent: same key_id, imported: false.
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request(
            "/auth/api_keys/import",
            &format!(r#"{{"name": "{name}-again", "key": "{key}"}}"#),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["imported"], false);
    assert_eq!(body["key_id"].as_str(), Some(key_id.as_str()));
    // The original name is preserved — the re-import did not overwrite it.
    assert_eq!(body["name"].as_str(), Some(name.as_str()));

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[ignore]
#[tokio::test]
async fn live_delete_is_idempotent_and_404_for_unknown() {
    let pool = live_pool().await;
    let router = api_keys_router(pool.clone(), test_config(), empty_trust());

    let name = format!("api-keys-test-delete-{}", uuid::Uuid::new_v4());
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        post_request("/auth/api_keys", &format!(r#"{{"name": "{name}"}}"#)),
    )
    .await;
    let body = json_body(response).await;
    let key_id = body["key_id"].as_str().expect("key_id").to_string();

    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        delete_request(&format!("/auth/api_keys/{key_id}")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let first_body = json_body(response).await;
    let first_revoked_at = first_body["revoked_at"]
        .as_str()
        .expect("revoked_at")
        .to_string();

    // A second DELETE is idempotent and preserves the original revoked_at.
    let response = call(
        router.clone(),
        admin_oidc_ctx(),
        delete_request(&format!("/auth/api_keys/{key_id}")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let second_body = json_body(response).await;
    assert_eq!(
        second_body["revoked_at"].as_str(),
        Some(first_revoked_at.as_str())
    );

    // DELETE of an unknown key_id returns 404.
    let response = call(
        router,
        admin_oidc_ctx(),
        delete_request(&format!("/auth/api_keys/{}", uuid::Uuid::new_v4())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    sqlx::query("DELETE FROM ingestion_api_keys WHERE key_id = $1")
        .bind(uuid::Uuid::parse_str(&key_id).expect("valid uuid"))
        .execute(&pool)
        .await
        .expect("cleanup");
}
