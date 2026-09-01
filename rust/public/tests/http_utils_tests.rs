// Unit tests for `micromegas::servers::http_utils::get_client_ip`'s selection,
// fallback, and anti-spoofing rules: picks the *rightmost* `X-Forwarded-For`
// entry of the *last* header field line.

use axum::extract::ConnectInfo;
use http::{Extensions, HeaderMap};
use micromegas::servers::http_utils::get_client_ip;
use std::net::SocketAddr;

fn socket_addr(addr: &str) -> SocketAddr {
    addr.parse().expect("valid socket address")
}

#[test]
fn multi_entry_chain_returns_rightmost_entry_trimmed() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "1.2.3.4, 10.0.0.1, 198.51.100.9".parse().unwrap(),
    );

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "198.51.100.9");
}

#[test]
fn single_entry_chain_returns_that_entry() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.1".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.1");
}

#[test]
fn client_prepended_spoof_is_ignored_in_favor_of_alb_appended_entry() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "666.spoof, 198.51.100.9".parse().unwrap(),
    );

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "198.51.100.9");
}

#[test]
fn client_prepended_valid_looking_ip_is_still_ignored() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "203.0.113.1, 198.51.100.9".parse().unwrap(),
    );

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "198.51.100.9");
}

#[test]
fn two_separate_header_field_lines_uses_the_last_lines_rightmost_entry() {
    // A caller-sent `X-Forwarded-For` line followed by the ALB's own appended
    // line -- `HeaderMap::get` alone would return only the *first* line
    // (the caller's), which is exactly the bug `get_all(...).last()` fixes.
    let mut headers = HeaderMap::new();
    headers.append("x-forwarded-for", "666.spoof".parse().unwrap());
    headers.append("x-forwarded-for", "198.51.100.9".parse().unwrap());

    // Sanity check: `get` alone would indeed surface the caller's line.
    assert_eq!(headers.get("x-forwarded-for").unwrap(), "666.spoof");

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "198.51.100.9");
}

#[test]
fn no_x_forwarded_for_falls_back_to_x_real_ip() {
    let mut headers = HeaderMap::new();
    headers.insert("x-real-ip", "203.0.113.42".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.42");
}

#[test]
fn no_headers_falls_back_to_connect_info_extension() {
    let headers = HeaderMap::new();
    let mut extensions = Extensions::new();
    extensions.insert(ConnectInfo(socket_addr("192.168.1.100:8080")));

    let ip = get_client_ip(&headers, &extensions);

    assert_eq!(ip, "192.168.1.100");
}

#[test]
fn no_headers_falls_back_to_bare_socket_addr_extension() {
    // The tonic side: a bare `SocketAddr`, not wrapped in `ConnectInfo`.
    let headers = HeaderMap::new();
    let mut extensions = Extensions::new();
    extensions.insert(socket_addr("192.168.1.100:9000"));

    let ip = get_client_ip(&headers, &extensions);

    assert_eq!(ip, "192.168.1.100");
}

#[test]
fn nothing_available_returns_unknown() {
    let headers = HeaderMap::new();
    let extensions = Extensions::new();

    let ip = get_client_ip(&headers, &extensions);

    assert_eq!(ip, "unknown");
}

#[test]
fn present_but_empty_x_forwarded_for_falls_through() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "".parse().unwrap());
    headers.insert("x-real-ip", "203.0.113.42".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.42");
}

#[test]
fn comma_only_x_forwarded_for_falls_through() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", ",".parse().unwrap());
    headers.insert("x-real-ip", "203.0.113.42".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.42");
}

#[test]
fn trailing_comma_x_forwarded_for_returns_the_last_non_empty_entry() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.1, ".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.1");
}

#[test]
fn non_ip_rightmost_x_forwarded_for_falls_through_to_x_real_ip() {
    // Not fronted by the trusted ALB (or any proxy that validates/appends), a caller can put
    // arbitrary text -- including a forged `key=value` pair -- in the rightmost entry. That
    // entry must be rejected rather than returned verbatim, since it flows straight into
    // structured `key=value` log lines.
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "1.2.3.4, evil user=admin".parse().unwrap(),
    );
    headers.insert("x-real-ip", "203.0.113.42".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "203.0.113.42");
}

#[test]
fn non_ip_x_real_ip_falls_through_to_connect_info_extension() {
    let mut headers = HeaderMap::new();
    headers.insert("x-real-ip", "evil user=admin".parse().unwrap());
    let mut extensions = Extensions::new();
    extensions.insert(ConnectInfo(socket_addr("192.168.1.100:8080")));

    let ip = get_client_ip(&headers, &extensions);

    assert_eq!(ip, "192.168.1.100");
}

#[test]
fn non_ip_x_forwarded_for_with_no_other_source_returns_unknown() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "evil user=admin".parse().unwrap());

    let ip = get_client_ip(&headers, &Extensions::new());

    assert_eq!(ip, "unknown");
}
