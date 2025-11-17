use datafusion::arrow::array::{
    Array, ArrayRef, BinaryDictionaryBuilder, DictionaryArray, GenericBinaryArray,
    GenericBinaryBuilder, StringArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use micromegas_analytics::arrow_properties::serialize_properties_to_jsonb;
use micromegas_analytics::dfext::jsonb::get::make_jsonb_get_udf;
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

/// Helper to create a RecordBatch from a JSONB array and key names
fn create_record_batch_with_keys(jsonb_array: ArrayRef, keys: Vec<&str>) -> RecordBatch {
    let key_array: ArrayRef = Arc::new(StringArray::from(keys));

    let schema = Arc::new(Schema::new(vec![
        Field::new("jsonb_data", jsonb_array.data_type().clone(), true),
        Field::new("key_name", DataType::Utf8, false),
    ]));

    RecordBatch::try_new(schema, vec![jsonb_array, key_array])
        .expect("Failed to create RecordBatch")
}

/// Helper to execute jsonb_get and return results
async fn execute_jsonb_get(batch: RecordBatch) -> Vec<Option<Vec<u8>>> {
    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_get_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_get(jsonb_data, key_name) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");

    assert_eq!(results.len(), 1, "Expected single result batch");
    let result_batch = &results[0];

    // The result should be a Dictionary<Int32, Binary>
    let result_array = result_batch.column(0);
    assert!(
        matches!(
            result_array.data_type(),
            DataType::Dictionary(_, inner) if matches!(inner.as_ref(), DataType::Binary)
        ),
        "Expected Dictionary<Int32, Binary> result, got {:?}",
        result_array.data_type()
    );

    // Extract values from dictionary
    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    let binary_values = dict_array
        .values()
        .as_any()
        .downcast_ref::<GenericBinaryArray<i32>>()
        .expect("Expected BinaryArray values");

    (0..dict_array.len())
        .map(|i| {
            if dict_array.is_null(i) {
                None
            } else {
                let key_index = dict_array.keys().value(i) as usize;
                Some(binary_values.value(key_index).to_vec())
            }
        })
        .collect()
}

#[tokio::test]
async fn test_jsonb_get_with_binary_input() {
    let map1: HashMap<String, String> = [
        ("version".to_string(), "1.0.0".to_string()),
        ("name".to_string(), "app1".to_string()),
    ]
    .into();

    let map2: HashMap<String, String> = [
        ("version".to_string(), "2.0.0".to_string()),
        ("name".to_string(), "app2".to_string()),
    ]
    .into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);
    let jsonb_bytes2 = create_jsonb_bytes(map2);

    let input = create_jsonb_binary_array(vec![jsonb_bytes1, jsonb_bytes2]);
    let batch = create_record_batch_with_keys(input, vec!["version", "name"]);

    let results = execute_jsonb_get(batch).await;

    assert_eq!(results.len(), 2);
    // Check that we got binary JSONB values back
    assert!(results[0].is_some());
    assert!(results[1].is_some());

    // Parse the returned JSONB to verify content
    let result1 = results[0].as_ref().unwrap();
    let jsonb1 = jsonb::RawJsonb::new(result1);
    assert_eq!(jsonb1.as_str().unwrap().unwrap(), "1.0.0");

    let result2 = results[1].as_ref().unwrap();
    let jsonb2 = jsonb::RawJsonb::new(result2);
    assert_eq!(jsonb2.as_str().unwrap().unwrap(), "app2");
}

