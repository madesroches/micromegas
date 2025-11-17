use datafusion::arrow::array::{
    Array, ArrayAccessor, ArrayRef, BinaryDictionaryBuilder, DictionaryArray, GenericBinaryBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use micromegas_analytics::arrow_properties::serialize_properties_to_jsonb;
use micromegas_analytics::dfext::jsonb::format_json::make_jsonb_format_json_udf;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper function to create JSONB binary data from a simple key-value map
fn create_jsonb_bytes(properties: HashMap<String, String>) -> Vec<u8> {
    serialize_properties_to_jsonb(&properties).expect("Failed to create JSONB bytes")
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

/// Helper to execute jsonb_format_json and return results
async fn execute_jsonb_format_json(batch: RecordBatch) -> Vec<Option<String>> {
    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_format_json_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_format_json(jsonb_data) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");

    assert_eq!(results.len(), 1, "Expected single result batch");
    let result_batch = &results[0];

    // The result should be a Dictionary<Int32, Utf8>
    let result_array = result_batch.column(0);
    assert!(
        matches!(
            result_array.data_type(),
            DataType::Dictionary(_, inner) if matches!(inner.as_ref(), DataType::Utf8)
        ),
        "Expected Dictionary<Int32, Utf8> result, got {:?}",
        result_array.data_type()
    );

    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray result");

    let string_values = dict_array.downcast_dict::<datafusion::arrow::array::StringArray>();
    assert!(string_values.is_some());
    let string_values = string_values.unwrap();

    (0..string_values.len())
        .map(|i| {
            if dict_array.is_null(i) {
                None
            } else {
                Some(string_values.value(i).to_string())
            }
        })
        .collect()
}

#[tokio::test]
async fn test_jsonb_format_json_with_binary() {
    // This test should pass with the current implementation
    let map1: HashMap<String, String> = [
        ("key1".to_string(), "value1".to_string()),
        ("key2".to_string(), "value2".to_string()),
    ]
    .into();

    let map2: HashMap<String, String> = [("key3".to_string(), "value3".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);
    let jsonb_bytes2 = create_jsonb_bytes(map2);

    let input = create_jsonb_binary_array(vec![jsonb_bytes1, jsonb_bytes2]);
    let batch = create_record_batch(input);

    let results = execute_jsonb_format_json(batch).await;

    assert_eq!(results.len(), 2);

    // Verify results contain expected keys (HashMap order is deterministic via BTreeMap in serialization)
    let json_str1 = results[0].as_ref().expect("Expected non-null result");
    let json_str2 = results[1].as_ref().expect("Expected non-null result");

    assert!(json_str1.contains("\"key1\""));
    assert!(json_str1.contains("\"value1\""));
    assert!(json_str1.contains("\"key2\""));
    assert!(json_str1.contains("\"value2\""));

    assert!(json_str2.contains("\"key3\""));
    assert!(json_str2.contains("\"value3\""));
}

#[tokio::test]
async fn test_jsonb_format_json_with_dictionary() {
    // This test will FAIL with the current implementation
    // It demonstrates the bug: jsonb_format_json doesn't accept Dictionary<Int32, Binary>
    let map1: HashMap<String, String> = [
        ("key1".to_string(), "value1".to_string()),
        ("key2".to_string(), "value2".to_string()),
    ]
    .into();

    let map2: HashMap<String, String> = [("key3".to_string(), "value3".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);
    let jsonb_bytes2 = create_jsonb_bytes(map2);

    // Create dictionary array: [0, 1, 0, 1] -> references to unique values
    let input = create_jsonb_dictionary_array(
        vec![jsonb_bytes1, jsonb_bytes2],
        vec![Some(0), Some(1), Some(0), Some(1)], // Repeat pattern to demonstrate dictionary efficiency
    );

    let batch = create_record_batch(input);

    // This should work but will fail with current implementation
    let results = execute_jsonb_format_json(batch).await;

    assert_eq!(results.len(), 4);

    // Verify results
    let json_str1 = results[0].as_ref().expect("Expected non-null result");
    let json_str2 = results[1].as_ref().expect("Expected non-null result");
    let json_str3 = results[2].as_ref().expect("Expected non-null result");
    let json_str4 = results[3].as_ref().expect("Expected non-null result");

    // First and third should be identical (same dictionary key)
    assert!(json_str1.contains("\"key1\""));
    assert!(json_str1.contains("\"value1\""));
    assert!(json_str1.contains("\"key2\""));
    assert!(json_str1.contains("\"value2\""));

    assert!(json_str2.contains("\"key3\""));
    assert!(json_str2.contains("\"value3\""));

    assert_eq!(json_str1, json_str3); // Same dictionary key = same output
    assert_eq!(json_str2, json_str4); // Same dictionary key = same output
}

#[tokio::test]
async fn test_jsonb_format_json_with_dictionary_and_nulls() {
    // This test will also FAIL with the current implementation
    let map1: HashMap<String, String> = [("key1".to_string(), "value1".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);

    // Create dictionary array with null: [Some(0), None, Some(0)]
    let input = create_jsonb_dictionary_array(vec![jsonb_bytes1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch(input);

    let results = execute_jsonb_format_json(batch).await;

    assert_eq!(results.len(), 3);

    // First value should be valid
    let json_str1 = results[0].as_ref().expect("Expected non-null result");
    assert!(json_str1.contains("\"key1\""));
    assert!(json_str1.contains("\"value1\""));

    // Second value should be null
    assert!(results[1].is_none(), "Expected null value at index 1");

    // Third value should be valid (same as first)
    let json_str3 = results[2].as_ref().expect("Expected non-null result");
    assert!(json_str3.contains("\"key1\""));
    assert!(json_str3.contains("\"value1\""));

    assert_eq!(json_str1, json_str3); // Same dictionary key = same output
}

#[tokio::test]
async fn test_jsonb_format_json_empty_object() {
    // Test with empty JSONB object - should work with Binary
    let map: HashMap<String, String> = HashMap::new();
    let jsonb_bytes = create_jsonb_bytes(map);

    let input = create_jsonb_binary_array(vec![jsonb_bytes]);
    let batch = create_record_batch(input);

    let results = execute_jsonb_format_json(batch).await;

    assert_eq!(results.len(), 1);
    let json_str = results[0].as_ref().expect("Expected non-null result");
    assert_eq!(json_str, "{}");
}
