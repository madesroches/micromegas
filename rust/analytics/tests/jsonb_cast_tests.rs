use datafusion::arrow::array::{
    Array, ArrayAccessor, ArrayRef, BinaryDictionaryBuilder, DictionaryArray, Float64Array,
    GenericBinaryBuilder, Int64Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use micromegas_analytics::dfext::jsonb::cast::{
    make_jsonb_as_f64_udf, make_jsonb_as_i64_udf, make_jsonb_as_string_udf,
};
use std::sync::Arc;

/// Helper function to create JSONB binary for a string value
fn create_jsonb_string(value: &str) -> Vec<u8> {
    let json = format!("\"{}\"", value);
    jsonb::parse_value(json.as_bytes())
        .expect("Failed to parse JSON")
        .to_vec()
}

/// Helper function to create JSONB binary for a number
fn create_jsonb_number(value: f64) -> Vec<u8> {
    let json = value.to_string();
    jsonb::parse_value(json.as_bytes())
        .expect("Failed to parse JSON")
        .to_vec()
}

/// Helper function to create JSONB binary for an integer
fn create_jsonb_integer(value: i64) -> Vec<u8> {
    let json = value.to_string();
    jsonb::parse_value(json.as_bytes())
        .expect("Failed to parse JSON")
        .to_vec()
}

/// Helper function to create a Binary array with JSONB data
fn create_jsonb_binary_array(jsonb_data: Vec<Vec<u8>>) -> ArrayRef {
    let mut builder = GenericBinaryBuilder::<i32>::new();
    for data in jsonb_data {
        builder.append_value(&data);
    }
    Arc::new(builder.finish())
}

/// Helper function to create a Dictionary<Int32, Binary> array with JSONB data
fn create_jsonb_dictionary_array(jsonb_data: Vec<Vec<u8>>, keys: Vec<Option<i32>>) -> ArrayRef {
    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();

    for key in keys {
        match key {
            Some(idx) => {
                builder.append_value(&jsonb_data[idx as usize]);
            }
            None => {
                builder.append_null();
            }
        }
    }

    Arc::new(builder.finish())
}

/// Helper to create a RecordBatch from a JSONB array
fn create_record_batch(jsonb_array: ArrayRef) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "jsonb_data",
        jsonb_array.data_type().clone(),
        true,
    )]));

    RecordBatch::try_new(schema, vec![jsonb_array]).expect("Failed to create RecordBatch")
}

// ============================================================================
// jsonb_as_string tests
// ============================================================================

