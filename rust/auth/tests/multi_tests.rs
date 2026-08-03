use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::multi::MultiAuthProvider;
use micromegas_auth::types::{AuthProvider, HttpRequestParts, ProviderUnavailable, RequestParts};
use std::sync::Arc;

/// A provider whose store is unreachable — every call fails with
/// `ProviderUnavailable`.
struct UnavailableProvider;

#[async_trait::async_trait]
impl AuthProvider for UnavailableProvider {
    async fn validate_request(
        &self,
        _parts: &dyn RequestParts,
    ) -> anyhow::Result<micromegas_auth::types::AuthContext> {
        Err(ProviderUnavailable(anyhow::anyhow!("key store unreachable")).into())
    }
}

fn any_bearer_parts() -> HttpRequestParts {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer any-token".parse().unwrap(),
    );
    HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().unwrap(),
    }
}

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

/// One unavailable provider, no others configured: the error must downcast to
/// `ProviderUnavailable`, not the generic "all providers failed" message.
#[tokio::test]
async fn test_multi_provider_unavailable_alone() {
    let multi = MultiAuthProvider::new().with_provider(Arc::new(UnavailableProvider));

    let parts = any_bearer_parts();
    let result = multi.validate_request(&parts as &dyn RequestParts).await;
    let err = result.expect_err("expected an error");
    assert!(err.downcast_ref::<ProviderUnavailable>().is_some());
}

/// An outage must not be masked by an ordinary rejection elsewhere in the chain:
/// one unavailable provider plus one that plainly rejects still surfaces as
/// `ProviderUnavailable`.
#[tokio::test]
async fn test_multi_provider_unavailable_plus_rejection() {
    let keyring = parse_key_ring(r#"[{"name": "test", "key": "secret"}]"#).unwrap();
    let api_key_provider = Arc::new(ApiKeyAuthProvider::new(keyring));

    let multi = MultiAuthProvider::new()
        .with_provider(api_key_provider)
        .with_provider(Arc::new(UnavailableProvider));

    // Wrong key: the env provider plainly rejects; the DB-like provider is
    // unavailable.
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
    let err = result.expect_err("expected an error");
    assert!(err.downcast_ref::<ProviderUnavailable>().is_some());
}

/// No regression: when every provider plainly rejects (none unavailable), the
/// generic "authentication failed with all providers" error is unchanged.
#[tokio::test]
async fn test_multi_provider_all_reject_no_unavailable() {
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
    let err = result.expect_err("expected an error");
    assert!(err.downcast_ref::<ProviderUnavailable>().is_none());
    assert!(
        err.to_string()
            .contains("authentication failed with all providers")
    );
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
