use anyhow::Result;
use chrono::{DateTime, Utc};

/// A DB-backed auth provider (e.g. `DbApiKeyAuthProvider`) could not reach its
/// key store — distinguishes a store *outage* from a rejected credential all
/// the way out to the HTTP/gRPC response.
///
/// Wraps the underlying `anyhow::Error` rather than replacing it: callers that
/// only need to log or propagate the error keep using it as an `anyhow::Error`
/// (via `?`/`From`), while `MultiAuthProvider`, `auth_middleware`, and the gRPC
/// `AuthService` downcast (`anyhow::Error::downcast_ref`) to detect this specific
/// kind and map it to a retryable status (503 / `Status::unavailable`) instead of
/// the blanket "invalid credential" response every other error gets. This is the
/// one place in this crate a typed error is warranted, per `rust/CLAUDE.md`'s
/// anyhow-vs-thiserror rule: callers must branch on this specific kind.
#[derive(thiserror::Error, Debug)]
#[error("auth provider unavailable: {0}")]
pub struct ProviderUnavailable(#[source] pub anyhow::Error);

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
    /// Audience that was matched during token validation
    /// - For OIDC access tokens: API audience (e.g., "<https://api.example.com>")
    /// - For OIDC ID tokens: client ID
    /// - For API key auth: None
    pub audience: Option<String>,
    /// Token expiration time (if applicable)
    pub expires_at: Option<DateTime<Utc>>,
    /// Authentication type
    pub auth_type: AuthType,
    /// Whether this user has admin privileges
    pub is_admin: bool,
    /// Whether this authentication allows user delegation (acting on behalf of others)
    /// - OIDC user tokens: false (user cannot impersonate others)
    /// - API keys/service accounts: true (can act on behalf of users)
    pub allow_delegation: bool,
}

/// Trait for extracting authentication-relevant data from requests
pub trait RequestParts: Send + Sync {
    /// Extract Authorization header as string
    fn authorization_header(&self) -> Option<&str>;

    /// Extract Bearer token from Authorization header
    fn bearer_token(&self) -> Option<&str> {
        self.authorization_header()
            .and_then(|h| h.strip_prefix("Bearer "))
    }

    /// Get custom header value by name
    fn get_header(&self, name: &str) -> Option<&str>;

    /// Get request method (if applicable)
    fn method(&self) -> Option<&str>;

    /// Get request URI (if applicable)
    fn uri(&self) -> Option<&str>;
}

/// HTTP request validation input
pub struct HttpRequestParts {
    /// HTTP headers
    pub headers: http::HeaderMap,
    /// HTTP method
    pub method: http::Method,
    /// Request URI
    pub uri: http::Uri,
}

impl RequestParts for HttpRequestParts {
    fn authorization_header(&self) -> Option<&str> {
        self.headers
            .get(http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
    }

    fn get_header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|h| h.to_str().ok())
    }

    fn method(&self) -> Option<&str> {
        Some(self.method.as_str())
    }

    fn uri(&self) -> Option<&str> {
        Some(self.uri.path())
    }
}

/// gRPC request validation input (tonic metadata)
pub struct GrpcRequestParts {
    /// gRPC metadata map
    pub metadata: tonic::metadata::MetadataMap,
}

impl RequestParts for GrpcRequestParts {
    fn authorization_header(&self) -> Option<&str> {
        self.metadata
            .get("authorization")
            .and_then(|h| h.to_str().ok())
    }

    fn get_header(&self, name: &str) -> Option<&str> {
        self.metadata.get(name).and_then(|h| h.to_str().ok())
    }

    fn method(&self) -> Option<&str> {
        None
    }

    fn uri(&self) -> Option<&str> {
        None
    }
}

/// Trait for authentication providers
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    /// Validate a request and return authentication context
    async fn validate_request(&self, parts: &dyn RequestParts) -> Result<AuthContext>;
}