#[tokio::test]
async fn test_jsonb_as_string_with_binary_input() {
    let jsonb1 = create_jsonb_string("hello");
    let jsonb2 = create_jsonb_string("world");

    let input = create_jsonb_binary_array(vec![jsonb1, jsonb2]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_string_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_string(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch.column(0);

    // Verify return type is Dictionary<Int32, Utf8>
    assert!(
        matches!(
            result_array.data_type(),
            DataType::Dictionary(key_type, value_type)
            if matches!(key_type.as_ref(), DataType::Int32)
            && matches!(value_type.as_ref(), DataType::Utf8)
        ),
        "Expected Dictionary<Int32, Utf8> return type, got {:?}",
        result_array.data_type()
    );

    // Extract values
    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    let string_values = dict_array.downcast_dict::<datafusion::arrow::array::StringArray>();
    assert!(string_values.is_some());
    let string_values = string_values.unwrap();

    assert_eq!(string_values.value(0), "hello");
    assert_eq!(string_values.value(1), "world");
}

#[tokio::test]
async fn test_jsonb_as_string_with_dictionary_input() {
    let jsonb1 = create_jsonb_string("active");
    let jsonb2 = create_jsonb_string("inactive");

    // Dictionary with repeated values: [active, inactive, active, inactive]
    let input = create_jsonb_dictionary_array(
        vec![jsonb1, jsonb2],
        vec![Some(0), Some(1), Some(0), Some(1)],
    );

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_string_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_string(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch.column(0);

    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    let string_values = dict_array.downcast_dict::<datafusion::arrow::array::StringArray>();
    assert!(string_values.is_some());
    let string_values = string_values.unwrap();

    assert_eq!(string_values.len(), 4);
    assert_eq!(string_values.value(0), "active");
    assert_eq!(string_values.value(1), "inactive");
    assert_eq!(string_values.value(2), "active");
    assert_eq!(string_values.value(3), "inactive");
}

#[tokio::test]
async fn test_jsonb_as_string_with_nulls() {
    let jsonb1 = create_jsonb_string("value");

    let input = create_jsonb_dictionary_array(vec![jsonb1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_string_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_string(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch.column(0);

    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    assert!(!dict_array.is_null(0));
    assert!(dict_array.is_null(1));
    assert!(!dict_array.is_null(2));
}

#[tokio::test]
async fn test_jsonb_as_string_non_string_returns_null() {
    // JSONB number is not a string, should return null
    let jsonb1 = create_jsonb_number(42.0);

    let input = create_jsonb_binary_array(vec![jsonb1]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_string_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_string(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch.column(0);

    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    assert!(dict_array.is_null(0));
}

// ============================================================================
// jsonb_as_f64 tests
// ============================================================================

#[tokio::test]
async fn test_jsonb_as_f64_with_binary_input() {
    let jsonb1 = create_jsonb_number(3.14);
    let jsonb2 = create_jsonb_number(2.71);

    let input = create_jsonb_binary_array(vec![jsonb1, jsonb2]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_f64_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_f64(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("Expected Float64Array");

    assert_eq!(result_array.len(), 2);
    assert!((result_array.value(0) - 3.14).abs() < 0.001);
    assert!((result_array.value(1) - 2.71).abs() < 0.001);
}

#[tokio::test]
async fn test_jsonb_as_f64_with_dictionary_input() {
    let jsonb1 = create_jsonb_number(1.0);
    let jsonb2 = create_jsonb_number(2.0);

    let input = create_jsonb_dictionary_array(
        vec![jsonb1, jsonb2],
        vec![Some(0), Some(1), Some(0), Some(1)],
    );

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_f64_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_f64(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("Expected Float64Array");

    assert_eq!(result_array.len(), 4);
    assert_eq!(result_array.value(0), 1.0);
    assert_eq!(result_array.value(1), 2.0);
    assert_eq!(result_array.value(2), 1.0);
    assert_eq!(result_array.value(3), 2.0);
}

#[tokio::test]
async fn test_jsonb_as_f64_with_nulls() {
    let jsonb1 = create_jsonb_number(42.0);

    let input = create_jsonb_dictionary_array(vec![jsonb1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_f64_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_f64(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("Expected Float64Array");

    assert!(!result_array.is_null(0));
    assert!(result_array.is_null(1));
    assert!(!result_array.is_null(2));
}

// ============================================================================
// jsonb_as_i64 tests
// ============================================================================

#[tokio::test]
async fn test_jsonb_as_i64_with_binary_input() {
    let jsonb1 = create_jsonb_integer(42);
    let jsonb2 = create_jsonb_integer(-10);

    let input = create_jsonb_binary_array(vec![jsonb1, jsonb2]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_i64_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_i64(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("Expected Int64Array");

    assert_eq!(result_array.len(), 2);
    assert_eq!(result_array.value(0), 42);
    assert_eq!(result_array.value(1), -10);
}

#[tokio::test]
async fn test_jsonb_as_i64_with_dictionary_input() {
    let jsonb1 = create_jsonb_integer(100);
    let jsonb2 = create_jsonb_integer(200);

    let input = create_jsonb_dictionary_array(
        vec![jsonb1, jsonb2],
        vec![Some(0), Some(1), Some(0), Some(1)],
    );

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_i64_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_i64(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("Expected Int64Array");

    assert_eq!(result_array.len(), 4);
    assert_eq!(result_array.value(0), 100);
    assert_eq!(result_array.value(1), 200);
    assert_eq!(result_array.value(2), 100);
    assert_eq!(result_array.value(3), 200);
}

#[tokio::test]
async fn test_jsonb_as_i64_with_nulls() {
    let jsonb1 = create_jsonb_integer(999);

    let input = create_jsonb_dictionary_array(vec![jsonb1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_as_i64_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_as_i64(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("Expected Int64Array");

    assert!(!result_array.is_null(0));
    assert!(result_array.is_null(1));
    assert!(!result_array.is_null(2));
}