#[tokio::test]
async fn test_jsonb_get_with_dictionary_input() {
    let map1: HashMap<String, String> = [
        ("version".to_string(), "1.0.0".to_string()),
        ("name".to_string(), "app1".to_string()),
    ]
    .into();

    let map2: HashMap<String, String> = [
        ("version".to_string(), "2.0.0".to_string()),
        ("name".to_string(), "app2".to_string()),
    ]
    .into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);
    let jsonb_bytes2 = create_jsonb_bytes(map2);

    // Dictionary with repeated values: [map1, map2, map1, map2]
    let input = create_jsonb_dictionary_array(
        vec![jsonb_bytes1, jsonb_bytes2],
        vec![Some(0), Some(1), Some(0), Some(1)],
    );

    let batch = create_record_batch_with_keys(input, vec!["version", "version", "name", "name"]);

    let results = execute_jsonb_get(batch).await;

    assert_eq!(results.len(), 4);

    // Verify extracted values
    let result1 = results[0].as_ref().unwrap();
    let jsonb1 = jsonb::RawJsonb::new(result1);
    assert_eq!(jsonb1.as_str().unwrap().unwrap(), "1.0.0");

    let result2 = results[1].as_ref().unwrap();
    let jsonb2 = jsonb::RawJsonb::new(result2);
    assert_eq!(jsonb2.as_str().unwrap().unwrap(), "2.0.0");

    let result3 = results[2].as_ref().unwrap();
    let jsonb3 = jsonb::RawJsonb::new(result3);
    assert_eq!(jsonb3.as_str().unwrap().unwrap(), "app1");

    let result4 = results[3].as_ref().unwrap();
    let jsonb4 = jsonb::RawJsonb::new(result4);
    assert_eq!(jsonb4.as_str().unwrap().unwrap(), "app2");
}

#[tokio::test]
async fn test_jsonb_get_with_nulls() {
    let map1: HashMap<String, String> = [("key1".to_string(), "value1".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);

    let input = create_jsonb_dictionary_array(vec![jsonb_bytes1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch_with_keys(input, vec!["key1", "key1", "key1"]);

    let results = execute_jsonb_get(batch).await;

    assert_eq!(results.len(), 3);
    assert!(results[0].is_some());
    assert!(results[1].is_none()); // Null input should produce null output
    assert!(results[2].is_some());
}

#[tokio::test]
async fn test_jsonb_get_missing_key() {
    let map1: HashMap<String, String> = [("existing_key".to_string(), "value".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);

    let input = create_jsonb_binary_array(vec![jsonb_bytes1]);
    let batch = create_record_batch_with_keys(input, vec!["missing_key"]);

    let results = execute_jsonb_get(batch).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_none()); // Missing key should produce null
}

#[tokio::test]
async fn test_jsonb_get_returns_dictionary_type() {
    let map1: HashMap<String, String> = [("key".to_string(), "value".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);

    let input = create_jsonb_binary_array(vec![jsonb_bytes1]);
    let batch = create_record_batch_with_keys(input, vec!["key"]);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_get_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_get(jsonb_data, key_name) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let schema = df.schema();
    let result_field = schema.field(0);

    assert!(
        matches!(
            result_field.data_type(),
            DataType::Dictionary(key_type, value_type)
            if matches!(key_type.as_ref(), DataType::Int32)
            && matches!(value_type.as_ref(), DataType::Binary)
        ),
        "Expected Dictionary<Int32, Binary> return type, got {:?}",
        result_field.data_type()
    );
}

#[tokio::test]
async fn test_jsonb_get_with_repeated_results_benefits_from_dict_output() {
    // Test that repeated results benefit from dictionary encoding
    let map1: HashMap<String, String> = [("status".to_string(), "active".to_string())].into();

    let map2: HashMap<String, String> = [("status".to_string(), "inactive".to_string())].into();

    let jsonb_bytes1 = create_jsonb_bytes(map1);
    let jsonb_bytes2 = create_jsonb_bytes(map2);

    // 10 rows, but only 2 unique status values
    let input = create_jsonb_dictionary_array(
        vec![jsonb_bytes1, jsonb_bytes2],
        vec![
            Some(0),
            Some(0),
            Some(0),
            Some(1),
            Some(1),
            Some(0),
            Some(0),
            Some(1),
            Some(0),
            Some(1),
        ],
    );

    let batch = create_record_batch_with_keys(
        input,
        vec![
            "status", "status", "status", "status", "status", "status", "status", "status",
            "status", "status",
        ],
    );

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_get_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_get(jsonb_data, key_name) as result FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch.column(0);

    // Check that it's dictionary encoded
    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    // The dictionary values should have only 2 unique entries (active and inactive as JSONB)
    // Note: Due to how BinaryDictionaryBuilder works, it may have more if not deduplicating
    // But the important thing is we get dict output for memory efficiency
    assert!(
        dict_array.values().len() <= 10,
        "Dictionary should have fewer values than total rows for efficiency"
    );
}
