//! User attribution validation for preventing impersonation attacks
//!
//! This module provides utilities for validating user attribution headers against
//! authenticated identity, preventing OIDC users from impersonating others while
//! allowing service accounts (API keys) to delegate on behalf of users.
//!
//! This is specifically designed for gRPC services using tonic metadata.

use micromegas_tracing::prelude::*;
use percent_encoding::percent_decode_str;
use tonic::{Status, metadata::MetadataMap};

/// Resolved user attribution from gRPC metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAttribution {
    /// The resolved user identifier (from x-user-id or auth token)
    pub user_id: String,
    /// The resolved user email (from x-user-email or auth token)
    pub user_email: String,
    /// The display name from x-user-name header (if provided)
    pub user_name: Option<String>,
    /// Service account name when delegation is being used
    pub service_account: Option<String>,
}

/// Extract header value, decoding percent-encoded UTF-8
/// Best-effort: logs warning and extracts printable chars on failure
fn get_header_string_lossy(metadata: &MetadataMap, key: &str) -> Option<String> {
    let value = metadata.get(key)?;

    match value.to_str() {
        Ok(s) => {
            // Decode percent-encoded UTF-8
            match percent_decode_str(s).decode_utf8() {
                Ok(decoded) => Some(decoded.into_owned()),
                Err(e) => {
                    warn!("Header '{key}' has invalid percent-encoded UTF-8: {e}");
                    Some(s.to_string()) // Use raw value as fallback
                }
            }
        }
        Err(_) => {
            // Header contains non-ASCII bytes - log and extract what we can
            let bytes = value.as_bytes();
            let printable: String = bytes
                .iter()
                .filter(|&&b| (0x20..=0x7E).contains(&b))
                .map(|&b| b as char)
                .collect();

            warn!(
                "Header '{key}' contains non-ASCII bytes, extracted printable portion: '{printable}'"
            );

            if !printable.is_empty() {
                Some(printable)
            } else {
                None
            }
        }
    }
}

/// Validate and resolve user attribution from gRPC metadata
///
/// This function prevents user impersonation by validating x-user-id and x-user-email
/// headers against the authenticated user's identity:
///
/// - **OIDC user tokens**: User identity MUST match token claims (no impersonation allowed)
/// - **API keys/service accounts**: Can act on behalf of users (delegation allowed)
/// - **Unauthenticated requests**: Pass through client-provided attribution
///
/// Header values support percent-encoded UTF-8 for international characters.
/// Invalid headers are handled gracefully with logging.
///
/// # Arguments
///
/// * `metadata` - gRPC metadata map (tonic::metadata::MetadataMap) containing authentication
///   and attribution headers
///
/// # Returns
///
/// Returns `Ok(UserAttribution)` containing:
/// - `user_id`: The resolved user identifier
/// - `user_email`: The resolved user email
/// - `user_name`: The display name from x-user-name header (if provided)
/// - `service_account`: `Some(name)` when delegation is being used, `None` otherwise
///
/// # Errors
///
/// Returns `Err(Box<Status::PermissionDenied>)` if an OIDC user attempts to impersonate another user.
///
/// # Example
///
/// ```rust
/// use micromegas_auth::user_attribution::validate_and_resolve_user_attribution_grpc;
/// use tonic::metadata::MetadataMap;
///
/// let mut metadata = MetadataMap::new();
/// metadata.insert("x-auth-subject", "alice@example.com".parse().unwrap());
/// metadata.insert("x-auth-email", "alice@example.com".parse().unwrap());
/// metadata.insert("x-allow-delegation", "false".parse().unwrap());
/// metadata.insert("x-user-id", "alice@example.com".parse().unwrap());
///
/// let result = validate_and_resolve_user_attribution_grpc(&metadata);
/// assert!(result.is_ok());
/// ```
pub fn validate_and_resolve_user_attribution_grpc(
    metadata: &MetadataMap,
) -> Result<UserAttribution, Box<Status>> {
    // Extract authentication context from headers (set by AuthService tower layer)
    let auth_subject = metadata.get("x-auth-subject").and_then(|v| v.to_str().ok());
    let auth_email = metadata.get("x-auth-email").and_then(|v| v.to_str().ok());
    let allow_delegation = metadata
        .get("x-allow-delegation")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<bool>().ok())
        .unwrap_or(false);

    // Extract claimed user attribution from client (with percent-decoding support)
    let claimed_user_id = get_header_string_lossy(metadata, "x-user-id");
    let claimed_user_email = get_header_string_lossy(metadata, "x-user-email");
    let claimed_user_name = get_header_string_lossy(metadata, "x-user-name");

    // If no authentication context, allow unauthenticated access with client-provided attribution
    let Some(authenticated_subject) = auth_subject else {
        return Ok(UserAttribution {
            user_id: claimed_user_id.unwrap_or_else(|| "unknown".to_string()),
            user_email: claimed_user_email.unwrap_or_else(|| "unknown".to_string()),
            user_name: claimed_user_name,
            service_account: None,
        });
    };

    if allow_delegation {
        // Service account - can delegate (act on behalf of users)
        let has_delegation = claimed_user_id.is_some() || claimed_user_email.is_some();
        let user_id = claimed_user_id.unwrap_or_else(|| authenticated_subject.to_string());
        let user_email = claimed_user_email
            .or_else(|| auth_email.map(|s| s.to_string()))
            .unwrap_or_else(|| "service-account".to_string());

        // Return service account name to indicate delegation
        let service_account = if has_delegation {
            Some(authenticated_subject.to_string())
        } else {
            None
        };

        Ok(UserAttribution {
            user_id,
            user_email,
            user_name: claimed_user_name,
            service_account,
        })
    } else {
        // OIDC user token - must match token claims (no impersonation)

        // Validate x-user-id matches token subject (if provided)
        if let Some(ref claimed_id) = claimed_user_id
            && claimed_id != authenticated_subject
        {
            return Err(Box::new(Status::permission_denied(format!(
                "User impersonation not allowed: x-user-id '{}' does not match authenticated subject '{}'",
                claimed_id, authenticated_subject
            ))));
        }

        // Validate x-user-email matches token email (if both provided)
        if let (Some(claimed_email), Some(authenticated_email)) = (&claimed_user_email, auth_email)
            && claimed_email != authenticated_email
        {
            return Err(Box::new(Status::permission_denied(format!(
                "User impersonation not allowed: x-user-email '{}' does not match authenticated email '{}'",
                claimed_email, authenticated_email
            ))));
        }

        // Use token claims as authoritative source
        Ok(UserAttribution {
            user_id: authenticated_subject.to_string(),
            user_email: auth_email.unwrap_or("unknown").to_string(),
            user_name: claimed_user_name,
            service_account: None,
        })
    }
}
