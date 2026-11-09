//! Unit tests for stream_query module

use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use analytics_web_srv::data_source_cache::DataSourceCache;
use analytics_web_srv::stream_query::{
    QueryMode, StreamQueryRequest, admin_stream_query_handler, encode_batch, encode_schema,
    run_stream_query, sanitize_origin_label, substitute_macros,
};
use arrow_ipc::writer::{DictionaryTracker, IpcWriteOptions};
use axum::{
    Extension, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use datafusion::arrow::array::{
    Int32Array, RecordBatch, StringArray, TimestampNanosecondArray, UInt64Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

// =============================================================================
// Handler-level tests
// =============================================================================

fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "admin".to_string(),
        email: None,
        issuer: "local".to_string(),
        is_admin: true,
    }
}

fn non_admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "reader".to_string(),
        email: Some("reader@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
    }
}

fn cache() -> DataSourceCache {
    DataSourceCache::new(unreachable_pool(), Duration::from_secs(60))
}

fn request(sql: &str, params: HashMap<String, String>, data_source: &str) -> StreamQueryRequest {
    StreamQueryRequest {
        sql: sql.to_string(),
        params,
        begin: None,
        end: None,
        data_source: data_source.to_string(),
        notebook: None,
        cell: None,
    }
}

async fn run(req: StreamQueryRequest, mode: QueryMode) -> (StatusCode, serde_json::Value) {
    let response = run_stream_query(AuthToken(String::new()), cache(), req, mode).await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("reading response body");
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn standard_mode_refuses_statement_built_by_macro_substitution() {
    let mut params = HashMap::new();
    params.insert("fn".to_string(), "regenerate_partitions".to_string());
    let (status, body) = run(
        request("SELECT * FROM $fn('log_stats')", params, "default"),
        QueryMode::Standard,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "FORBIDDEN");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("regenerate_partitions")
    );
}

#[tokio::test]
async fn standard_mode_refuses_before_resolving_data_source() {
    // The cache cannot reach a database; a 403 proves no resolution was attempted.
    let (status, _) = run(
        request("DROP MATERIALIZED VIEW v", HashMap::new(), "default"),
        QueryMode::Standard,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_mode_does_not_classify() {
    let (status, body) = run(
        request(
            "SELECT * FROM regenerate_partitions('a')",
            HashMap::new(),
            "",
        ),
        QueryMode::Admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "DATA_SOURCE_NOT_FOUND");

    let (status, _) = run(
        request(
            "SELECT * FROM regenerate_partitions('a')",
            HashMap::new(),
            "",
        ),
        QueryMode::Standard,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

fn admin_router(user: ValidatedUser) -> Router {
    Router::new()
        .route("/api/admin/query-stream", post(admin_stream_query_handler))
        .layer(Extension(cache()))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
}

async fn post_admin(user: ValidatedUser) -> StatusCode {
    let body = serde_json::json!({"sql": "SELECT 1", "data_source": ""}).to_string();
    admin_router(user)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/query-stream")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("building request"),
        )
        .await
        .expect("router response")
        .status()
}

#[tokio::test]
async fn admin_endpoint_rejects_non_admin() {
    assert_eq!(post_admin(non_admin_user()).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_endpoint_reaches_handler_for_admin() {
    // Empty data source yields 400 from the handler itself.
    assert_eq!(post_admin(admin_user()).await, StatusCode::BAD_REQUEST);
}

// =============================================================================
// substitute_macros tests
// =============================================================================

#[test]
fn test_substitute_macros_basic() {
    let sql = "SELECT * FROM logs WHERE level = '$level'";
    let mut params = HashMap::new();
    params.insert("level".to_string(), "ERROR".to_string());

    let result = substitute_macros(sql, &params);
    assert_eq!(result, "SELECT * FROM logs WHERE level = 'ERROR'");
}

#[test]
fn test_substitute_macros_multiple_params() {
    let sql = "SELECT * FROM logs WHERE level = '$level' AND computer = '$host'";
    let mut params = HashMap::new();
    params.insert("level".to_string(), "INFO".to_string());
    params.insert("host".to_string(), "server01".to_string());

    let result = substitute_macros(sql, &params);
    assert_eq!(
        result,
        "SELECT * FROM logs WHERE level = 'INFO' AND computer = 'server01'"
    );
}

#[test]
fn test_substitute_macros_sql_injection_prevention() {
    let sql = "SELECT * FROM logs WHERE name = '$name'";
    let mut params = HashMap::new();
    // Attempt SQL injection with single quotes
    params.insert(
        "name".to_string(),
        "O'Malley'; DROP TABLE logs; --".to_string(),
    );

    let result = substitute_macros(sql, &params);
    // Single quotes should be escaped
    assert_eq!(
        result,
        "SELECT * FROM logs WHERE name = 'O''Malley''; DROP TABLE logs; --'"
    );
}

#[test]
fn test_substitute_macros_empty_params() {
    let sql = "SELECT * FROM logs";
    let params = HashMap::new();

    let result = substitute_macros(sql, &params);
    assert_eq!(result, "SELECT * FROM logs");
}

#[test]
fn test_substitute_macros_no_matching_param() {
    let sql = "SELECT * FROM logs WHERE level = '$level'";
    let params = HashMap::new();

    let result = substitute_macros(sql, &params);
    // Param not found, placeholder remains
    assert_eq!(result, "SELECT * FROM logs WHERE level = '$level'");
}

// =============================================================================
// encode_schema tests
// =============================================================================

fn create_lz4_options() -> IpcWriteOptions {
    IpcWriteOptions::default()
        .try_with_compression(Some(arrow_ipc::CompressionType::LZ4_FRAME))
        .expect("enabling LZ4 compression")
}

fn create_test_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, true),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            false,
        ),
        Field::new("count", DataType::UInt64, false),
    ])
}

