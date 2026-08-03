//! Unit tests for `normalize_channel_scheme`, the pure helper that rewrites the
//! `grpc://`/`grpc+tls://` scheme convention used by data source configs into the
//! `http://`/`https://` scheme tonic's `Channel` expects for its TLS decision.

use micromegas::client::flightsql_client_factory::normalize_channel_scheme;

#[test]
fn test_grpc_scheme_becomes_http() {
    assert_eq!(
        normalize_channel_scheme("grpc://host:1234"),
        "http://host:1234"
    );
}

#[test]
fn test_grpc_tls_scheme_becomes_https() {
    assert_eq!(
        normalize_channel_scheme("grpc+tls://host:1234"),
        "https://host:1234"
    );
}

#[test]
fn test_http_scheme_unchanged() {
    assert_eq!(
        normalize_channel_scheme("http://host:1234"),
        "http://host:1234"
    );
}

#[test]
fn test_https_scheme_unchanged() {
    assert_eq!(
        normalize_channel_scheme("https://host:1234"),
        "https://host:1234"
    );
}

#[test]
fn test_mixed_case_scheme_preserves_host_casing() {
    assert_eq!(
        normalize_channel_scheme("GRPC://Host:1234"),
        "http://Host:1234"
    );
    assert_eq!(
        normalize_channel_scheme("GRPC+TLS://Host:1234"),
        "https://Host:1234"
    );
}
