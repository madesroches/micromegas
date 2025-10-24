use crate::types::{AuthContext, AuthProvider, AuthType};
use anyhow::{Result, anyhow};
use base64::Engine;
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, Validation, decode};
use moka::future::Cache;
use openidconnect::IssuerUrl;
use openidconnect::core::{CoreJsonWebKeySet, CoreProviderMetadata};
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::{BigUint, RsaPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Fetch JWKS from the OIDC provider using openidconnect's built-in discovery
async fn fetch_jwks(issuer_url: &IssuerUrl) -> Result<Arc<CoreJsonWebKeySet>> {
    // Create HTTP client with SSRF protection (no redirects)
    let http_client = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {e:?}"))?;

    // Use openidconnect's built-in OIDC discovery
    let metadata = CoreProviderMetadata::discover_async(issuer_url.clone(), &http_client)
        .await
        .map_err(|e| {
            anyhow!(
                "Failed to discover OIDC metadata from {}: {e:?}",
                issuer_url
            )
        })?;

    // Fetch JWKS from jwks_uri
    let jwks_uri = metadata.jwks_uri();
    let jwks: CoreJsonWebKeySet = http_client
        .get(jwks_uri.url().as_str())
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch JWKS from {}: {e:?}", jwks_uri))?
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse JWKS: {e:?}"))?;

    Ok(Arc::new(jwks))
}

/// JWKS cache for an OIDC issuer
///
/// Caches JSON Web Key Sets with automatic TTL expiration.
/// Uses moka for thread-safe caching with atomic cache miss handling.
struct JwksCache {
    issuer_url: IssuerUrl,
    cache: Cache<String, Arc<CoreJsonWebKeySet>>,
}

impl JwksCache {
    /// Create a new JWKS cache
    fn new(issuer_url: IssuerUrl, ttl: Duration) -> Self {
        let cache = Cache::builder().time_to_live(ttl).build();

        Self { issuer_url, cache }
    }

    /// Get the JWKS, fetching from the issuer if not cached
    async fn get(&self) -> Result<Arc<CoreJsonWebKeySet>> {
        let issuer_url = self.issuer_url.clone();

        self.cache
            .try_get_with(
                "jwks".to_string(),
                async move { fetch_jwks(&issuer_url).await },
            )
            .await
            .map_err(|e| anyhow!("Failed to fetch JWKS: {e:?}"))
    }
}

/// Configuration for a single OIDC issuer
#[derive(Debug, Clone, Deserialize)]
pub struct OidcIssuer {
    /// Issuer URL (e.g., "https://accounts.google.com")
    pub issuer: String,
    /// Expected audience (client ID)
    pub audience: String,
}

const DEFAULT_JWKS_REFRESH_INTERVAL_SECS: u64 = 3600;
const DEFAULT_TOKEN_CACHE_SIZE: u64 = 1000;
const DEFAULT_TOKEN_CACHE_TTL_SECS: u64 = 300;

/// OIDC configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OidcConfig {
    /// List of configured OIDC issuers
    pub issuers: Vec<OidcIssuer>,
    /// JWKS refresh interval in seconds (default: 3600 = 1 hour)
    pub jwks_refresh_interval_secs: u64,
    /// Token cache size (default: 1000)
    pub token_cache_size: u64,
    /// Token cache TTL in seconds (default: 300 = 5 min)
    pub token_cache_ttl_secs: u64,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            issuers: Vec::new(),
            jwks_refresh_interval_secs: DEFAULT_JWKS_REFRESH_INTERVAL_SECS,
            token_cache_size: DEFAULT_TOKEN_CACHE_SIZE,
            token_cache_ttl_secs: DEFAULT_TOKEN_CACHE_TTL_SECS,
        }
    }
}

impl OidcConfig {
    /// Load OIDC configuration from environment variable
    pub fn from_env() -> Result<Self> {
        let json = std::env::var("MICROMEGAS_OIDC_CONFIG")
            .map_err(|_| anyhow!("MICROMEGAS_OIDC_CONFIG environment variable not set"))?;
        let config: OidcConfig = serde_json::from_str(&json)
            .map_err(|e| anyhow!("Failed to parse MICROMEGAS_OIDC_CONFIG: {e:?}"))?;
        Ok(config)
    }
}

/// JWT Claims from OIDC ID token
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// Issuer - identifies the principal that issued the JWT
    iss: String,
    /// Subject - identifies the principal that is the subject of the JWT
    sub: String,
    /// Audience - identifies the recipients that the JWT is intended for
    #[serde(default)]
    aud: Vec<String>,
    /// Expiration time - identifies the expiration time on or after which the JWT must not be accepted
    exp: i64,
    /// Email address of the user (optional, provider-specific)
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
}

