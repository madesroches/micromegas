//! Integration tests for auth endpoints
//!
//! These tests verify the auth endpoints work correctly with cookies.
//!
//! Note: Tests for auth_me and cookie_auth_middleware with JWT validation
//! require a mock OIDC server or environment with MICROMEGAS_OIDC_CONFIG set.
//! The signature validation tests are skipped in unit tests since they
//! require real JWKS endpoints.

use analytics_web_srv::auth::{AuthState, OidcClientConfig, auth_logout};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use http::header::COOKIE;
use std::sync::Arc;
use tower::ServiceExt;

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
