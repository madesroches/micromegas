use micromegas_auth::api_key::{ApiKeyAuthProvider, Key, KeyRing, parse_key_ring};
use micromegas_auth::types::{AuthProvider, AuthType};

#[tokio::test]
async fn test_valid_api_key() {
    let mut keyring = KeyRing::new();
    keyring.insert(
        Key::new("test-key-123".to_string()),
        "test-user".to_string(),
    );

    let provider = ApiKeyAuthProvider::new(keyring);
    let result = provider.validate_token("test-key-123").await;

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
    let result = provider.validate_token("invalid-key").await;

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
        keyring.get(&Key::new("key1".to_string())),
        Some(&"user1".to_string())
    );
    assert_eq!(
        keyring.get(&Key::new("key2".to_string())),
        Some(&"user2".to_string())
    );
}
