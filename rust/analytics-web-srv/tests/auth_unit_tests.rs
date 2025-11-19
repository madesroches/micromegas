//! Unit tests for auth module helper functions

use analytics_web_srv::auth::{
    AuthApiError, AuthState, OidcClientConfig, clear_cookie, create_cookie, generate_nonce,
};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::SameSite;
use base64::Engine;
use http::StatusCode;
use std::sync::Arc;

fn create_test_auth_state() -> AuthState {
    AuthState {
        oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
        config: OidcClientConfig {
            issuer: "https://issuer.example.com".to_string(),
            client_id: "test-client".to_string(),
            redirect_uri: "http://localhost:3000/auth/callback".to_string(),
        },
        cookie_domain: None,
        secure_cookies: false,
        state_signing_secret: b"test-secret-32-bytes-for-testing".to_vec(),
    }
}

#[test]
fn test_generate_nonce_uniqueness() {
    let nonce1 = generate_nonce();
    let nonce2 = generate_nonce();
    assert_ne!(nonce1, nonce2);
}

#[test]
fn test_generate_nonce_length() {
    let nonce = generate_nonce();
    // 32 bytes base64 encoded should be 43 characters (URL_SAFE_NO_PAD)
    assert_eq!(nonce.len(), 43);
}

#[test]
fn test_generate_nonce_valid_base64() {
    let nonce = generate_nonce();
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&nonce);
    assert!(decoded.is_ok());
    assert_eq!(decoded.expect("should decode").len(), 32);
}

#[test]
fn test_create_cookie_basic_properties() {
    let state = create_test_auth_state();

    let cookie = create_cookie("test_cookie", "test_value".to_string(), 3600, &state);
    assert_eq!(cookie.name(), "test_cookie");
    assert_eq!(cookie.value(), "test_value");
    assert!(cookie.http_only().unwrap_or(false));
    assert_eq!(cookie.path().unwrap_or(""), "/");
    assert_eq!(cookie.same_site(), Some(SameSite::Lax));
}

#[test]
fn test_create_cookie_secure_flag() {
    let mut state = create_test_auth_state();
    state.secure_cookies = true;

    let cookie = create_cookie("secure_cookie", "value".to_string(), 3600, &state);
    assert!(cookie.secure().unwrap_or(false));
}

#[test]
fn test_create_cookie_with_domain() {
    let mut state = create_test_auth_state();
    state.cookie_domain = Some(".example.com".to_string());

    let cookie = create_cookie("domain_cookie", "value".to_string(), 3600, &state);
    // Cookie library strips leading dot from domain
    assert_eq!(cookie.domain(), Some("example.com"));
}

#[test]
fn test_clear_cookie_expires_immediately() {
    let state = create_test_auth_state();

    let cookie = clear_cookie("expired_cookie", &state);
    assert_eq!(cookie.name(), "expired_cookie");
    assert_eq!(cookie.value(), "");
    assert_eq!(cookie.max_age(), Some(time::Duration::seconds(0)));
}

#[test]
fn test_auth_api_error_status_codes() {
    let invalid_url_resp = AuthApiError::InvalidReturnUrl.into_response();
    assert_eq!(invalid_url_resp.status(), StatusCode::BAD_REQUEST);

    let invalid_state_resp = AuthApiError::InvalidState.into_response();
    assert_eq!(invalid_state_resp.status(), StatusCode::BAD_REQUEST);

    let token_failed_resp = AuthApiError::TokenExchangeFailed.into_response();
    assert_eq!(token_failed_resp.status(), StatusCode::UNAUTHORIZED);

    let unauthorized_resp = AuthApiError::Unauthorized.into_response();
    assert_eq!(unauthorized_resp.status(), StatusCode::UNAUTHORIZED);

    let invalid_token_resp = AuthApiError::InvalidToken.into_response();
    assert_eq!(invalid_token_resp.status(), StatusCode::UNAUTHORIZED);

    let internal_resp = AuthApiError::Internal("test error".to_string()).into_response();
    assert_eq!(internal_resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
