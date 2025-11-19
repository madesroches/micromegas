use micromegas_auth::url_validation::validate_return_url;

#[test]
fn test_validate_return_url_valid_paths() {
    assert!(validate_return_url("/"));
    assert!(validate_return_url("/dashboard"));
    assert!(validate_return_url("/process/123"));
    assert!(validate_return_url("/path/to/resource?query=value"));
    assert!(validate_return_url("/path#anchor"));
    assert!(validate_return_url("/path?a=1&b=2"));
}

#[test]
fn test_validate_return_url_rejects_absolute_urls() {
    assert!(!validate_return_url("https://evil.com"));
    assert!(!validate_return_url("http://evil.com/path"));
    assert!(!validate_return_url("//evil.com/path"));
    assert!(!validate_return_url("javascript://alert(1)"));
}

#[test]
fn test_validate_return_url_rejects_non_slash_start() {
    assert!(!validate_return_url("path/to/resource"));
    assert!(!validate_return_url("dashboard"));
    assert!(!validate_return_url(""));
}

#[test]
fn test_validate_return_url_rejects_protocol_markers() {
    assert!(!validate_return_url("/path://something"));
    assert!(!validate_return_url("/foo://bar"));
}

#[test]
fn test_validate_return_url_rejects_url_encoded_double_slash() {
    // Security regression test: Prevent open redirect via URL encoding bypass
    // %2F encodes to /
    assert!(!validate_return_url("/%2F/evil.com"));
    assert!(!validate_return_url("/%2Fevil.com/phishing"));
    assert!(!validate_return_url("/%2F%2Fevil.com"));

    // Double encoding: %252F decodes to %2F (still contains %, which is valid in paths)
    // This is acceptable because it would need to be decoded again client-side,
    // and browsers don't auto-decode URLs in redirects
    // The important thing is single-encoded attacks are blocked
}

#[test]
fn test_validate_return_url_rejects_backslash_variants() {
    // Some browsers treat backslashes as forward slashes
    assert!(!validate_return_url("/\\evil.com"));
    assert!(!validate_return_url("/\\/evil.com"));
    assert!(!validate_return_url("/\\\\evil.com"));
}

#[test]
fn test_validate_return_url_rejects_encoded_protocols() {
    // URL-encoded protocol markers should be rejected after decoding
    assert!(!validate_return_url("/http%3A%2F%2Fevil.com"));
    assert!(!validate_return_url("/https%3A%2F%2Fevil.com"));
    assert!(!validate_return_url("/%2F%2Fevil.com%2Fphishing"));
}

#[test]
fn test_validate_return_url_accepts_encoded_valid_paths() {
    // Valid relative paths with URL encoding should still work
    assert!(validate_return_url("/path%20with%20spaces"));
    assert!(validate_return_url("/path?query=%20value"));
    assert!(validate_return_url("/user/%40username"));
}
