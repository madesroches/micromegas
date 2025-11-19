//! URL validation utilities for authentication flows
//!
//! Provides secure validation functions to prevent common web vulnerabilities
//! like open redirects in OAuth flows and other authentication redirects.

use percent_encoding;
use url::Url;

/// Validate that a return URL is a safe relative path
///
/// This function prevents open redirect attacks by ensuring the URL:
/// 1. Is URL-decoded to prevent encoding bypasses
/// 2. Is a relative path starting with exactly one /
/// 3. Does not contain protocol-relative URLs (//) or absolute URLs
/// 4. Does not contain backslashes (which some browsers treat as forward slashes)
/// 5. Parses correctly as a relative URL
///
/// # Security
///
/// This function defends against various open redirect attack vectors:
/// - URL encoding bypasses: `/%2F/evil.com` → `//evil.com`
/// - Protocol-relative URLs: `//evil.com`
/// - Absolute URLs: `https://evil.com`
/// - Backslash variants: `/\evil.com` (some browsers normalize to `//evil.com`)
/// - Encoded protocols: `/http%3A%2F%2Fevil.com`
///
/// # Examples
///
/// ```
/// use micromegas_auth::url_validation::validate_return_url;
///
/// // Valid relative paths
/// assert!(validate_return_url("/"));
/// assert!(validate_return_url("/dashboard"));
/// assert!(validate_return_url("/path/to/resource?query=value"));
/// assert!(validate_return_url("/path%20with%20spaces"));
///
/// // Invalid: absolute URLs
/// assert!(!validate_return_url("https://evil.com"));
/// assert!(!validate_return_url("//evil.com"));
///
/// // Invalid: URL-encoded open redirect attempts
/// assert!(!validate_return_url("/%2F/evil.com"));
/// assert!(!validate_return_url("/http%3A%2F%2Fevil.com"));
///
/// // Invalid: backslash variants
/// assert!(!validate_return_url("/\\evil.com"));
/// ```
pub fn validate_return_url(url: &str) -> bool {
    // Decode URL to prevent encoding bypasses like /%2F/evil.com
    // Use percent_decode_str which handles all URL encoding variants
    let decoded = match percent_encoding::percent_decode_str(url).decode_utf8() {
        Ok(s) => s.to_string(),
        Err(_) => return false, // Invalid UTF-8 after decoding
    };

    // Must start with exactly one /
    if !decoded.starts_with('/') {
        return false;
    }

    // Must not start with // (protocol-relative URL)
    if decoded.starts_with("//") {
        return false;
    }

    // Must not contain protocol markers (://), even after decoding
    if decoded.contains("://") {
        return false;
    }

    // Additional check: ensure no path traversal attempts after the first /
    // Reject URLs like /\\/evil.com or /\evil.com (backslash variants)
    if decoded.contains('\\') {
        return false;
    }

    // Validate that it parses as a valid relative URL when joined with a base
    Url::options()
        .base_url(Some(
            &Url::parse("http://localhost").expect("base URL should parse"),
        ))
        .parse(&decoded)
        .is_ok()
}
