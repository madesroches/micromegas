use micromegas_auth::api_key::{ApiKeyAuthProvider, Key, KeyRing, KeyRingValue, parse_key_ring};
use micromegas_auth::ip_allowlist::IpAllowlist;
use micromegas_auth::types::{AuthProvider, AuthType, HttpRequestParts, RequestParts};

fn unrestricted(name: &str) -> KeyRingValue {
    KeyRingValue {
        name: name.to_string(),
        allowlist: IpAllowlist::parse(&[]).expect("empty allowlist parses"),
    }
}

fn restricted(name: &str, allowed_cidrs: &[&str]) -> KeyRingValue {
    KeyRingValue {
        name: name.to_string(),
        allowlist: IpAllowlist::parse(
            &allowed_cidrs
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .expect("allowlist parses"),
    }
}

fn parts_with_client_ip(token: &str, client_ip: Option<std::net::IpAddr>) -> HttpRequestParts {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("valid header"),
    );
    HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().expect("valid uri"),
        client_ip,
    }
}

#[tokio::test]
async fn test_valid_api_key() {
    let mut keyring = KeyRing::new();
    keyring.insert(
        Key::new("test-key-123".to_string()),
        unrestricted("test-user"),
    );

    let provider = ApiKeyAuthProvider::new(keyring);

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer test-key-123".parse().unwrap(),
    );
    let parts = HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().unwrap(),
        client_ip: None,
    };

    let result = provider.validate_request(&parts as &dyn RequestParts).await;

    assert!(result.is_ok());
    let ctx = result.unwrap();
    assert_eq!(ctx.subject, "test-user");
    assert_eq!(ctx.issuer, "api_key");
    assert_eq!(ctx.auth_type, AuthType::ApiKey);
    assert_eq!(ctx.email, None);
    assert_eq!(ctx.expires_at, None);
}

#[tokio::test]
async fn test_invalid_api_key() {
    let keyring = KeyRing::new();
    let provider = ApiKeyAuthProvider::new(keyring);

    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        "Bearer invalid-key".parse().unwrap(),
    );
    let parts = HttpRequestParts {
        headers,
        method: http::Method::GET,
        uri: "/test".parse().unwrap(),
        client_ip: None,
    };

    let result = provider.validate_request(&parts as &dyn RequestParts).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().to_string(), "invalid API token");
}

#[test]
fn test_parse_key_ring() {
    let json = r#"[
        {"name": "user1", "key": "key1"},
        {"name": "user2", "key": "key2"}
    ]"#;

    let keyring = parse_key_ring(json).expect("Failed to parse keyring");
    assert_eq!(keyring.len(), 2);
    assert_eq!(
        keyring.get(&Key::new("key1".to_string())).map(|v| &v.name),
        Some(&"user1".to_string())
    );
    assert_eq!(
        keyring.get(&Key::new("key2".to_string())).map(|v| &v.name),
        Some(&"user2".to_string())
    );
}

/// A missing `allowed_cidrs` field parses to an unrestricted (empty) allowlist -- the
/// backward-compatible default for every keyring entry written before this feature existed.
#[test]
fn test_parse_key_ring_defaults_allowed_cidrs_to_unrestricted() {
    let json = r#"[{"name": "user1", "key": "key1"}]"#;
    let keyring = parse_key_ring(json).expect("Failed to parse keyring");
    let value = keyring
        .get(&Key::new("key1".to_string()))
        .expect("key present");
    assert!(value.allowlist.allows(None));
}

/// A malformed `allowed_cidrs` entry fails the whole parse (fail-fast at startup), same as any
/// other keyring-shape error.
#[test]
fn test_parse_key_ring_rejects_malformed_allowed_cidrs() {
    let json = r#"[{"name": "user1", "key": "key1", "allowed_cidrs": ["not-a-cidr"]}]"#;
    assert!(parse_key_ring(json).is_err());
}

/// A restrictive `allowed_cidrs` accepts a request whose `client_ip` is inside it.
#[tokio::test]
async fn restricted_key_accepts_request_from_an_in_range_ip() {
    let mut keyring = KeyRing::new();
    keyring.insert(
        Key::new("test-key-123".to_string()),
        restricted("test-user", &["203.0.113.0/24"]),
    );
    let provider = ApiKeyAuthProvider::new(keyring);

    let parts = parts_with_client_ip("test-key-123", Some("203.0.113.42".parse().unwrap()));
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_ok());
}

/// A restrictive `allowed_cidrs` rejects a request whose `client_ip` is outside it.
#[tokio::test]
async fn restricted_key_rejects_request_from_an_out_of_range_ip() {
    let mut keyring = KeyRing::new();
    keyring.insert(
        Key::new("test-key-123".to_string()),
        restricted("test-user", &["203.0.113.0/24"]),
    );
    let provider = ApiKeyAuthProvider::new(keyring);

    let parts = parts_with_client_ip("test-key-123", Some("198.51.100.7".parse().unwrap()));
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_err());
}

/// A restrictive `allowed_cidrs` rejects a request with no resolvable client IP -- "unknown"
/// must never satisfy a restriction.
#[tokio::test]
async fn restricted_key_rejects_request_with_no_resolvable_client_ip() {
    let mut keyring = KeyRing::new();
    keyring.insert(
        Key::new("test-key-123".to_string()),
        restricted("test-user", &["203.0.113.0/24"]),
    );
    let provider = ApiKeyAuthProvider::new(keyring);

    let parts = parts_with_client_ip("test-key-123", None);
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_err());
}

/// An entry with no `allowed_cidrs` restriction is unaffected by `client_ip` -- the regression
/// check that `test_valid_api_key`/`test_invalid_api_key` keep passing unchanged.
#[tokio::test]
async fn unrestricted_key_accepts_request_regardless_of_client_ip() {
    let mut keyring = KeyRing::new();
    keyring.insert(
        Key::new("test-key-123".to_string()),
        unrestricted("test-user"),
    );
    let provider = ApiKeyAuthProvider::new(keyring);

    let parts = parts_with_client_ip("test-key-123", None);
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_ok());

    let parts = parts_with_client_ip("test-key-123", Some("198.51.100.7".parse().unwrap()));
    let result = provider.validate_request(&parts as &dyn RequestParts).await;
    assert!(result.is_ok());
}