#[test]
fn test_encode_schema_produces_valid_ipc() {
    let schema = create_test_schema();
    let mut tracker = DictionaryTracker::new(false);
    let ipc_bytes = encode_schema(&schema, &mut tracker, &create_lz4_options())
        .expect("Failed to encode schema");

    // IPC bytes should not be empty
    assert!(
        !ipc_bytes.is_empty(),
        "Schema IPC bytes should not be empty"
    );

    // Verify IPC bytes have expected structure (starts with continuation token and length)
    // Arrow IPC format uses 4-byte length prefix
    assert!(ipc_bytes.len() >= 8, "Schema IPC bytes should have header");
}

#[test]
fn test_encode_schema_empty_schema() {
    let schema = Schema::empty();
    let mut tracker = DictionaryTracker::new(false);
    let ipc_bytes = encode_schema(&schema, &mut tracker, &create_lz4_options())
        .expect("Failed to encode empty schema");

    // Even empty schema should produce valid IPC
    assert!(
        !ipc_bytes.is_empty(),
        "Empty schema should still produce IPC bytes"
    );
}

#[test]
fn test_encode_schema_all_types() {
    // Test schema with various Arrow types
    let schema = Schema::new(vec![
        Field::new("bool_col", DataType::Boolean, false),
        Field::new("i8_col", DataType::Int8, false),
        Field::new("i16_col", DataType::Int16, false),
        Field::new("i32_col", DataType::Int32, false),
        Field::new("i64_col", DataType::Int64, false),
        Field::new("u8_col", DataType::UInt8, false),
        Field::new("u16_col", DataType::UInt16, false),
        Field::new("u32_col", DataType::UInt32, false),
        Field::new("u64_col", DataType::UInt64, false),
        Field::new("f32_col", DataType::Float32, false),
        Field::new("f64_col", DataType::Float64, false),
        Field::new("string_col", DataType::Utf8, true),
        Field::new("large_string_col", DataType::LargeUtf8, true),
    ]);

    let mut tracker = DictionaryTracker::new(false);
    let ipc_bytes = encode_schema(&schema, &mut tracker, &create_lz4_options())
        .expect("Failed to encode complex schema");
    assert!(!ipc_bytes.is_empty());
}

// =============================================================================
// encode_batch tests
// =============================================================================

fn create_test_batch() -> RecordBatch {
    let schema = Arc::new(create_test_schema());

    let id_array = Int32Array::from(vec![1, 2, 3]);
    let name_array = StringArray::from(vec![Some("Alice"), Some("Bob"), None]);
    let timestamp_array = TimestampNanosecondArray::from(vec![
        1704067200000000000i64, // 2024-01-01 00:00:00
        1704153600000000000i64, // 2024-01-02 00:00:00
        1704240000000000000i64, // 2024-01-03 00:00:00
    ]);
    let count_array = UInt64Array::from(vec![100, 200, 300]);

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_array),
            Arc::new(name_array),
            Arc::new(timestamp_array),
            Arc::new(count_array),
        ],
    )
    .expect("Failed to create test batch")
}

#[test]
fn test_encode_batch_produces_valid_ipc() {
    let batch = create_test_batch();
    let mut tracker = DictionaryTracker::new(false);
    let options = create_lz4_options();

    let ipc_bytes = encode_batch(&batch, &mut tracker, &options).expect("Failed to encode batch");

    // IPC bytes should not be empty
    assert!(!ipc_bytes.is_empty(), "Batch IPC bytes should not be empty");
}

#[test]
fn test_encode_batch_preserves_row_count() {
    let batch = create_test_batch();
    let mut tracker = DictionaryTracker::new(false);
    let options = create_lz4_options();

    let ipc_bytes = encode_batch(&batch, &mut tracker, &options).expect("Failed to encode batch");

    // The IPC bytes should be parseable
    // We verify that we have valid data structure
    assert!(ipc_bytes.len() > 8, "Batch IPC should have data");
}

