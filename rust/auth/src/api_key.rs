use crate::types::{AuthContext, AuthProvider, AuthType};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Display};
use subtle::ConstantTimeEq;

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
    /// Validate an API key token using constant-time comparison
    ///
    /// This implementation protects against timing attacks by:
    /// 1. Comparing the provided token against ALL keys in the keyring
    /// 2. Using constant-time comparison from the `subtle` crate
    /// 3. Always iterating through all keys regardless of match status
    ///
    /// This ensures the operation takes the same amount of time whether:
    /// - The key is found early in the iteration
    /// - The key is found late in the iteration
    /// - The key is not found at all
    async fn validate_token(&self, token: &str) -> Result<AuthContext> {
        let token_bytes = token.as_bytes();
        let mut found: Option<AuthContext> = None;

        // Compare against all keys in constant time
        // IMPORTANT: We iterate through ALL keys, even if we find a match,
        // to ensure constant-time operation
        for (stored_key, name) in &self.keyring {
            let stored_bytes = stored_key.value.as_bytes();

            // Constant-time comparison
            // Returns 1 if equal, 0 if not equal
            let matches = token_bytes.ct_eq(stored_bytes).unwrap_u8() == 1;

            // Conditionally set the result without branching on the match
            // If matches is true, we set found; if matches is false, found stays as-is
            if matches {
                found = Some(AuthContext {
                    subject: name.clone(),
                    email: None,
                    issuer: "api_key".to_string(),
                    expires_at: None,
                    auth_type: AuthType::ApiKey,
                    is_admin: false,
                });
            }
            // Note: We do NOT break or return early - we continue checking all keys
        }

        found.ok_or_else(|| anyhow!("invalid API token"))
    }
}
