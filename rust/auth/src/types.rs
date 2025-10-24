use anyhow::Result;
use chrono::{DateTime, Utc};

/// Authentication type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthType {
    /// API key authentication
    ApiKey,
    /// OIDC authentication
    Oidc,
}

/// Authentication context containing user identity and metadata
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// Unique subject identifier (e.g., user ID, service account ID)
    pub subject: String,
    /// Email address (if available)
    pub email: Option<String>,
    /// Issuer (for OIDC) or "api_key" for API key auth
    pub issuer: String,
    /// Token expiration time (if applicable)
    pub expires_at: Option<DateTime<Utc>>,
    /// Authentication type
    pub auth_type: AuthType,
    /// Whether this user has admin privileges
    pub is_admin: bool,
}

/// Trait for authentication providers
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    /// Validate a bearer token and return authentication context
    async fn validate_token(&self, token: &str) -> Result<AuthContext>;
}
