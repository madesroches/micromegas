//! Unit tests for app_db models (screen name validation and normalization)

use analytics_web_srv::app_db::{normalize_screen_name, validate_screen_name};

#[test]
fn test_normalize_screen_name() {
    assert_eq!(normalize_screen_name("Error Logs"), "error-logs");
    assert_eq!(
        normalize_screen_name("My Custom Screen"),
        "my-custom-screen"
    );
    assert_eq!(normalize_screen_name("Test--Name"), "test-name");
    assert_eq!(normalize_screen_name("-leading-"), "leading");
    assert_eq!(normalize_screen_name("UPPERCASE"), "uppercase");
    assert_eq!(normalize_screen_name("with123numbers"), "with123numbers");
}

#[test]
fn test_validate_screen_name_valid() {
    assert!(validate_screen_name("error-logs").is_ok());
    assert!(validate_screen_name("prod-metrics-v2").is_ok());
    assert!(validate_screen_name("my-custom-screen").is_ok());
    assert!(validate_screen_name("abc").is_ok());
    // Minimum valid length (3 chars)
    assert!(validate_screen_name("abc").is_ok());
    // Maximum valid length (100 chars)
    let max_len_name = "a".repeat(100);
    assert!(validate_screen_name(&max_len_name).is_ok());
    // Numbers allowed after first letter
    assert!(validate_screen_name("logs123").is_ok());
    assert!(validate_screen_name("a1b2c3").is_ok());
}

#[test]
fn test_validate_screen_name_invalid() {
    // Too short
    assert_eq!(
        validate_screen_name("ab").unwrap_err().code,
        "NAME_TOO_SHORT"
    );

    // Reserved name
    assert_eq!(
        validate_screen_name("new").unwrap_err().code,
        "RESERVED_NAME"
    );

    // Invalid start (number)
    assert_eq!(
        validate_screen_name("123test").unwrap_err().code,
        "INVALID_START"
    );

    // Invalid start (hyphen)
    assert_eq!(
        validate_screen_name("-test").unwrap_err().code,
        "INVALID_START"
    );

    // Invalid end (hyphen)
    assert_eq!(
        validate_screen_name("test-").unwrap_err().code,
        "INVALID_END"
    );

    // Invalid character (uppercase) - starts with uppercase so INVALID_START
    assert_eq!(
        validate_screen_name("Test").unwrap_err().code,
        "INVALID_START"
    );

    // Invalid character (uppercase in middle)
    assert_eq!(
        validate_screen_name("testName").unwrap_err().code,
        "INVALID_CHARACTER"
    );

    // Invalid character (space)
    assert_eq!(
        validate_screen_name("test name").unwrap_err().code,
        "INVALID_CHARACTER"
    );

    // Consecutive hyphens
    assert_eq!(
        validate_screen_name("test--name").unwrap_err().code,
        "CONSECUTIVE_HYPHENS"
    );

    // Too long (101 chars)
    let too_long = "a".repeat(101);
    assert_eq!(
        validate_screen_name(&too_long).unwrap_err().code,
        "NAME_TOO_LONG"
    );

    // Empty string
    assert_eq!(validate_screen_name("").unwrap_err().code, "NAME_TOO_SHORT");

    // Invalid characters (underscore, dot, etc.)
    assert_eq!(
        validate_screen_name("test_name").unwrap_err().code,
        "INVALID_CHARACTER"
    );
    assert_eq!(
        validate_screen_name("test.name").unwrap_err().code,
        "INVALID_CHARACTER"
    );
}

#[test]
fn test_normalize_edge_cases() {
    // Multiple spaces become single hyphen
    assert_eq!(normalize_screen_name("a   b"), "a-b");
    // Leading/trailing spaces removed
    assert_eq!(normalize_screen_name("  test  "), "test");
    // Special characters removed
    assert_eq!(normalize_screen_name("test@name!"), "testname");
    assert_eq!(normalize_screen_name("test_name"), "testname");
    // Empty after normalization
    assert_eq!(normalize_screen_name("@#$%"), "");
    // Unicode characters removed
    assert_eq!(normalize_screen_name("tëst"), "tst");
}
