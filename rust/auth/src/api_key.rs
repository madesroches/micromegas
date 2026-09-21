use crate::ip_allowlist::IpAllowlist;
use crate::types::{AuthContext, AuthProvider, AuthType};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Display};
use subtle::ConstantTimeEq;

/// Represents a key in the keyring.
#[derive(Hash, Eq, PartialEq)]
pub struct Key {
    /// The key value
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
    /// The name associated with the key
    pub name: String,
    /// The key
    #[serde(deserialize_with = "key_from_string")]
    pub key: Key,
    /// CIDR ranges or bare IPs this key may be used from. Absent/empty = unrestricted.
    #[serde(default)]
    pub allowed_cidrs: Vec<String>,
}

/// A keyring entry's name plus its parsed IP allowlist -- the parsed allowlist travels with the
/// name so [`ApiKeyAuthProvider::validate_request`] never re-parses CIDR strings on the hot path.
pub struct KeyRingValue {
    /// The name associated with the key.
    pub name: String,
    /// The parsed IP allowlist -- empty means unrestricted.
    pub allowlist: IpAllowlist,
}

/// A map from `Key` to [`KeyRingValue`] (name + parsed allowlist).
pub type KeyRing = HashMap<Key, KeyRingValue>;

/// Parses a JSON string into a `KeyRing`.
///
/// The JSON string is expected to be an array of objects, each with a `name` and `key` field,
/// and an optional `allowed_cidrs` array. Fails the whole parse (fail-fast at startup, same as
/// every other keyring-shape error) on a malformed `allowed_cidrs` entry.
pub fn parse_key_ring(json: &str) -> Result<KeyRing> {
    let entries: Vec<KeyRingEntry> = serde_json::from_str(json)?;
    let mut ring = KeyRing::new();
    for entry in entries {
        let allowlist = IpAllowlist::parse(&entry.allowed_cidrs)?;
        ring.insert(
            entry.key,
            KeyRingValue {
                name: entry.name,
                allowlist,
            },
        );
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
    /// Validate an API key request using constant-time comparison
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
    async fn validate_request(
        &self,
        parts: &dyn crate::types::RequestParts,
    ) -> Result<AuthContext> {
        let token = parts
            .bearer_token()
            .ok_or_else(|| anyhow!("missing bearer token"))?;

        let token_bytes = token.as_bytes();
        let mut found: Option<(AuthContext, &IpAllowlist)> = None;

        // Compare against all keys in constant time
        // IMPORTANT: We iterate through ALL keys, even if we find a match,
        // to ensure constant-time operation
        for (stored_key, value) in &self.keyring {
            let stored_bytes = stored_key.value.as_bytes();

            // Constant-time comparison
            // Returns 1 if equal, 0 if not equal
            let matches = token_bytes.ct_eq(stored_bytes).unwrap_u8() == 1;

            // Conditionally set the result without branching on the match
            // If matches is true, we set found; if matches is false, found stays as-is
            if matches {
                found = Some((
                    AuthContext {
                        subject: value.name.clone(),
                        email: None,
                        issuer: "api_key".to_string(),
                        audience: None,
                        expires_at: None,
                        auth_type: AuthType::ApiKey,
                        // SECURITY: API keys CAN delegate (act on behalf of users)
                        allow_delegation: true,
                        // Env-configured keys carry no Stage 4/4b grant.
                        bound_audience: None,
                        read_audiences: vec![],
                        // API keys carry no email for a `user:` member to match, so only a
                        // `MembershipProvider` wrapping this provider over a wildcard-admin group
                        // could ever make one admin -- see the migration v10 module doc comment.
                        memberships: std::sync::Arc::from([]),
                    },
                    &value.allowlist,
                ));
            }
            // Note: We do NOT break or return early - we continue checking all keys
        }

        // The allowlist check runs once, after the constant-time scan, on `found` only -- it
        // checks public data (the request's own resolved IP), so there is no timing side-channel
        // to protect for it specifically, and it leaves the loop's constant-time property over
        // the *key comparison* untouched.
        let (context, allowlist) = found.ok_or_else(|| anyhow!("invalid API token"))?;
        if !allowlist.allows(parts.client_ip()) {
            micromegas_tracing::warn!(
                "env api key rejected by allowlist: name={} client_ip={:?}",
                context.subject,
                parts.client_ip()
            );
            anyhow::bail!("invalid API token: source IP not permitted");
        }
        Ok(context)
    }
}
