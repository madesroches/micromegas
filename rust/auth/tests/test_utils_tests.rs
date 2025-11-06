mod test_utils;

use test_utils::*;

#[test]
fn test_generate_keypair() {
    let keypair = TestKeyPair::generate();
    assert!(!keypair.public_key_pem.is_empty());
}

#[test]
fn test_create_and_verify_token() {
    let keypair = TestKeyPair::generate();
    let token = create_valid_token(
        &keypair,
        "https://test.example.com",
        "test-audience",
        "user123",
        Some("user@example.com"),
    );

    let claims = keypair
        .verify_token(&token)
        .expect("failed to verify token");
    assert_eq!(claims.sub, "user123");
    assert_eq!(claims.iss, "https://test.example.com");
    assert_eq!(claims.aud, "test-audience");
    assert_eq!(claims.email, Some("user@example.com".to_string()));
}

#[test]
fn test_expired_token() {
    let keypair = TestKeyPair::generate();
    let token = create_expired_token(
        &keypair,
        "https://test.example.com",
        "test-audience",
        "user123",
    );

    let result = keypair.verify_token(&token);
    assert!(result.is_err());
}
