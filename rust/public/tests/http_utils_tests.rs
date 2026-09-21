// Smoke tests for `micromegas::servers::http_utils::get_client_ip`'s own contribution over
// `micromegas_auth::client_ip::resolve_client_ip`: formatting an `IpAddr` as a string, and
// formatting `None` as `"unknown"`. The IP-resolution selection/fallback/anti-spoofing logic
// itself moved to `rust/auth/tests/client_ip_tests.rs`, against `resolve_client_ip` directly.

use http::{Extensions, HeaderMap};
use micromegas::servers::http_utils::get_client_ip;

#[test]
fn resolved_ip_is_formatted_as_a_string() {
    let mut headers = HeaderMap::new();
    headers.insert("x-real-ip", "203.0.113.42".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.42");
}

#[test]
fn nothing_available_returns_unknown() {
    let headers = HeaderMap::new();
    let extensions = Extensions::new();

    let ip = get_client_ip(&headers, &extensions);

    assert_eq!(ip, "unknown");
}
