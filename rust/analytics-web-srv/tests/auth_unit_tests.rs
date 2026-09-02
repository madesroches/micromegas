//! Unit tests for auth module helper functions

use analytics_web_srv::auth::{
    AuthApiError, AuthState, OidcClientConfig, clear_cookie, create_cookie,
    extract_audit_claims_from_token,
};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::SameSite;
use base64::Engine;
use http::StatusCode;
use micromegas::auth::groups::DbGroupsSource;
use micromegas::auth::oauth_state::generate_nonce;
use std::sync::Arc;
use std::time::Duration;

/// A pool that is never actually reachable, matching `rust/auth/tests/test_utils.rs`'s
/// `unreachable_pool` -- fine here, since none of these tests call `get_auth_provider`.
fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn create_test_auth_state() -> AuthState {
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
        state_signing_secret: b"test-secret-32-bytes-for-testing".to_vec(),
        base_path: String::new(),
        groups: Arc::new(DbGroupsSource::new(
            unreachable_pool(),
            Duration::from_secs(60),
        )),
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

// The next four tests are `#[tokio::test]`, not plain `#[test]`: `create_test_auth_state`
// builds a `DbGroupsSource` over a `connect_lazy` pool, and `connect_lazy` needs a Tokio runtime
// context to construct even though it never actually connects.

#[tokio::test]
async fn test_create_cookie_basic_properties() {
    let state = create_test_auth_state();

    let cookie = create_cookie("test_cookie", "test_value".to_string(), 3600, &state);
    assert_eq!(cookie.name(), "test_cookie");
    assert_eq!(cookie.value(), "test_value");
    assert!(cookie.http_only().unwrap_or(false));
    assert_eq!(cookie.path().unwrap_or(""), "/");
    assert_eq!(cookie.same_site(), Some(SameSite::Lax));
}

#[tokio::test]
async fn test_create_cookie_secure_flag() {
    let mut state = create_test_auth_state();
    state.secure_cookies = true;

    let cookie = create_cookie("secure_cookie", "value".to_string(), 3600, &state);
    assert!(cookie.secure().unwrap_or(false));
}

#[tokio::test]
async fn test_create_cookie_with_domain() {
    let mut state = create_test_auth_state();
    state.cookie_domain = Some(".example.com".to_string());

    let cookie = create_cookie("domain_cookie", "value".to_string(), 3600, &state);
    // Cookie library strips leading dot from domain
    assert_eq!(cookie.domain(), Some("example.com"));
}

#[tokio::test]
async fn test_clear_cookie_expires_immediately() {
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

// ---------------------------------------------------------------------------
// extract_audit_claims_from_token
// ---------------------------------------------------------------------------

/// Builds an unsigned JWT (`base64url(header).base64url(payload).base64url(sig)`)
/// carrying the given JSON payload -- `extract_audit_claims_from_token` never
/// verifies the signature, so any placeholder bytes work for the third segment.
fn unsigned_jwt(payload_json: &str) -> String {
    let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes());
    format!(
        "{}.{}.{}",
        b64(r#"{"alg":"none","typ":"JWT"}"#),
        b64(payload_json),
        b64("sig")
    )
}

#[test]
fn extract_audit_claims_from_token_reads_sub_and_email() {
    let token = unsigned_jwt(r#"{"sub": "user-123", "email": "alice@example.com"}"#);

    let claims = extract_audit_claims_from_token(&token);

    assert_eq!(claims.sub, Some("user-123".to_string()));
    assert_eq!(claims.email, Some("alice@example.com".to_string()));
}

#[test]
fn extract_audit_claims_from_token_missing_email_yields_none() {
    let token = unsigned_jwt(r#"{"sub": "user-123"}"#);

    let claims = extract_audit_claims_from_token(&token);

    assert_eq!(claims.sub, Some("user-123".to_string()));
    assert_eq!(claims.email, None);
}

#[test]
fn extract_audit_claims_from_token_malformed_shape_yields_both_none() {
    // Not 3 dot-separated parts.
    let claims = extract_audit_claims_from_token("not-a-jwt");
    assert_eq!(claims.sub, None);
    assert_eq!(claims.email, None);
}

#[test]
fn extract_audit_claims_from_token_non_base64_payload_yields_both_none() {
    let claims = extract_audit_claims_from_token("header.not!base64url.sig");
    assert_eq!(claims.sub, None);
    assert_eq!(claims.email, None);
}

#[test]
fn extract_audit_claims_from_token_non_json_payload_yields_both_none() {
    let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes());
    let token = format!("{}.{}.{}", b64("header"), b64("not json"), b64("sig"));

    let claims = extract_audit_claims_from_token(&token);

    assert_eq!(claims.sub, None);
    assert_eq!(claims.email, None);
}
