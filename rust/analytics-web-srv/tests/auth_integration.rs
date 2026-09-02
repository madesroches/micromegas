//! Integration tests for auth endpoints
//!
//! These tests verify the auth endpoints work correctly with cookies.
//!
//! Note: Tests for auth_me and cookie_auth_middleware with JWT validation
//! require a mock OIDC server or environment with MICROMEGAS_OIDC_CONFIG set.
//! The signature validation tests are skipped in unit tests since they
//! require real JWKS endpoints.
//!
//! `cookie_auth_middleware_maps_group_store_outage_to_503` below is the exception: it stands up
//! a real mock JWKS/discovery server (so the inner `OidcAuthProvider` validates the token) paired
//! with an `unreachable_pool`-style lazy `DbGroupsSource` (never actually reachable), to verify
//! that a `ProviderUnavailable` from the group store -- not just a rejected credential -- maps to
//! a 503, no live DB needed.

use analytics_web_srv::auth::{AuthState, OidcClientConfig, auth_logout};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::{get, post},
};
use base64::Engine;
use http::header::COOKIE;
use micromegas::auth::groups::DbGroupsSource;
use rsa::pkcs1::{DecodeRsaPublicKey, EncodeRsaPrivateKey, EncodeRsaPublicKey};
use rsa::traits::PublicKeyParts;
use serde::Serialize;
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A pool that is never actually reachable -- mirrors `rust/auth/tests/test_utils.rs`'s
/// `unreachable_pool`, reimplemented here since that helper is private to the `micromegas-auth`
/// crate's own test binaries.
fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn create_test_auth_state() -> AuthState {
    // Use a fixed secret for testing
    let state_signing_secret = b"test-secret-key-32-bytes-long!!!".to_vec();

    AuthState {
        oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
        auth_provider: Arc::new(tokio::sync::OnceCell::new()),
        config: OidcClientConfig {
            issuer: "https://issuer.example.com".to_string(),
            client_id: "test-client".to_string(),
            redirect_uri: "http://localhost:3000/auth/callback".to_string(),
        },
        cookie_domain: None,
        secure_cookies: false,
        state_signing_secret,
        base_path: String::new(),
        groups: Arc::new(DbGroupsSource::new(
            unreachable_pool(),
            Duration::from_secs(60),
        )),
    }
}

#[tokio::test]
async fn test_auth_logout_clears_cookies() {
    let state = create_test_auth_state();
    let app = Router::new()
        .route("/auth/logout", post(auth_logout))
        .with_state(state);

    let request = Request::builder()
        .method("POST")
        .uri("/auth/logout")
        .header(COOKIE, "id_token=some_token; refresh_token=some_refresh")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    // Check that cookies are being cleared via Set-Cookie headers
    let set_cookies: Vec<_> = response.headers().get_all("set-cookie").iter().collect();

    // Should have set-cookie headers for clearing both id_token and refresh_token
    assert!(set_cookies.len() >= 2);

    // Verify cookies are being cleared (max-age=0)
    let id_token_cleared = set_cookies.iter().any(|h| {
        let s = h.to_str().unwrap_or("");
        s.contains("id_token=") && s.contains("Max-Age=0")
    });
    let refresh_token_cleared = set_cookies.iter().any(|h| {
        let s = h.to_str().unwrap_or("");
        s.contains("refresh_token=") && s.contains("Max-Age=0")
    });

    assert!(
        id_token_cleared,
        "id_token should be cleared with Max-Age=0"
    );
    assert!(
        refresh_token_cleared,
        "refresh_token should be cleared with Max-Age=0"
    );
}

#[tokio::test]
async fn test_cookie_with_httponly_and_samesite_lax() {
    let state = create_test_auth_state();
    let app = Router::new()
        .route("/auth/logout", post(auth_logout))
        .with_state(state);

    let request = Request::builder()
        .method("POST")
        .uri("/auth/logout")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    let set_cookies: Vec<_> = response.headers().get_all("set-cookie").iter().collect();

    // Check that cookies have HttpOnly and SameSite=Lax
    for cookie_header in set_cookies {
        let s = cookie_header.to_str().unwrap_or("");
        assert!(
            s.contains("HttpOnly"),
            "Cookie should have HttpOnly flag: {s}"
        );
        assert!(
            s.contains("SameSite=Lax"),
            "Cookie should have SameSite=Lax: {s}"
        );
        assert!(s.contains("Path=/"), "Cookie should have Path=/: {s}");
    }
}

const OIDC_CONFIG_VAR: &str = "MICROMEGAS_OIDC_CONFIG";

/// Clears the env var this test mutates on drop, so a failing assertion can't leak state into
/// another test. Paired with `#[serial]`, since the var is process-wide.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests touching `OIDC_CONFIG_VAR` are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(OIDC_CONFIG_VAR);
        }
    }
}

