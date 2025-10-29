use micromegas_telemetry_sink::oidc_client_credentials_decorator::OidcClientCredentialsDecorator;

#[test]
fn test_decorator_creation() {
    // Verify struct creation works without panicking
    let _decorator = OidcClientCredentialsDecorator::new(
        "https://example.com/token".to_string(),
        "test-client".to_string(),
        "test-secret".to_string(),
    );
    // If we get here without panicking, the test passes
}
