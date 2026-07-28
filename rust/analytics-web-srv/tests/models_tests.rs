//! Unit tests for app_db models (name validation and normalization)

use analytics_web_srv::app_db::{normalize_name, validate_folder_path, validate_name};

#[test]
fn test_normalize_name() {
    assert_eq!(normalize_name("Error Logs"), "error-logs");
    assert_eq!(normalize_name("My Custom Screen"), "my-custom-screen");
    assert_eq!(normalize_name("Test--Name"), "test-name");
    assert_eq!(normalize_name("-leading-"), "leading");
    assert_eq!(normalize_name("UPPERCASE"), "uppercase");
    assert_eq!(normalize_name("with123numbers"), "with123numbers");
}

#[test]
fn test_validate_name_valid() {
    assert!(validate_name("error-logs").is_ok());
    assert!(validate_name("prod-metrics-v2").is_ok());
    assert!(validate_name("my-custom-screen").is_ok());
    assert!(validate_name("abc").is_ok());
    // Minimum valid length (3 chars)
    assert!(validate_name("abc").is_ok());
    // Maximum valid length (100 chars)
    let max_len_name = "a".repeat(100);
    assert!(validate_name(&max_len_name).is_ok());
    // Numbers allowed after first letter
    assert!(validate_name("logs123").is_ok());
    assert!(validate_name("a1b2c3").is_ok());
}

#[test]
fn test_validate_name_invalid() {
    // Too short
    assert_eq!(validate_name("ab").unwrap_err().code, "NAME_TOO_SHORT");

    // Reserved name
    assert_eq!(validate_name("new").unwrap_err().code, "RESERVED_NAME");

    // Invalid start (number)
    assert_eq!(validate_name("123test").unwrap_err().code, "INVALID_START");

    // Invalid start (hyphen)
    assert_eq!(validate_name("-test").unwrap_err().code, "INVALID_START");

    // Invalid end (hyphen)
    assert_eq!(validate_name("test-").unwrap_err().code, "INVALID_END");

    // Invalid character (uppercase) - starts with uppercase so INVALID_START
    assert_eq!(validate_name("Test").unwrap_err().code, "INVALID_START");

    // Invalid character (uppercase in middle)
    assert_eq!(
        validate_name("testName").unwrap_err().code,
        "INVALID_CHARACTER"
    );

    // Invalid character (space)
    assert_eq!(
        validate_name("test name").unwrap_err().code,
        "INVALID_CHARACTER"
    );

    // Consecutive hyphens
    assert_eq!(
        validate_name("test--name").unwrap_err().code,
        "CONSECUTIVE_HYPHENS"
    );

    // Too long (101 chars)
    let too_long = "a".repeat(101);
    assert_eq!(validate_name(&too_long).unwrap_err().code, "NAME_TOO_LONG");

    // Empty string
    assert_eq!(validate_name("").unwrap_err().code, "NAME_TOO_SHORT");

    // Invalid characters (underscore, dot, etc.)
    assert_eq!(
        validate_name("test_name").unwrap_err().code,
        "INVALID_CHARACTER"
    );
    assert_eq!(
        validate_name("test.name").unwrap_err().code,
        "INVALID_CHARACTER"
    );
}

// ---------------------------------------------------------------------------
// validate_folder_path
// ---------------------------------------------------------------------------

#[test]
fn test_validate_folder_path_root_is_valid() {
    assert!(validate_folder_path("").is_ok());
}

#[test]
fn test_validate_folder_path_valid_segments() {
    assert!(validate_folder_path("team").is_ok());
    assert!(validate_folder_path("team/dashboards").is_ok());
    assert!(validate_folder_path("team/dashboards/incidents").is_ok());
    // Single/double-char segments would be rejected by validate_name's min length of 3,
    // but are valid folder segments (min length 1).
    assert!(validate_folder_path("qa").is_ok());
    assert!(validate_folder_path("ui").is_ok());
    assert!(validate_folder_path("a").is_ok());
}

#[test]
fn test_validate_folder_path_numeric_only_segment_allowed() {
    // Folder segments may start/end with a digit, unlike screen names.
    assert!(validate_folder_path("2025").is_ok());
    assert!(validate_folder_path("archive/2025").is_ok());
}

#[test]
fn test_validate_folder_path_reserved_screen_name_allowed() {
    // "new" is reserved for screen names (the /screen/new route) but not for folders.
    assert!(validate_folder_path("new").is_ok());
    assert_eq!(validate_name("new").unwrap_err().code, "RESERVED_NAME");
}

#[test]
fn test_validate_folder_path_empty_segment_rejected() {
    // Leading/trailing/double slashes produce an empty segment.
    assert_eq!(
        validate_folder_path("/team").unwrap_err().code,
        "NAME_TOO_SHORT"
    );
    assert_eq!(
        validate_folder_path("team/").unwrap_err().code,
        "NAME_TOO_SHORT"
    );
    assert_eq!(
        validate_folder_path("team//dashboards").unwrap_err().code,
        "NAME_TOO_SHORT"
    );
}

#[test]
fn test_validate_folder_path_invalid_character_rejected() {
    assert_eq!(
        validate_folder_path("Team").unwrap_err().code,
        "INVALID_CHARACTER"
    );
    assert_eq!(
        validate_folder_path("team/my_folder").unwrap_err().code,
        "INVALID_CHARACTER"
    );
}

#[test]
fn test_validate_folder_path_consecutive_hyphens_rejected() {
    assert_eq!(
        validate_folder_path("team--dashboards").unwrap_err().code,
        "CONSECUTIVE_HYPHENS"
    );
}

#[test]
fn test_validate_folder_path_too_long_rejected() {
    // Each segment must be <= 100 chars, so build several long segments to exceed
    // the 1024-char total-path cap without exceeding any single segment's limit.
    let long_segment = "a".repeat(100);
    let long_path = std::iter::repeat_n(long_segment, 11)
        .collect::<Vec<_>>()
        .join("/");
    assert!(long_path.len() > 1024);
    assert_eq!(
        validate_folder_path(&long_path).unwrap_err().code,
        "PATH_TOO_LONG"
    );
}

#[test]
fn test_normalize_edge_cases() {
    // Multiple spaces become single hyphen
    assert_eq!(normalize_name("a   b"), "a-b");
    // Leading/trailing spaces removed
    assert_eq!(normalize_name("  test  "), "test");
    // Special characters removed
    assert_eq!(normalize_name("test@name!"), "testname");
    assert_eq!(normalize_name("test_name"), "testname");
    // Empty after normalization
    assert_eq!(normalize_name("@#$%"), "");
    // Unicode characters removed
    assert_eq!(normalize_name("tëst"), "tst");
}
