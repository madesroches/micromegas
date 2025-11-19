use http::HeaderMap;
use micromegas::servers::http_gateway::{HeaderForwardingConfig, build_origin_metadata};
use std::net::SocketAddr;

#[test]
fn test_default_config() {
    let config = HeaderForwardingConfig::default();

    // Should forward allowed headers
    assert!(config.should_forward("Authorization"));
    assert!(config.should_forward("authorization")); // case-insensitive
    assert!(config.should_forward("X-Request-ID"));
    assert!(config.should_forward("X-User-ID"));

    // Should block blocked headers
    assert!(!config.should_forward("Cookie"));
    assert!(!config.should_forward("Set-Cookie"));
    assert!(!config.should_forward("X-Client-IP"));

    // Should not forward unlisted headers
    assert!(!config.should_forward("X-Custom-Header"));
}

#[test]
fn test_prefix_matching() {
    let config = HeaderForwardingConfig {
        allowed_headers: vec![],
        allowed_prefixes: vec!["X-Custom-".to_string(), "X-Tenant-".to_string()],
        blocked_headers: vec![],
    };

    assert!(config.should_forward("X-Custom-Auth"));
    assert!(config.should_forward("x-custom-auth")); // case-insensitive
    assert!(config.should_forward("X-Tenant-ID"));
    assert!(!config.should_forward("X-Other-Header"));
}

#[test]
fn test_blocked_overrides_allowed() {
    let config = HeaderForwardingConfig {
        allowed_headers: vec!["X-Special".to_string()],
        allowed_prefixes: vec!["X-".to_string()],
        blocked_headers: vec!["X-Special".to_string()],
    };

    // Blocked should override allowed
    assert!(!config.should_forward("X-Special"));

    // Other X- headers should still work
    assert!(config.should_forward("X-Other"));
}

#[test]
fn test_case_insensitive() {
    let config = HeaderForwardingConfig::default();

    assert!(config.should_forward("authorization"));
    assert!(config.should_forward("AUTHORIZATION"));
    assert!(config.should_forward("Authorization"));

    assert!(!config.should_forward("cookie"));
    assert!(!config.should_forward("COOKIE"));
    assert!(!config.should_forward("Cookie"));
}

#[test]
fn test_build_origin_metadata_with_client_type() {
    let mut headers = HeaderMap::new();
    headers.insert("x-client-type", "web".parse().unwrap());
    let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    let metadata = build_origin_metadata(&headers, &addr);

    // Should augment client type with +gateway
    let client_type = metadata
        .get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(client_type, "web+gateway");

    // Should generate request ID
    assert!(metadata.get("x-request-id").is_some());

    // Should extract client IP
    let client_ip = metadata
        .get("x-client-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(client_ip, "127.0.0.1");
}

#[test]
fn test_build_origin_metadata_without_client_type() {
    let headers = HeaderMap::new();
    let addr: SocketAddr = "192.168.1.100:8080".parse().unwrap();

    let metadata = build_origin_metadata(&headers, &addr);

    // Should default to unknown+gateway
    let client_type = metadata
        .get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(client_type, "unknown+gateway");

    // Should generate request ID
    assert!(metadata.get("x-request-id").is_some());

    // Should extract client IP
    let client_ip = metadata
        .get("x-client-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(client_ip, "192.168.1.100");
}

#[test]
fn test_build_origin_metadata_forwards_request_id() {
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", "req-12345".parse().unwrap());
    let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    let metadata = build_origin_metadata(&headers, &addr);

    // Should forward existing request ID
    let request_id = metadata
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(request_id, "req-12345");
}

#[test]
fn test_build_origin_metadata_ignores_client_ip_header() {
    let mut headers = HeaderMap::new();
    // Client tries to spoof IP
    headers.insert("x-client-ip", "1.2.3.4".parse().unwrap());
    let addr: SocketAddr = "192.168.1.100:8080".parse().unwrap();

    let metadata = build_origin_metadata(&headers, &addr);

    // Should use real connection IP, not spoofed header
    let client_ip = metadata
        .get("x-client-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert_eq!(client_ip, "192.168.1.100");
}