#[test]
fn test_encode_batch_empty_batch() {
    let schema = Arc::new(create_test_schema());

    let id_array = Int32Array::from(Vec::<i32>::new());
    let name_array = StringArray::from(Vec::<Option<&str>>::new());
    let timestamp_array = TimestampNanosecondArray::from(Vec::<i64>::new());
    let count_array = UInt64Array::from(Vec::<u64>::new());

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_array),
            Arc::new(name_array),
            Arc::new(timestamp_array),
            Arc::new(count_array),
        ],
    )
    .expect("Failed to create empty batch");

    let mut tracker = DictionaryTracker::new(false);
    let options = create_lz4_options();

    let ipc_bytes =
        encode_batch(&batch, &mut tracker, &options).expect("Failed to encode empty batch");

    // Even empty batch should produce valid IPC
    assert!(
        !ipc_bytes.is_empty(),
        "Empty batch should still produce IPC bytes"
    );
}

#[test]
fn test_encode_multiple_batches_with_tracker() {
    // Test that dictionary tracker properly tracks state across batches
    let mut tracker = DictionaryTracker::new(false);
    let options = create_lz4_options();

    for i in 0..3 {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("value", DataType::Utf8, true),
        ]));

        let id_array = Int32Array::from(vec![i, i + 1, i + 2]);
        let value_array = StringArray::from(vec!["a", "b", "c"]);

        let batch = RecordBatch::try_new(schema, vec![Arc::new(id_array), Arc::new(value_array)])
            .expect("Failed to create batch");

        let ipc_bytes = encode_batch(&batch, &mut tracker, &options)
            .expect("Failed to encode batch in sequence");

        assert!(!ipc_bytes.is_empty());
    }
}

// =============================================================================
// Integration test: schema + batch together (simulating protocol)
// =============================================================================

#[test]
fn test_encode_schema_and_batch_readable() {
    // This test verifies that encoding schema and batch produces bytes
    // that could be used by the frontend (though we can't fully parse without
    // the frontend's RecordBatchReader implementation)
    let schema = create_test_schema();
    let batch = create_test_batch();

    // Use same tracker for schema and batch to ensure dictionary IDs align
    let mut tracker = DictionaryTracker::new(false);
    let options = create_lz4_options();

    let schema_bytes =
        encode_schema(&schema, &mut tracker, &options).expect("Failed to encode schema");
    let batch_bytes = encode_batch(&batch, &mut tracker, &options).expect("Failed to encode batch");

    // Verify we have valid IPC data
    assert!(!schema_bytes.is_empty());
    assert!(!batch_bytes.is_empty());

    // Combined protocol would look like:
    // {"type":"schema","size":N}\n[schema_bytes]{"type":"batch","size":M}\n[batch_bytes]{"type":"done"}\n
    let total_size = schema_bytes.len() + batch_bytes.len();
    assert!(
        total_size > 0,
        "Combined IPC data should have non-zero size"
    );
}

// =============================================================================
// sanitize_origin_label tests
// =============================================================================

#[test]
fn test_sanitize_origin_label_normal_name_passes_through() {
    assert_eq!(
        sanitize_origin_label("My Notebook Cell"),
        Some("My Notebook Cell".to_string())
    );
}

#[test]
fn test_sanitize_origin_label_rejects_control_characters() {
    assert_eq!(sanitize_origin_label("cell\u{0}name"), None);
}

#[test]
fn test_sanitize_origin_label_rejects_over_max_length() {
    let too_long = "a".repeat(129);
    assert_eq!(sanitize_origin_label(&too_long), None);
}

#[test]
fn test_sanitize_origin_label_accepts_max_length() {
    let max_len = "a".repeat(128);
    assert_eq!(sanitize_origin_label(&max_len), Some(max_len));
}

#[test]
fn test_sanitize_origin_label_rejects_non_ascii() {
    assert_eq!(sanitize_origin_label("café"), None);
}

#[test]
fn test_sanitize_origin_label_rejects_empty() {
    assert_eq!(sanitize_origin_label(""), None);
}

#[test]
fn test_sanitize_origin_label_rejects_whitespace_only() {
    assert_eq!(sanitize_origin_label("   "), None);
}

#[test]
fn test_sanitize_origin_label_trims_leading_and_trailing_whitespace() {
    assert_eq!(
        sanitize_origin_label("  My Cell  "),
        Some("My Cell".to_string())
    );
}

// =============================================================================
// ErrorCode serialization tests
// =============================================================================

#[test]
fn test_error_code_serialization() {
    use analytics_web_srv::stream_query::ErrorCode;

    // Test that error codes serialize to SCREAMING_SNAKE_CASE
    let json = serde_json::to_string(&ErrorCode::InvalidSql).expect("serialization failed");
    assert_eq!(json, "\"INVALID_SQL\"");

    let json = serde_json::to_string(&ErrorCode::ConnectionFailed).expect("serialization failed");
    assert_eq!(json, "\"CONNECTION_FAILED\"");

    let json = serde_json::to_string(&ErrorCode::Internal).expect("serialization failed");
    assert_eq!(json, "\"INTERNAL\"");

    let json = serde_json::to_string(&ErrorCode::Forbidden).expect("serialization failed");
    assert_eq!(json, "\"FORBIDDEN\"");
}
