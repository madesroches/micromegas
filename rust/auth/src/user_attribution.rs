//! User attribution validation for preventing impersonation attacks
//!
//! This module provides utilities for validating user attribution headers against
//! authenticated identity, preventing OIDC users from impersonating others while
//! allowing service accounts (API keys) to delegate on behalf of users.
//!
//! This is specifically designed for gRPC services using tonic metadata.

use tonic::{Status, metadata::MetadataMap};

/// Validate and resolve user attribution from gRPC metadata
///
/// This function prevents user impersonation by validating x-user-id and x-user-email
/// headers against the authenticated user's identity:
///
/// - **OIDC user tokens**: User identity MUST match token claims (no impersonation allowed)
/// - **API keys/service accounts**: Can act on behalf of users (delegation allowed)
/// - **Unauthenticated requests**: Pass through client-provided attribution
///
/// # Arguments
///
/// * `metadata` - gRPC metadata map (tonic::metadata::MetadataMap) containing authentication
///   and attribution headers
///
/// # Returns
///
/// Returns `Ok((user_id, user_email, service_account_name))` where:
/// - `user_id`: The resolved user identifier
/// - `user_email`: The resolved user email
/// - `service_account_name`: `Some(name)` when delegation is being used, `None` otherwise
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
) -> Result<(String, String, Option<String>), Box<Status>> {
    // Extract authentication context from headers (set by AuthService tower layer)
    let auth_subject = metadata.get("x-auth-subject").and_then(|v| v.to_str().ok());
    let auth_email = metadata.get("x-auth-email").and_then(|v| v.to_str().ok());
    let allow_delegation = metadata
        .get("x-allow-delegation")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<bool>().ok())
        .unwrap_or(false);

    // Extract claimed user attribution from client
    let claimed_user_id = metadata.get("x-user-id").and_then(|v| v.to_str().ok());
    let claimed_user_email = metadata.get("x-user-email").and_then(|v| v.to_str().ok());

    // If no authentication context, allow unauthenticated access with client-provided attribution
    let Some(authenticated_subject) = auth_subject else {
        return Ok((
            claimed_user_id.unwrap_or("unknown").to_string(),
            claimed_user_email.unwrap_or("unknown").to_string(),
            None,
        ));
    };

    if allow_delegation {
        // Service account - can delegate (act on behalf of users)
        let user_id = claimed_user_id.unwrap_or(authenticated_subject).to_string();
        let user_email = claimed_user_email
            .or(auth_email)
            .unwrap_or("service-account")
            .to_string();

        // Return service account name to indicate delegation
        let service_account = if claimed_user_id.is_some() || claimed_user_email.is_some() {
            Some(authenticated_subject.to_string())
        } else {
            None
        };

        Ok((user_id, user_email, service_account))
    } else {
        // OIDC user token - must match token claims (no impersonation)

        // Validate x-user-id matches token subject (if provided)
        if let Some(claimed_id) = claimed_user_id
            && claimed_id != authenticated_subject
        {
            return Err(Box::new(Status::permission_denied(format!(
                "User impersonation not allowed: x-user-id '{}' does not match authenticated subject '{}'",
                claimed_id, authenticated_subject
            ))));
        }

        // Validate x-user-email matches token email (if both provided)
        if let (Some(claimed_email), Some(authenticated_email)) = (claimed_user_email, auth_email)
            && claimed_email != authenticated_email
        {
            return Err(Box::new(Status::permission_denied(format!(
                "User impersonation not allowed: x-user-email '{}' does not match authenticated email '{}'",
                claimed_email, authenticated_email
            ))));
        }

        // Use token claims as authoritative source
        let user_id = authenticated_subject.to_string();
        let user_email = auth_email.unwrap_or("unknown").to_string();

        Ok((user_id, user_email, None))
    }
}
