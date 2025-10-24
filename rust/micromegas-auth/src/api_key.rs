use crate::types::{AuthContext, AuthProvider, AuthType};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Display};

/// Represents a key in the keyring.
#[derive(Hash, Eq, PartialEq)]
pub struct Key {
    pub value: String,
}

impl Key {
    /// Creates a new `Key` from a string value.
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<String> for Key {
    fn from(value: String) -> Self {
        Self { value }
    }
}

impl Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<sensitive key>")
    }
}

/// Deserializes a string into a `Key`.
fn key_from_string<'de, D>(deserializer: D) -> Result<Key, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Ok(Key::new(s))
}

/// Represents an entry in the keyring, mapping a key to a name.
#[derive(Deserialize)]
pub struct KeyRingEntry {
    pub name: String,
    #[serde(deserialize_with = "key_from_string")]
    pub key: Key,
}

/// A map from `Key` to `String` (name).
pub type KeyRing = HashMap<Key, String>;

/// Parses a JSON string into a `KeyRing`.
///
/// The JSON string is expected to be an array of objects, each with a `name` and `key` field.
pub fn parse_key_ring(json: &str) -> Result<KeyRing> {
    let entries: Vec<KeyRingEntry> = serde_json::from_str(json)?;
    let mut ring = KeyRing::new();
    for entry in entries {
        ring.insert(entry.key, entry.name);
    }
    Ok(ring)
}

/// API key authentication provider
pub struct ApiKeyAuthProvider {
    keyring: KeyRing,
}

impl ApiKeyAuthProvider {
    /// Create a new API key authentication provider
    pub fn new(keyring: KeyRing) -> Self {
        Self { keyring }
    }
}

#[async_trait::async_trait]
impl AuthProvider for ApiKeyAuthProvider {
    async fn validate_token(&self, token: &str) -> Result<AuthContext> {
        let key: Key = token.to_string().into();

        if let Some(name) = self.keyring.get(&key) {
            Ok(AuthContext {
                subject: name.clone(),
                email: None,
                issuer: "api_key".to_string(),
                expires_at: None,
                auth_type: AuthType::ApiKey,
                is_admin: false,
            })
        } else {
            Err(anyhow!("invalid API token"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_valid_api_key() {
        let mut keyring = KeyRing::new();
        keyring.insert(Key::new("test-key-123".to_string()), "test-user".to_string());

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
        assert_eq!(keyring.get(&Key::new("key1".to_string())), Some(&"user1".to_string()));
        assert_eq!(keyring.get(&Key::new("key2".to_string())), Some(&"user2".to_string()));
    }
}
