//! Unit tests for data source validation and models (no database required)

use analytics_web_srv::app_db::{DataSourceConfig, ValidationError, validate_data_source_config};

// ---------------------------------------------------------------------------
// validate_data_source_config
// ---------------------------------------------------------------------------

fn config_json(url: &str) -> serde_json::Value {
    serde_json::json!({ "url": url })
}

#[test]
fn test_valid_https_url() {
    let result = validate_data_source_config(&config_json("https://flight.example.com:443"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap().url, "https://flight.example.com:443");
}

#[test]
fn test_valid_http_url() {
    let result = validate_data_source_config(&config_json("http://localhost:50051"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap().url, "http://localhost:50051");
}

#[test]
fn test_empty_url_rejected() {
    let result = validate_data_source_config(&config_json(""));
    let err = result.unwrap_err();
    assert_eq!(err.code, "MISSING_URL");
}

#[test]
fn test_missing_url_field_rejected() {
    let config = serde_json::json!({});
    let err = validate_data_source_config(&config).unwrap_err();
    assert_eq!(err.code, "INVALID_CONFIG");
}

#[test]
fn test_non_http_scheme_rejected() {
    let result = validate_data_source_config(&config_json("grpc://localhost:50051"));
    let err = result.unwrap_err();
    assert_eq!(err.code, "INVALID_URL");
}

#[test]
fn test_ftp_scheme_rejected() {
    let result = validate_data_source_config(&config_json("ftp://example.com"));
    let err = result.unwrap_err();
    assert_eq!(err.code, "INVALID_URL");
}

#[test]
fn test_no_scheme_rejected() {
    let result = validate_data_source_config(&config_json("example.com:50051"));
    let err = result.unwrap_err();
    assert_eq!(err.code, "INVALID_URL");
}

#[test]
fn test_case_insensitive_scheme() {
    assert!(validate_data_source_config(&config_json("HTTP://localhost:50051")).is_ok());
    assert!(validate_data_source_config(&config_json("HTTPS://localhost:50051")).is_ok());
    assert!(validate_data_source_config(&config_json("Https://localhost:50051")).is_ok());
}

#[test]
fn test_invalid_json_structure() {
    let config = serde_json::json!("just a string");
    let err = validate_data_source_config(&config).unwrap_err();
    assert_eq!(err.code, "INVALID_CONFIG");
}

#[test]
fn test_null_config_rejected() {
    let config = serde_json::json!(null);
    let err = validate_data_source_config(&config).unwrap_err();
    assert_eq!(err.code, "INVALID_CONFIG");
}

#[test]
fn test_extra_fields_ignored() {
    let config = serde_json::json!({ "url": "https://example.com", "extra": "field" });
    let result = validate_data_source_config(&config);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().url, "https://example.com");
}

// ---------------------------------------------------------------------------
// DataSourceConfig deserialization
// ---------------------------------------------------------------------------

#[test]
fn test_data_source_config_roundtrip() {
    let config = DataSourceConfig {
        url: "https://flight.example.com:443".to_string(),
    };
    let json = serde_json::to_value(&config).unwrap();
    let parsed: DataSourceConfig = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.url, config.url);
}

// ---------------------------------------------------------------------------
// ValidationError
// ---------------------------------------------------------------------------

#[test]
fn test_validation_error_fields() {
    let err = ValidationError::new("TEST_CODE", "test message");
    assert_eq!(err.code, "TEST_CODE");
    assert_eq!(err.message, "test message");
}
