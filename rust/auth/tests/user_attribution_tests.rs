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
    create_metadata_with_name(
        auth_subject,
        auth_email,
        allow_delegation,
        claimed_user_id,
        claimed_user_email,
        None,
    )
}

/// Helper to create metadata with auth, user attribution headers, and user name
fn create_metadata_with_name(
    auth_subject: Option<&str>,
    auth_email: Option<&str>,
    allow_delegation: bool,
    claimed_user_id: Option<&str>,
    claimed_user_email: Option<&str>,
    claimed_user_name: Option<&str>,
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
    if let Some(user_name) = claimed_user_name {
        metadata.insert("x-user-name", user_name.parse().unwrap());
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

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "alice@example.com");
    assert_eq!(attr.user_email, "alice@example.com");
    assert!(attr.user_name.is_none()); // No user name provided
    assert!(attr.service_account.is_none()); // No delegation
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

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "alice@example.com");
    assert_eq!(attr.user_email, "alice@example.com");
    assert!(attr.service_account.is_none());
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

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "alice@example.com");
    assert_eq!(attr.user_email, "alice@example.com");
    assert_eq!(attr.service_account, Some("backend-service".to_string()));
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

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "backend-service");
    assert_eq!(attr.user_email, "service@example.com");
    assert!(attr.service_account.is_none()); // No delegation used
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

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "test-user");
    assert_eq!(attr.user_email, "test@example.com");
    assert!(attr.service_account.is_none());
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

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "unknown");
    assert_eq!(attr.user_email, "unknown");
    assert!(attr.service_account.is_none());
}

// ============================================================================
// UTF-8 / Percent-encoding tests
// ============================================================================

#[test]
fn test_percent_encoded_utf8_user_name() {
    // Percent-encoded UTF-8 should be decoded: "José García" → "Jos%C3%A9%20Garc%C3%ADa"
    let metadata = create_metadata_with_name(
        None, // No auth
        None,
        false,
        Some("jose"),
        Some("jose@example.com"),
        Some("Jos%C3%A9%20Garc%C3%ADa"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "jose");
    assert_eq!(attr.user_email, "jose@example.com");
    assert_eq!(attr.user_name, Some("José García".to_string()));
}

#[test]
fn test_percent_encoded_german_umlaut() {
    // "Müller" → "M%C3%BCller"
    let metadata =
        create_metadata_with_name(None, None, false, Some("muller"), None, Some("M%C3%BCller"));

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let attr = result.unwrap();
    assert_eq!(attr.user_name, Some("Müller".to_string()));
}

#[test]
fn test_percent_encoded_cjk() {
    // "田中" → "%E7%94%B0%E4%B8%AD"
    let metadata = create_metadata_with_name(
        None,
        None,
        false,
        Some("tanaka"),
        None,
        Some("%E7%94%B0%E4%B8%AD"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let attr = result.unwrap();
    assert_eq!(attr.user_name, Some("田中".to_string()));
}

#[test]
fn test_plain_ascii_user_name() {
    // Plain ASCII should pass through unchanged
    let metadata = create_metadata_with_name(
        None,
        None,
        false,
        Some("john"),
        Some("john@example.com"),
        Some("John Smith"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "john");
    assert_eq!(attr.user_email, "john@example.com");
    assert_eq!(attr.user_name, Some("John Smith".to_string()));
}

#[test]
fn test_user_name_with_oidc() {
    // User name should pass through with OIDC authentication
    let metadata = create_metadata_with_name(
        Some("alice@example.com"),
        Some("alice@example.com"),
        false,
        Some("alice@example.com"),
        Some("alice@example.com"),
        Some("Alice%20Wonderland"), // Percent-encoded space
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let attr = result.unwrap();
    assert_eq!(attr.user_name, Some("Alice Wonderland".to_string()));
}

#[test]
fn test_user_name_with_delegation() {
    // User name should pass through with service account delegation
    let metadata = create_metadata_with_name(
        Some("backend-service"),
        None,
        true, // Delegation allowed
        Some("alice@example.com"),
        Some("alice@example.com"),
        Some("Alice%20Smith"),
    );

    let result = validate_and_resolve_user_attribution_grpc(&metadata);
    assert!(result.is_ok());

    let attr = result.unwrap();
    assert_eq!(attr.user_id, "alice@example.com");
    assert_eq!(attr.user_email, "alice@example.com");
    assert_eq!(attr.user_name, Some("Alice Smith".to_string()));
    assert_eq!(attr.service_account, Some("backend-service".to_string()));
}
