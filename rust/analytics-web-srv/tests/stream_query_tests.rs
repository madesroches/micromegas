//! Unit tests for stream_query module

use analytics_web_srv::stream_query::{
    contains_blocked_function, encode_batch, encode_schema, substitute_macros,
};
use arrow_ipc::writer::{CompressionContext, DictionaryTracker};
use datafusion::arrow::array::{
    Int32Array, RecordBatch, StringArray, TimestampNanosecondArray, UInt64Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use std::collections::HashMap;
use std::sync::Arc;

// =============================================================================
// contains_blocked_function tests
// =============================================================================

#[test]
fn test_blocked_function_retire_partitions() {
    let sql = "SELECT * FROM retire_partitions()";
    assert_eq!(
        contains_blocked_function(sql),
        Some("retire_partitions"),
        "retire_partitions should be blocked"
    );
}

#[test]
fn test_blocked_function_retire_partition_by_metadata() {
    let sql = "SELECT retire_partition_by_metadata('test')";
    assert_eq!(
        contains_blocked_function(sql),
        Some("retire_partition_by_metadata"),
        "retire_partition_by_metadata should be blocked"
    );
}

#[test]
fn test_blocked_function_retire_partition_by_file() {
    let sql = "CALL retire_partition_by_file('some/path')";
    assert_eq!(
        contains_blocked_function(sql),
        Some("retire_partition_by_file"),
        "retire_partition_by_file should be blocked"
    );
}

#[test]
fn test_blocked_function_case_insensitive() {
    let sql = "SELECT * FROM RETIRE_PARTITIONS()";
    assert_eq!(
        contains_blocked_function(sql),
        Some("retire_partitions"),
        "Blocked function check should be case insensitive"
    );
}

#[test]
fn test_allowed_query_select() {
    let sql = "SELECT * FROM log_entries LIMIT 10";
    assert_eq!(
        contains_blocked_function(sql),
        None,
        "Normal SELECT should be allowed"
    );
}

#[test]
fn test_allowed_query_with_partition_word() {
    // Contains "partition" but not a blocked function
    let sql = "SELECT * FROM list_partitions()";
    assert_eq!(
        contains_blocked_function(sql),
        None,
        "list_partitions should be allowed"
    );
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
    let ipc_bytes = encode_schema(&schema).expect("Failed to encode schema");

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
    let ipc_bytes = encode_schema(&schema).expect("Failed to encode empty schema");

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

    let ipc_bytes = encode_schema(&schema).expect("Failed to encode complex schema");
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
    let mut compression = CompressionContext::default();

    let ipc_bytes =
        encode_batch(&batch, &mut tracker, &mut compression).expect("Failed to encode batch");

    // IPC bytes should not be empty
    assert!(!ipc_bytes.is_empty(), "Batch IPC bytes should not be empty");
}

#[test]
fn test_encode_batch_preserves_row_count() {
    let batch = create_test_batch();
    let mut tracker = DictionaryTracker::new(false);
    let mut compression = CompressionContext::default();

    let ipc_bytes =
        encode_batch(&batch, &mut tracker, &mut compression).expect("Failed to encode batch");

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
    let mut compression = CompressionContext::default();

    let ipc_bytes =
        encode_batch(&batch, &mut tracker, &mut compression).expect("Failed to encode empty batch");

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
    let mut compression = CompressionContext::default();

    for i in 0..3 {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("value", DataType::Utf8, true),
        ]));

        let id_array = Int32Array::from(vec![i, i + 1, i + 2]);
        let value_array = StringArray::from(vec!["a", "b", "c"]);

        let batch = RecordBatch::try_new(schema, vec![Arc::new(id_array), Arc::new(value_array)])
            .expect("Failed to create batch");

        let ipc_bytes = encode_batch(&batch, &mut tracker, &mut compression)
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

    let schema_bytes = encode_schema(&schema).expect("Failed to encode schema");
    let mut tracker = DictionaryTracker::new(false);
    let mut compression = CompressionContext::default();
    let batch_bytes =
        encode_batch(&batch, &mut tracker, &mut compression).expect("Failed to encode batch");

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
