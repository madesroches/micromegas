//! Validated-user / claims types shared between the middleware and handlers.

use base64::Engine;
use micromegas::auth::types::AuthContext;
use serde::{Deserialize, Serialize};

/// User info response
#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub(crate) sub: String,
    pub(crate) email: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) is_admin: bool,
}

/// JWT claims for decoding (minimal) - used for auth_me name extraction
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    #[serde(default)]
    name: Option<String>,
}

/// Validated user information extracted from JWT after signature verification
///
/// This struct is inserted into request extensions by the auth middleware
/// and can be used by handlers to access validated user information.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Struct inserted into request extensions; handlers access fields via Extension<ValidatedUser>
pub struct ValidatedUser {
    /// Unique subject identifier (user ID)
    pub subject: String,
    /// Email address (if available)
    pub email: Option<String>,
    /// Token issuer URL
    pub issuer: String,
    /// Whether this user has admin privileges
    pub is_admin: bool,
}

impl From<&AuthContext> for ValidatedUser {
    fn from(ctx: &AuthContext) -> Self {
        Self {
            subject: ctx.subject.clone(),
            email: ctx.email.clone(),
            issuer: ctx.issuer.clone(),
            is_admin: ctx.is_admin,
        }
    }
}

/// Request parts adapter for cookie-based tokens
///
/// Adapts a cookie-based token into the RequestParts trait expected by OidcAuthProvider.
/// Used by both cookie_auth_middleware and auth_me endpoint for token validation.
pub(crate) struct CookieTokenRequestParts {
    pub(crate) token: String,
}

impl micromegas::auth::types::RequestParts for CookieTokenRequestParts {
    fn authorization_header(&self) -> Option<&str> {
        None
    }

    fn bearer_token(&self) -> Option<&str> {
        Some(&self.token)
    }

    fn get_header(&self, _name: &str) -> Option<&str> {
        None
    }

    fn method(&self) -> Option<&str> {
        None
    }

    fn uri(&self) -> Option<&str> {
        None
    }
}

/// Extract the 'name' claim from a JWT payload
///
/// This is used by auth_me() to get the display name which isn't in AuthContext
pub(crate) fn extract_name_from_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()?;

    let claims: IdTokenClaims = serde_json::from_slice(&payload_bytes).ok()?;
    claims.name
}

/// Extract the 'sub' claim from a JWT payload for audit logging
pub(crate) fn extract_subject_from_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()?;

    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    claims["sub"].as_str().map(|s| s.to_string())
}