/// OIDC issuer client for token validation
struct OidcIssuerClient {
    issuer: String,
    audience: String,
    jwks_cache: JwksCache,
}

impl OidcIssuerClient {
    fn new(issuer: String, audience: String, jwks_ttl: Duration) -> Result<Self> {
        let issuer_url = IssuerUrl::new(issuer.clone())
            .map_err(|e| anyhow!("Invalid issuer URL '{}': {e:?}", issuer))?;

        Ok(Self {
            issuer,
            audience,
            jwks_cache: JwksCache::new(issuer_url, jwks_ttl),
        })
    }
}

/// Load admin users from environment variable
fn load_admin_users() -> Vec<String> {
    match std::env::var("MICROMEGAS_ADMINS") {
        Ok(json) => serde_json::from_str::<Vec<String>>(&json).unwrap_or_default(),
        Err(_) => vec![],
    }
}

/// Convert a JWK to a DecodingKey for jsonwebtoken
fn jwk_to_decoding_key(
    jwk: &openidconnect::core::CoreJsonWebKey,
) -> Result<jsonwebtoken::DecodingKey> {
    // Serialize the JWK to JSON to extract parameters
    let jwk_json =
        serde_json::to_value(jwk).map_err(|e| anyhow!("Failed to serialize JWK: {e:?}"))?;

    // Extract n and e parameters
    let n = jwk_json
        .get("n")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("JWK missing 'n' parameter"))?;
    let e = jwk_json
        .get("e")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("JWK missing 'e' parameter"))?;

    // Decode base64url encoded parameters
    let n_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(n.as_bytes())
        .map_err(|e| anyhow!("Failed to decode 'n': {e:?}"))?;
    let e_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(e.as_bytes())
        .map_err(|e| anyhow!("Failed to decode 'e': {e:?}"))?;

    // Create RSA public key
    let n_bigint = BigUint::from_bytes_be(&n_bytes);
    let e_bigint = BigUint::from_bytes_be(&e_bytes);

    let public_key = RsaPublicKey::new(n_bigint, e_bigint)
        .map_err(|e| anyhow!("Failed to create RSA public key: {e:?}"))?;

    // Convert to PEM format
    let pem = public_key
        .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
        .map_err(|e| anyhow!("Failed to encode public key as PEM: {e:?}"))?;

    // Create DecodingKey
    jsonwebtoken::DecodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|e| anyhow!("Failed to create decoding key: {e:?}"))
}

/// OIDC authentication provider
///
/// Validates ID tokens from configured OIDC providers.
/// Caches both JWKS and validated tokens for performance.
pub struct OidcAuthProvider {
    /// Map from issuer URL to issuer client
    clients: HashMap<String, Arc<OidcIssuerClient>>,
    /// Cache for validated tokens
    token_cache: Cache<String, Arc<AuthContext>>,
    /// Admin users (by email or subject)
    admin_users: Vec<String>,
}

impl std::fmt::Debug for OidcAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthProvider")
            .field("clients", &self.clients.keys())
            .field("admin_users", &"(not printed)")
            .finish()
    }
}

impl OidcAuthProvider {
    /// Create a new OIDC authentication provider
    pub async fn new(config: OidcConfig) -> Result<Self> {
        if config.issuers.is_empty() {
            return Err(anyhow!("At least one OIDC issuer must be configured"));
        }

        let jwks_ttl = Duration::from_secs(config.jwks_refresh_interval_secs);
        let mut clients = HashMap::new();

        // Initialize a client for each configured issuer
        for issuer_config in config.issuers {
            let client = OidcIssuerClient::new(
                issuer_config.issuer.clone(),
                issuer_config.audience,
                jwks_ttl,
            )?;

            clients.insert(issuer_config.issuer, Arc::new(client));
        }

        // Create token cache
        let token_cache = Cache::builder()
            .max_capacity(config.token_cache_size)
            .time_to_live(Duration::from_secs(config.token_cache_ttl_secs))
            .build();

        // Load admin users from environment
        let admin_users = load_admin_users();

        Ok(Self {
            clients,
            token_cache,
            admin_users,
        })
    }

    fn is_admin(&self, subject: &str, email: Option<&str>) -> bool {
        self.admin_users
            .iter()
            .any(|admin| admin == subject || email.map(|e| admin == e).unwrap_or(false))
    }

