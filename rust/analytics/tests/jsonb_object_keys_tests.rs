use datafusion::arrow::array::{
    Array, ArrayRef, BinaryDictionaryBuilder, DictionaryArray, GenericBinaryBuilder, ListArray,
    StringArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use micromegas_analytics::dfext::jsonb::keys::make_jsonb_object_keys_udf;
use std::sync::Arc;

/// Helper function to create JSONB binary for an object
fn create_jsonb_object(json: &str) -> Vec<u8> {
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

/// Helper to extract keys from a Dictionary<Int32, List<Utf8>> result
fn extract_keys_from_dict_list(
    dict_array: &DictionaryArray<Int32Type>,
    index: usize,
) -> Vec<String> {
    let list_values = dict_array
        .values()
        .as_any()
        .downcast_ref::<ListArray>()
        .expect("Expected ListArray values");

    let key_index = dict_array.keys().value(index) as usize;
    let values = list_values.value(key_index);
    let string_array = values
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("Expected StringArray");

    (0..string_array.len())
        .map(|i| string_array.value(i).to_string())
        .collect()
}

#[tokio::test]
async fn test_jsonb_object_keys_simple_object() {
    let jsonb1 = create_jsonb_object(r#"{"a": 1, "b": 2}"#);
    let jsonb2 = create_jsonb_object(r#"{"name": "server", "port": 8080}"#);

    let input = create_jsonb_binary_array(vec![jsonb1, jsonb2]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let result_array = result_batch.column(0);

    // Verify return type is Dictionary<Int32, List<Utf8>>
    assert!(
        matches!(
            result_array.data_type(),
            DataType::Dictionary(key_type, value_type)
            if matches!(key_type.as_ref(), DataType::Int32)
            && matches!(value_type.as_ref(), DataType::List(field) if matches!(field.data_type(), DataType::Utf8))
        ),
        "Expected Dictionary<Int32, List<Utf8>> return type, got {:?}",
        result_array.data_type()
    );

    let dict_array = result_array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    assert_eq!(dict_array.len(), 2);

    let keys1 = extract_keys_from_dict_list(dict_array, 0);
    assert_eq!(keys1.len(), 2);
    assert!(keys1.contains(&"a".to_string()));
    assert!(keys1.contains(&"b".to_string()));

    let keys2 = extract_keys_from_dict_list(dict_array, 1);
    assert_eq!(keys2.len(), 2);
    assert!(keys2.contains(&"name".to_string()));
    assert!(keys2.contains(&"port".to_string()));
}

#[tokio::test]
async fn test_jsonb_object_keys_with_dictionary_input() {
    let jsonb1 = create_jsonb_object(r#"{"x": 1}"#);
    let jsonb2 = create_jsonb_object(r#"{"y": 2, "z": 3}"#);

    // Dictionary with repeated values: [jsonb1, jsonb2, jsonb1, jsonb2]
    let input = create_jsonb_dictionary_array(
        vec![jsonb1, jsonb2],
        vec![Some(0), Some(1), Some(0), Some(1)],
    );

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let dict_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    assert_eq!(dict_array.len(), 4);

    let keys0 = extract_keys_from_dict_list(dict_array, 0);
    assert_eq!(keys0, vec!["x"]);

    let keys1 = extract_keys_from_dict_list(dict_array, 1);
    assert_eq!(keys1.len(), 2);
    assert!(keys1.contains(&"y".to_string()));
    assert!(keys1.contains(&"z".to_string()));

    let keys2 = extract_keys_from_dict_list(dict_array, 2);
    assert_eq!(keys2, vec!["x"]);

    let keys3 = extract_keys_from_dict_list(dict_array, 3);
    assert_eq!(keys3.len(), 2);

    // Verify dictionary deduplication: repeated inputs should share dictionary values
    // Keys at indices 0 and 2 should point to the same dictionary entry
    assert_eq!(dict_array.keys().value(0), dict_array.keys().value(2));
    // Keys at indices 1 and 3 should point to the same dictionary entry
    assert_eq!(dict_array.keys().value(1), dict_array.keys().value(3));
}

#[tokio::test]
async fn test_jsonb_object_keys_with_null_input() {
    let jsonb1 = create_jsonb_object(r#"{"key": "value"}"#);

    let input = create_jsonb_dictionary_array(vec![jsonb1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let dict_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    assert!(!dict_array.is_null(0));
    // Null input is stored as a null entry in the dictionary values
    // and the key points to that null entry
    let list_values = dict_array
        .values()
        .as_any()
        .downcast_ref::<ListArray>()
        .expect("Expected ListArray values");
    let key1 = dict_array.keys().value(1) as usize;
    assert!(list_values.is_null(key1)); // Null input produces null in dictionary
    assert!(!dict_array.is_null(2));
}

#[tokio::test]
async fn test_jsonb_object_keys_non_object_returns_null() {
    // Array is not an object
    let jsonb_array = create_jsonb_object(r#"[1, 2, 3]"#);
    // String is not an object
    let jsonb_string = create_jsonb_object(r#""hello""#);
    // Number is not an object
    let jsonb_number = create_jsonb_object(r#"42"#);

    let input = create_jsonb_binary_array(vec![jsonb_array, jsonb_string, jsonb_number]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let dict_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    let list_values = dict_array
        .values()
        .as_any()
        .downcast_ref::<ListArray>()
        .expect("Expected ListArray values");

    // All non-objects should return null (stored as null in dictionary)
    assert!(list_values.is_null(dict_array.keys().value(0) as usize)); // Array
    assert!(list_values.is_null(dict_array.keys().value(1) as usize)); // String
    assert!(list_values.is_null(dict_array.keys().value(2) as usize)); // Number
}

#[tokio::test]
async fn test_jsonb_object_keys_empty_object() {
    let jsonb_empty = create_jsonb_object(r#"{}"#);

    let input = create_jsonb_binary_array(vec![jsonb_empty]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let dict_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    assert!(!dict_array.is_null(0));
    let keys = extract_keys_from_dict_list(dict_array, 0);
    assert!(keys.is_empty()); // Empty object returns empty array
}

#[tokio::test]
async fn test_jsonb_object_keys_nested_object() {
    // Keys should only be top-level
    let jsonb_nested = create_jsonb_object(r#"{"outer": {"inner": "value"}, "sibling": 42}"#);

    let input = create_jsonb_binary_array(vec![jsonb_nested]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let dict_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    let keys = extract_keys_from_dict_list(dict_array, 0);
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&"outer".to_string()));
    assert!(keys.contains(&"sibling".to_string()));
    // "inner" should NOT be in the result
    assert!(!keys.contains(&"inner".to_string()));
}

#[tokio::test]
async fn test_jsonb_object_keys_preserves_key_order() {
    // Keys should be returned in the order they appear in the object
    let jsonb = create_jsonb_object(r#"{"first": 1, "second": 2, "third": 3}"#);

    let input = create_jsonb_binary_array(vec![jsonb]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_object_keys(jsonb_data) as keys FROM test_table")
        .await
        .expect("Failed to execute query");

    let results = df.collect().await.expect("Failed to collect results");
    let result_batch = &results[0];
    let dict_array = result_batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("Expected DictionaryArray");

    let keys = extract_keys_from_dict_list(dict_array, 0);
    assert_eq!(keys.len(), 3);
    // The order should be preserved
    assert_eq!(keys[0], "first");
    assert_eq!(keys[1], "second");
    assert_eq!(keys[2], "third");
}