/// Claims shape matching `micromegas_auth::oidc::Claims`'s wire format (that struct is private
/// to its crate, so this test signs its own JWT with an equivalent shape rather than importing
/// it).
#[derive(Serialize)]
struct TestClaims {
    sub: String,
    iss: String,
    aud: String,
    exp: i64,
    email: Option<String>,
}

/// Starts a mock OIDC issuer serving discovery + a JWKS containing `public_key`, and signs a
/// token with `private_key`.
async fn start_mock_issuer_and_sign_token() -> (MockServer, String, String) {
    let mut rng = rsa::rand_core::OsRng;
    let private_key =
        rsa::RsaPrivateKey::new(&mut rng, 2048).expect("generating test RSA private key");
    let public_key = private_key.to_public_key();
    let private_pem = private_key
        .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
        .expect("encoding private key as PEM");
    let public_pem = public_key
        .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
        .expect("encoding public key as PEM");
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_pem.as_bytes())
        .expect("building jsonwebtoken encoding key");

    let server = MockServer::start().await;
    let issuer = server.uri();

    let discovery = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
        .mount(&server)
        .await;

    let public_key_parsed =
        rsa::RsaPublicKey::from_pkcs1_pem(&public_pem).expect("parsing test public key");
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(public_key_parsed.n().to_bytes_be());
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(public_key_parsed.e().to_bytes_be());
    let jwks = serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "n": n,
            "e": e,
        }]
    });
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(&server)
        .await;

    let claims = TestClaims {
        sub: "user123".to_string(),
        iss: issuer.clone(),
        aud: "test-client".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp(),
        email: Some("user123@example.com".to_string()),
    };
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &encoding_key,
    )
    .expect("signing test token");

    (server, issuer, token)
}

/// A handler this test's group-store-outage path never actually reaches -- the middleware
/// rejects the request with 503 before `next.run` is ever called.
async fn unreachable_handler() -> StatusCode {
    StatusCode::OK
}

/// A `ProviderUnavailable` from the group store (not a rejected credential) must map to 503, the
/// same distinction `axum.rs::auth_middleware`/`tower.rs::AuthService` make for their own DB-backed
/// providers -- so a client retries instead of treating the session as permanently invalid. The
/// token itself validates fine (real mock JWKS, real signature verification); only the group
/// store -- an `unreachable_pool`-style lazy pool that never actually connects -- is unavailable.
#[tokio::test]
#[serial]
async fn cookie_auth_middleware_maps_group_store_outage_to_503() {
    let _guard = EnvGuard;

    let (_server, issuer, token) = start_mock_issuer_and_sign_token().await;

    let oidc_config = serde_json::json!({
        "issuers": [{ "issuer": issuer, "audience": "test-client" }]
    });
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(OIDC_CONFIG_VAR, oidc_config.to_string());
    }

    let state = create_test_auth_state();
    let app = Router::new()
        .route("/protected", get(unreachable_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            analytics_web_srv::auth::cookie_auth_middleware,
        ));

    let request = Request::builder()
        .method("GET")
        .uri("/protected")
        .header(COOKIE, format!("id_token={token}"))
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a group-store outage must map to 503, not 401"
    );
}

// Note: The following tests are commented out because they require either:
// 1. A mock OIDC server with proper JWKS endpoint
// 2. The MICROMEGAS_OIDC_CONFIG environment variable set
//
// These tests validated the OLD behavior (basic JWT validation without signature check).
// With Phase 1 security improvements, all tokens are now validated with full signature
// verification using JWKS from the OIDC provider.
//
// To test signature validation:
// - Set up a mock OIDC server (e.g., using wiremock or similar)
// - Configure MICROMEGAS_OIDC_CONFIG with the mock server's issuer URL
// - Create tokens signed with the mock server's private key
//
// For now, manual testing with real OIDC providers (Auth0, Azure AD, Google) is
// recommended to verify the signature validation works correctly.
//
// TODO: Add mock OIDC server tests in Phase 3 (Audit & Observability) or as a
// separate test infrastructure improvement.
//
// Previous tests that are now obsolete:
// - test_auth_me_returns_user_info_with_valid_token
// - test_auth_me_returns_401_without_token
// - test_auth_me_returns_401_with_expired_token
// - test_auth_me_returns_401_with_invalid_jwt_format
// - test_auth_me_returns_401_with_invalid_base64_payload
// - test_auth_me_falls_back_to_preferred_username
// - test_cookie_auth_middleware_allows_valid_token
// - test_cookie_auth_middleware_rejects_missing_token
// - test_cookie_auth_middleware_rejects_expired_token
// - test_cookie_auth_middleware_rejects_invalid_jwt
// - test_cookie_auth_middleware_rejects_malformed_payload
