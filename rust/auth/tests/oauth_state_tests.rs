//! Tests for OAuth state signing and verification

use base64::Engine;
use micromegas_auth::oauth_state::{OAuthState, sign_state, verify_state};

fn create_test_state() -> OAuthState {
    OAuthState {
        nonce: "test-nonce-12345".to_string(),
        return_url: "/dashboard".to_string(),
        pkce_verifier: "pkce-verifier-abc".to_string(),
    }
}

#[test]
fn test_sign_and_verify_state() {
    let state = create_test_state();
    let secret = b"test-secret-key-32-bytes-long!!!";

    let signed = sign_state(&state, secret).expect("signing should succeed");
    let verified = verify_state(&signed, secret).expect("verification should succeed");

    assert_eq!(verified, state);
}

#[test]
fn test_verify_rejects_tampered_state() {
    let state = create_test_state();
    let secret = b"test-secret-key-32-bytes-long!!!";

    let mut signed = sign_state(&state, secret).expect("signing should succeed");

    // Tamper with the signed state by modifying a character
    signed.replace_range(10..11, "X");

    let result = verify_state(&signed, secret);
    assert!(result.is_err(), "tampered state should be rejected");
}

#[test]
fn test_verify_rejects_wrong_secret() {
    let state = create_test_state();
    let secret1 = b"test-secret-key-32-bytes-long!!!";
    let secret2 = b"different-secret-32-bytes-long!!";

    let signed = sign_state(&state, secret1).expect("signing should succeed");
    let result = verify_state(&signed, secret2);

    assert!(result.is_err(), "wrong secret should be rejected");
}

#[test]
fn test_verify_rejects_invalid_format() {
    let secret = b"test-secret-key-32-bytes-long!!!";

    // Missing signature part
    let result = verify_state("only-one-part", secret);
    assert!(result.is_err(), "invalid format should be rejected");

    // Too many parts
    let result = verify_state("part1.part2.part3", secret);
    assert!(result.is_err(), "invalid format should be rejected");
}

#[test]
fn test_verify_rejects_invalid_base64() {
    let secret = b"test-secret-key-32-bytes-long!!!";

    let result = verify_state("invalid!!!base64.also!!!invalid", secret);
    assert!(result.is_err(), "invalid base64 should be rejected");
}

#[test]
fn test_sign_deterministic_with_same_input() {
    let state = create_test_state();
    let secret = b"test-secret-key-32-bytes-long!!!";

    let signed1 = sign_state(&state, secret).expect("signing should succeed");
    let signed2 = sign_state(&state, secret).expect("signing should succeed");

    assert_eq!(signed1, signed2);
}

#[test]
fn test_sign_different_with_different_return_url() {
    let state1 = create_test_state();
    let mut state2 = create_test_state();
    state2.return_url = "/different".to_string();

    let secret = b"test-secret-key-32-bytes-long!!!";

    let signed1 = sign_state(&state1, secret).expect("signing should succeed");
    let signed2 = sign_state(&state2, secret).expect("signing should succeed");

    assert_ne!(signed1, signed2);

    // But both should verify correctly
    let verified1 = verify_state(&signed1, secret).expect("verification should succeed");
    let verified2 = verify_state(&signed2, secret).expect("verification should succeed");
    assert_eq!(verified1, state1);
    assert_eq!(verified2, state2);
}

#[test]
fn test_signed_state_contains_two_base64_parts() {
    let state = create_test_state();
    let secret = b"test-secret-key-32-bytes-long!!!";

    let signed = sign_state(&state, secret).expect("signing should succeed");
    let parts: Vec<&str> = signed.split('.').collect();

    assert_eq!(parts.len(), 2);

    // Both parts should be valid base64
    assert!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .is_ok()
    );
    assert!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .is_ok()
    );
}
