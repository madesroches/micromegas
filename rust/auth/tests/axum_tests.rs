use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{StatusCode, header::AUTHORIZATION},
};
use micromegas_auth::{
    api_key::{ApiKeyAuthProvider, parse_key_ring},
    axum::auth_middleware,
    types::{AuthContext, AuthProvider, ProviderUnavailable, RequestParts},
};
use std::sync::Arc;
use tower::ServiceExt;

/// A provider whose store is unreachable — every call fails with
/// `ProviderUnavailable`, never a plain rejection.
struct AlwaysUnavailableProvider;

#[async_trait::async_trait]
impl AuthProvider for AlwaysUnavailableProvider {
    async fn validate_request(&self, _parts: &dyn RequestParts) -> anyhow::Result<AuthContext> {
        Err(ProviderUnavailable(anyhow::anyhow!("key store unreachable")).into())
    }
}

#[tokio::test]
async fn test_valid_api_key() {
    let json = r#"[{"name": "test-user", "key": "secret-key-123"}]"#;
    let keyring = parse_key_ring(json).expect("parse keyring");
    let provider: Arc<dyn AuthProvider> = Arc::new(ApiKeyAuthProvider::new(keyring));

    let app = Router::new()
        .route(
            "/test",
            axum::routing::get(|req: Request| async move {
                let auth_ctx = req.extensions().get::<AuthContext>().expect("auth context");
                assert_eq!(auth_ctx.subject, "test-user");
                assert_eq!(auth_ctx.issuer, "api_key");
                "ok"
            }),
        )
        .layer(axum::middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));

    let request = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Bearer secret-key-123")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_invalid_api_key() {
    let json = r#"[{"name": "test-user", "key": "secret-key-123"}]"#;
    let keyring = parse_key_ring(json).expect("parse keyring");
    let provider: Arc<dyn AuthProvider> = Arc::new(ApiKeyAuthProvider::new(keyring));

    let app = Router::new()
        .route("/test", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));

    let request = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Bearer wrong-key")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_missing_authorization_header() {
    let json = r#"[{"name": "test-user", "key": "secret-key-123"}]"#;
    let keyring = parse_key_ring(json).expect("parse keyring");
    let provider: Arc<dyn AuthProvider> = Arc::new(ApiKeyAuthProvider::new(keyring));

    let app = Router::new()
        .route("/test", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));

    let request = Request::builder()
        .uri("/test")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A key-store outage must yield 503, not 401: the client (e.g.
/// `rust/telemetry-sink/src/http_event_sink.rs`) treats `4xx` as permanent and
/// `5xx` as retryable.
#[tokio::test]
async fn test_provider_unavailable_yields_503() {
    let provider: Arc<dyn AuthProvider> = Arc::new(AlwaysUnavailableProvider);

    let app = Router::new()
        .route("/test", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));

    let request = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Bearer whatever")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// A restricted keyring entry, layered through the real `Router`/`auth_middleware` stack --
/// accepts a request whose resolved IP (via `X-Forwarded-For`, no `ConnectInfo` needed) is in
/// range, and rejects one from an out-of-range IP and one with no resolvable IP at all.
#[tokio::test]
async fn test_restricted_api_key_accepts_in_range_and_rejects_out_of_range_ip() {
    let json =
        r#"[{"name": "test-user", "key": "secret-key-123", "allowed_cidrs": ["203.0.113.0/24"]}]"#;
    let keyring = parse_key_ring(json).expect("parse keyring");
    let provider: Arc<dyn AuthProvider> = Arc::new(ApiKeyAuthProvider::new(keyring));

    let app = Router::new()
        .route("/test", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));

    let in_range = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Bearer secret-key-123")
        .header("x-forwarded-for", "203.0.113.42")
        .body(Body::empty())
        .expect("build request");
    let response = app.clone().oneshot(in_range).await.expect("call service");
    assert_eq!(response.status(), StatusCode::OK);

    let out_of_range = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Bearer secret-key-123")
        .header("x-forwarded-for", "198.51.100.7")
        .body(Body::empty())
        .expect("build request");
    let response = app
        .clone()
        .oneshot(out_of_range)
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // No `X-Forwarded-For`/`X-Real-IP` and no `ConnectInfo` extension (this router never enables
    // it) -- the client IP never resolves, and an unresolved IP must never satisfy a restriction.
    let no_resolvable_ip = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Bearer secret-key-123")
        .body(Body::empty())
        .expect("build request");
    let response = app
        .clone()
        .oneshot(no_resolvable_ip)
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_invalid_authorization_format() {
    let json = r#"[{"name": "test-user", "key": "secret-key-123"}]"#;
    let keyring = parse_key_ring(json).expect("parse keyring");
    let provider: Arc<dyn AuthProvider> = Arc::new(ApiKeyAuthProvider::new(keyring));

    let app = Router::new()
        .route("/test", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }));

    let request = Request::builder()
        .uri("/test")
        .header(AUTHORIZATION, "Basic secret-key-123")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
