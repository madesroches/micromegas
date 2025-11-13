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
