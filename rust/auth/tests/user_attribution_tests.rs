use micromegas_auth::user_attribution::validate_and_resolve_user_attribution_grpc;
use tonic::metadata::MetadataMap;

/// Helper to create metadata with auth and user attribution headers
fn create_metadata(
    auth_subject: Option<&str>,
    auth_email: Option<&str>,
    allow_delegation: bool,
    claimed_user_id: Option<&str>,
    claimed_user_email: Option<&str>,
) -> MetadataMap {
    let mut metadata = MetadataMap::new();

    if let Some(subject) = auth_subject {
        metadata.insert("x-auth-subject", subject.parse().unwrap());
    }
    if let Some(email) = auth_email {
        metadata.insert("x-auth-email", email.parse().unwrap());
    }
    metadata.insert(
        "x-allow-delegation",
        allow_delegation.to_string().parse().unwrap(),
    );

    if let Some(user_id) = claimed_user_id {
        metadata.insert("x-user-id", user_id.parse().unwrap());
    }
    if let Some(user_email) = claimed_user_email {
        metadata.insert("x-user-email", user_email.parse().unwrap());
    }

    metadata
}

#[test]
fn test_oidc_user_with_matching_headers() {
    // OIDC user with matching x-user-id and x-user-email should succeed
    let metadata = create_metadata(
        Some("alice@example.com"),
        Some("alice@example.com"),
        false, // OIDC user - no delegation
        Some("alice@example.com"),
        Some("alice@example.com"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let (user_id, user_email, service_account) = result.unwrap();
    assert_eq!(user_id, "alice@example.com");
    assert_eq!(user_email, "alice@example.com");
    assert!(service_account.is_none()); // No delegation
}

#[test]
fn test_oidc_user_with_no_user_headers() {
    // OIDC user without x-user-id/x-user-email should use token claims
    let metadata = create_metadata(
        Some("alice@example.com"),
        Some("alice@example.com"),
        false, // OIDC user - no delegation
        None,
        None,
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let (user_id, user_email, service_account) = result.unwrap();
    assert_eq!(user_id, "alice@example.com");
    assert_eq!(user_email, "alice@example.com");
    assert!(service_account.is_none());
}

#[test]
fn test_oidc_user_impersonation_via_user_id() {
    // OIDC user trying to impersonate another user via x-user-id should fail
    let metadata = create_metadata(
        Some("alice@example.com"),
        Some("alice@example.com"),
        false,                   // OIDC user - no delegation
        Some("bob@example.com"), // Trying to impersonate bob
        Some("alice@example.com"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_err());

    let err = *result.unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert!(err.message().contains("impersonation"));
}

#[test]
fn test_oidc_user_impersonation_via_email() {
    // OIDC user trying to impersonate another user via x-user-email should fail
    let metadata = create_metadata(
        Some("alice@example.com"),
        Some("alice@example.com"),
        false, // OIDC user - no delegation
        Some("alice@example.com"),
        Some("bob@example.com"), // Trying to impersonate bob's email
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_err());

    let err = *result.unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert!(err.message().contains("impersonation"));
}

#[test]
fn test_api_key_with_delegation() {
    // API key (service account) with x-user-id should delegate successfully
    let metadata = create_metadata(
        Some("backend-service"),
        None,
        true, // API key - delegation allowed
        Some("alice@example.com"),
        Some("alice@example.com"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let (user_id, user_email, service_account) = result.unwrap();
    assert_eq!(user_id, "alice@example.com");
    assert_eq!(user_email, "alice@example.com");
    assert_eq!(service_account, Some("backend-service".to_string()));
}

#[test]
fn test_api_key_without_delegation() {
    // API key without user headers should use service account identity
    let metadata = create_metadata(
        Some("backend-service"),
        Some("service@example.com"),
        true, // API key - delegation allowed
        None,
        None,
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let (user_id, user_email, service_account) = result.unwrap();
    assert_eq!(user_id, "backend-service");
    assert_eq!(user_email, "service@example.com");
    assert!(service_account.is_none()); // No delegation used
}

#[test]
fn test_unauthenticated_with_user_headers() {
    // Unauthenticated request with user headers should pass through
    let metadata = create_metadata(
        None, // No auth
        None,
        false,
        Some("test-user"),
        Some("test@example.com"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let (user_id, user_email, service_account) = result.unwrap();
    assert_eq!(user_id, "test-user");
    assert_eq!(user_email, "test@example.com");
    assert!(service_account.is_none());
}

#[test]
fn test_unauthenticated_without_user_headers() {
    // Unauthenticated request without user headers should default to unknown
    let metadata = create_metadata(
        None, // No auth
        None, false, None, None,
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let (user_id, user_email, service_account) = result.unwrap();
    assert_eq!(user_id, "unknown");
    assert_eq!(user_email, "unknown");
    assert!(service_account.is_none());
}