    /// Validate an ID token and return authentication context
    async fn validate_id_token(&self, token: &str) -> Result<AuthContext> {
        // Try to decode with each configured issuer until one works
        // This is a simplified approach - in production we'd decode the payload first to get the issuer
        for client in self.clients.values() {
            // Get JWKS from cache or fetch
            let jwks = match client.jwks_cache.get().await {
                Ok(jwks) => jwks,
                Err(_) => continue, // Try next issuer
            };

            // Try each key in the JWKS
            for key in jwks.keys() {
                // Convert JWK to DecodingKey
                let decoding_key = match jwk_to_decoding_key(key) {
                    Ok(key) => key,
                    Err(_) => continue, // Try next key
                };

                // Try to validate with this key
                let mut validation = Validation::new(Algorithm::RS256);
                // Don't validate audience yet - we'll do it manually
                validation.validate_aud = false;
                validation.set_issuer(&[&client.issuer]);

                match decode::<Claims>(token, &decoding_key, &validation) {
                    Ok(token_data) => {
                        let claims = token_data.claims;

                        // Manually validate audience
                        if !claims.aud.contains(&client.audience) {
                            return Err(anyhow!("Invalid audience"));
                        }

                        // Convert expiration to DateTime
                        let expires_at = DateTime::from_timestamp(claims.exp, 0)
                            .ok_or_else(|| anyhow!("Invalid expiration timestamp"))?;

                        if expires_at < Utc::now() {
                            return Err(anyhow!("Token has expired"));
                        }

                        // Check if user is admin
                        let is_admin = self.is_admin(&claims.sub, claims.email.as_deref());

                        return Ok(AuthContext {
                            subject: claims.sub,
                            email: claims.email,
                            issuer: claims.iss,
                            expires_at: Some(expires_at),
                            auth_type: AuthType::Oidc,
                            is_admin,
                        });
                    }
                    Err(_) => continue, // Try next key
                }
            }
        }

        Err(anyhow!("Failed to verify token with any configured issuer"))
    }
}

#[async_trait::async_trait]
impl AuthProvider for OidcAuthProvider {
    async fn validate_token(&self, token: &str) -> Result<AuthContext> {
        // Check token cache first
        if let Some(cached) = self.token_cache.get(token).await {
            return Ok((*cached).clone());
        }

        // Validate token
        let auth_ctx = self.validate_id_token(token).await?;

        // Cache the result
        self.token_cache
            .insert(token.to_string(), Arc::new(auth_ctx.clone()))
            .await;

        Ok(auth_ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oidc_config_parsing() {
        let json = r#"{
            "issuers": [
                {
                    "issuer": "https://accounts.google.com",
                    "audience": "test-client-id"
                }
            ]
        }"#;

        let config: OidcConfig = serde_json::from_str(json).expect("Failed to parse config");
        assert_eq!(config.issuers.len(), 1);
        assert_eq!(config.issuers[0].issuer, "https://accounts.google.com");
        assert_eq!(config.issuers[0].audience, "test-client-id");
        assert_eq!(config.jwks_refresh_interval_secs, 3600); // default
        assert_eq!(config.token_cache_size, 1000); // default
        assert_eq!(config.token_cache_ttl_secs, 300); // default
    }

    #[test]
    fn test_oidc_config_with_custom_values() {
        let json = r#"{
            "issuers": [
                {
                    "issuer": "https://accounts.google.com",
                    "audience": "test-client-id"
                }
            ],
            "jwks_refresh_interval_secs": 7200,
            "token_cache_size": 5000,
            "token_cache_ttl_secs": 600
        }"#;

        let config: OidcConfig = serde_json::from_str(json).expect("Failed to parse config");
        assert_eq!(config.jwks_refresh_interval_secs, 7200);
        assert_eq!(config.token_cache_size, 5000);
        assert_eq!(config.token_cache_ttl_secs, 600);
    }

    #[tokio::test]
    async fn test_oidc_provider_creation() {
        let config = OidcConfig {
            issuers: vec![OidcIssuer {
                issuer: "https://accounts.google.com".to_string(),
                audience: "test-client-id".to_string(),
            }],
            jwks_refresh_interval_secs: 3600,
            token_cache_size: 1000,
            token_cache_ttl_secs: 300,
        };

        let provider = OidcAuthProvider::new(config).await;
        assert!(provider.is_ok());

        let provider = provider.unwrap();
        assert_eq!(provider.clients.len(), 1);
        assert!(provider.clients.contains_key("https://accounts.google.com"));
    }

    #[tokio::test]
    async fn test_oidc_provider_empty_issuers() {
        let config = OidcConfig {
            issuers: vec![],
            jwks_refresh_interval_secs: 3600,
            token_cache_size: 1000,
            token_cache_ttl_secs: 300,
        };

        let provider = OidcAuthProvider::new(config).await;
        assert!(provider.is_err());
        assert!(
            provider
                .unwrap_err()
                .to_string()
                .contains("At least one OIDC issuer")
        );
    }
}
