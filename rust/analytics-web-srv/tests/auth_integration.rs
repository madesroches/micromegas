//! Integration tests for auth endpoints
//!
//! These tests verify the auth endpoints work correctly with cookies and JWT tokens.
//! They do NOT test OIDC provider integration (auth_login, auth_callback) as those
//! require a mock OIDC server setup.

use analytics_web_srv::auth::{
    AuthState, OidcClientConfig, auth_logout, auth_me, cookie_auth_middleware,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::{get, post},
};
use chrono::{Duration, Utc};
use http::header::COOKIE;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs1::EncodeRsaPrivateKey;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower::ServiceExt;

/// Test JWT claims
#[derive(Debug, Serialize, Deserialize)]
struct TestClaims {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    preferred_username: Option<String>,
    exp: i64,
    iat: i64,
}

/// Test key pair for signing JWTs
struct TestKeyPair {
    encoding_key: EncodingKey,
}

impl TestKeyPair {
    fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let private_key =
            RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate RSA private key");

        let private_pem = private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("failed to encode private key as PEM");

        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes())
            .expect("failed to create encoding key");

        Self { encoding_key }
    }

    fn create_token(&self, claims: TestClaims) -> String {
        encode(&Header::new(Algorithm::RS256), &claims, &self.encoding_key)
            .expect("failed to encode token")
    }
}

fn create_valid_token(keypair: &TestKeyPair, subject: &str, email: Option<&str>) -> String {
    let now = Utc::now();
    let claims = TestClaims {
        sub: subject.to_string(),
        email: email.map(String::from),
        name: Some("Test User".to_string()),
        preferred_username: None,
        exp: (now + Duration::hours(1)).timestamp(),
        iat: now.timestamp(),
    };
    keypair.create_token(claims)
}

fn create_expired_token(keypair: &TestKeyPair, subject: &str) -> String {
    let now = Utc::now();
    let claims = TestClaims {
        sub: subject.to_string(),
        email: None,
        name: None,
        preferred_username: None,
        exp: (now - Duration::hours(1)).timestamp(), // Expired 1 hour ago
        iat: (now - Duration::hours(2)).timestamp(),
    };
    keypair.create_token(claims)
}

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
    }
}

#[tokio::test]
async fn test_auth_me_returns_user_info_with_valid_token() {
    let keypair = TestKeyPair::generate();
    let token = create_valid_token(&keypair, "user123", Some("test@example.com"));

    let app = Router::new().route("/auth/me", get(auth_me));

    let request = Request::builder()
        .uri("/auth/me")
        .header(COOKIE, format!("access_token={token}"))
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let user_info: serde_json::Value = serde_json::from_slice(&body).expect("body should be JSON");

    assert_eq!(user_info["sub"], "user123");
    assert_eq!(user_info["email"], "test@example.com");
    assert_eq!(user_info["name"], "Test User");
}

#[tokio::test]
async fn test_auth_me_returns_401_without_token() {
    let app = Router::new().route("/auth/me", get(auth_me));

    let request = Request::builder()
        .uri("/auth/me")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_me_returns_401_with_expired_token() {
    let keypair = TestKeyPair::generate();
    let token = create_expired_token(&keypair, "user123");

    let app = Router::new().route("/auth/me", get(auth_me));

    let request = Request::builder()
        .uri("/auth/me")
        .header(COOKIE, format!("access_token={token}"))
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_me_returns_401_with_invalid_jwt_format() {
    let app = Router::new().route("/auth/me", get(auth_me));

    let request = Request::builder()
        .uri("/auth/me")
        .header(COOKIE, "access_token=not.a.valid.jwt")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_me_returns_401_with_invalid_base64_payload() {
    let app = Router::new().route("/auth/me", get(auth_me));

    let request = Request::builder()
        .uri("/auth/me")
        .header(COOKIE, "access_token=header.!!!invalid-base64!!!.signature")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_me_falls_back_to_preferred_username() {
    let keypair = TestKeyPair::generate();
    let now = Utc::now();
    let claims = TestClaims {
        sub: "user123".to_string(),
        email: None,
        name: None,
        preferred_username: Some("jdoe".to_string()),
        exp: (now + Duration::hours(1)).timestamp(),
        iat: now.timestamp(),
    };
    let token = keypair.create_token(claims);

    let app = Router::new().route("/auth/me", get(auth_me));

    let request = Request::builder()
        .uri("/auth/me")
        .header(COOKIE, format!("access_token={token}"))
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let user_info: serde_json::Value = serde_json::from_slice(&body).expect("body should be JSON");

    assert_eq!(user_info["sub"], "user123");
    // Should fall back to preferred_username when email is not present
    assert_eq!(user_info["email"], "jdoe");
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
        .header(
            COOKIE,
            "access_token=some_token; refresh_token=some_refresh",
        )
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    // Check that cookies are being cleared via Set-Cookie headers
    let set_cookies: Vec<_> = response.headers().get_all("set-cookie").iter().collect();

    // Should have set-cookie headers for clearing both access_token and refresh_token
    assert!(set_cookies.len() >= 2);

    // Verify cookies are being cleared (max-age=0)
    let access_token_cleared = set_cookies.iter().any(|h| {
        let s = h.to_str().unwrap_or("");
        s.contains("access_token=") && s.contains("Max-Age=0")
    });
    let refresh_token_cleared = set_cookies.iter().any(|h| {
        let s = h.to_str().unwrap_or("");
        s.contains("refresh_token=") && s.contains("Max-Age=0")
    });

    assert!(
        access_token_cleared,
        "access_token should be cleared with Max-Age=0"
    );
    assert!(
        refresh_token_cleared,
        "refresh_token should be cleared with Max-Age=0"
    );
}

#[tokio::test]
async fn test_cookie_auth_middleware_allows_valid_token() {
    let keypair = TestKeyPair::generate();
    let token = create_valid_token(&keypair, "user123", Some("test@example.com"));

    let app = Router::new()
        .route("/protected", get(|| async { "ok" }))
        .layer(middleware::from_fn(cookie_auth_middleware));

    let request = Request::builder()
        .uri("/protected")
        .header(COOKIE, format!("access_token={token}"))
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_cookie_auth_middleware_rejects_missing_token() {
    let app = Router::new()
        .route("/protected", get(|| async { "ok" }))
        .layer(middleware::from_fn(cookie_auth_middleware));

    let request = Request::builder()
        .uri("/protected")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_cookie_auth_middleware_rejects_expired_token() {
    let keypair = TestKeyPair::generate();
    let token = create_expired_token(&keypair, "user123");

    let app = Router::new()
        .route("/protected", get(|| async { "ok" }))
        .layer(middleware::from_fn(cookie_auth_middleware));

    let request = Request::builder()
        .uri("/protected")
        .header(COOKIE, format!("access_token={token}"))
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_cookie_auth_middleware_rejects_invalid_jwt() {
    let app = Router::new()
        .route("/protected", get(|| async { "ok" }))
        .layer(middleware::from_fn(cookie_auth_middleware));

    let request = Request::builder()
        .uri("/protected")
        .header(COOKIE, "access_token=invalid.jwt.token")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_cookie_auth_middleware_rejects_malformed_payload() {
    // Create a JWT-like token with 3 parts but invalid base64 payload
    let app = Router::new()
        .route("/protected", get(|| async { "ok" }))
        .layer(middleware::from_fn(cookie_auth_middleware));

    let request = Request::builder()
        .uri("/protected")
        .header(COOKIE, "access_token=header.@@@invalid@@@.signature")
        .body(Body::empty())
        .expect("request should build");

    let response = app.oneshot(request).await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
