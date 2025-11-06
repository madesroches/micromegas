use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::multi::MultiAuthProvider;
use micromegas_auth::types::{AuthProvider, HttpRequestParts, RequestParts};
use std::sync::Arc;

#[tokio::test]
async fn test_multi_provider_api_key() {
    let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
    let api_key_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

    let multi = MultiAuthProvider::new().with_provider(api_key_provider);

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer secret".parse().unwrap(),
    );
    let parts = HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().unwrap(),
    };

    let result = multi.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_ok());
    let auth_ctx = result.unwrap();
    assert_eq!(auth_ctx.subject, "test");
}

#[tokio::test]
async fn test_multi_provider_no_providers() {
    let multi = MultiAuthProvider::new();

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer any-token".parse().unwrap(),
    );
    let parts = HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().unwrap(),
    };

    let result = multi.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_multi_provider_invalid_token() {
    let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
    let api_key_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

    let multi = MultiAuthProvider::new().with_provider(api_key_provider);

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer wrong-token".parse().unwrap(),
    );
    let parts = HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().unwrap(),
    };

    let result = multi.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_err());
}
