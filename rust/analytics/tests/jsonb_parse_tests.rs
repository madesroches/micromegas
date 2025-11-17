use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericBinaryArray, StringArray, StringDictionaryBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;
use jsonb::RawJsonb;
use micromegas_analytics::dfext::jsonb::parse::make_jsonb_parse_udf;
use std::sync::Arc;

/// Helper function to create a plain String array
fn create_string_array(values: Vec<&str>) -> ArrayRef {
    Arc::new(StringArray::from(values))
}

/// Helper function to create a Dictionary<Int32, Utf8> array
fn create_string_dictionary_array(values: Vec<&str>, keys: Vec<Option<i32>>) -> ArrayRef {
    let mut builder = StringDictionaryBuilder::<Int32Type>::new();

    for key in keys {
        match key {
            Some(idx) => {
                builder.append_value(values[idx as usize]);
            }
            None => {
                builder.append_null();
            }
        }
    }

    Arc::new(builder.finish())
}

/// Helper to create a RecordBatch from a JSON string array
fn create_record_batch(json_array: ArrayRef) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "json_data",
        json_array.data_type().clone(),
        true,
    )]));

    RecordBatch::try_new(schema, vec![json_array]).expect("Failed to create RecordBatch")
}

/// Helper to execute jsonb_parse and return results as JSONB bytes
async fn execute_jsonb_parse(batch: RecordBatch) -> Vec<Option<Vec<u8>>> {
    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_parse_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_parse(json_data) as result FROM test_table")
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
async fn test_jsonb_parse_with_string_input() {
    let json1 = r#"{"key": "value1"}"#;
    let json2 = r#"{"key": "value2"}"#;

    let input = create_string_array(vec![json1, json2]);
    let batch = create_record_batch(input);

    let results = execute_jsonb_parse(batch).await;

    assert_eq!(results.len(), 2);
    assert!(results[0].is_some());
    assert!(results[1].is_some());

    // Verify the JSONB content
    let jsonb1 = RawJsonb::new(results[0].as_ref().unwrap());
    let owned1 = jsonb1.get_by_name("key", true).unwrap().unwrap();
    let raw1 = owned1.as_raw();
    let value1 = raw1.as_str().unwrap().unwrap();
    assert_eq!(value1, "value1");

    let jsonb2 = RawJsonb::new(results[1].as_ref().unwrap());
    let owned2 = jsonb2.get_by_name("key", true).unwrap().unwrap();
    let raw2 = owned2.as_raw();
    let value2 = raw2.as_str().unwrap().unwrap();
    assert_eq!(value2, "value2");
}

#[tokio::test]
async fn test_jsonb_parse_with_dictionary_input() {
    let json1 = r#"{"status": "active"}"#;
    let json2 = r#"{"status": "inactive"}"#;

    // Dictionary with repeated values: [json1, json2, json1, json2]
    let input = create_string_dictionary_array(
        vec![json1, json2],
        vec![Some(0), Some(1), Some(0), Some(1)],
    );

    let batch = create_record_batch(input);

    let results = execute_jsonb_parse(batch).await;

    assert_eq!(results.len(), 4);

    // Verify all results are valid JSONB
    for result in &results {
        assert!(result.is_some());
    }

    // Verify content
    let jsonb0 = RawJsonb::new(results[0].as_ref().unwrap());
    let owned0 = jsonb0.get_by_name("status", true).unwrap().unwrap();
    let raw0 = owned0.as_raw();
    let status0 = raw0.as_str().unwrap().unwrap();
    assert_eq!(status0, "active");

    let jsonb1 = RawJsonb::new(results[1].as_ref().unwrap());
    let owned1 = jsonb1.get_by_name("status", true).unwrap().unwrap();
    let raw1 = owned1.as_raw();
    let status1 = raw1.as_str().unwrap().unwrap();
    assert_eq!(status1, "inactive");

    // Results 2 and 3 should match 0 and 1
    let jsonb2 = RawJsonb::new(results[2].as_ref().unwrap());
    let owned2 = jsonb2.get_by_name("status", true).unwrap().unwrap();
    let raw2 = owned2.as_raw();
    let status2 = raw2.as_str().unwrap().unwrap();
    assert_eq!(status2, "active");

    let jsonb3 = RawJsonb::new(results[3].as_ref().unwrap());
    let owned3 = jsonb3.get_by_name("status", true).unwrap().unwrap();
    let raw3 = owned3.as_raw();
    let status3 = raw3.as_str().unwrap().unwrap();
    assert_eq!(status3, "inactive");
}

#[tokio::test]
async fn test_jsonb_parse_with_nulls() {
    let json1 = r#"{"key": "value"}"#;

    let input = create_string_dictionary_array(vec![json1], vec![Some(0), None, Some(0)]);

    let batch = create_record_batch(input);

    let results = execute_jsonb_parse(batch).await;

    assert_eq!(results.len(), 3);
    assert!(results[0].is_some());
    assert!(results[1].is_none()); // Null input should produce null output
    assert!(results[2].is_some());
}

#[tokio::test]
async fn test_jsonb_parse_returns_dictionary_type() {
    let json1 = r#"{"test": "data"}"#;

    let input = create_string_array(vec![json1]);
    let batch = create_record_batch(input);

    let ctx = SessionContext::new();
    ctx.register_udf(make_jsonb_parse_udf());
    ctx.register_batch("test_table", batch)
        .expect("Failed to register batch");

    let df = ctx
        .sql("SELECT jsonb_parse(json_data) as result FROM test_table")
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
async fn test_jsonb_parse_with_invalid_json() {
    // Invalid JSON should produce null (not error)
    let valid_json = r#"{"key": "value"}"#;
    let invalid_json = r#"{"key": invalid}"#;

    let input = create_string_array(vec![valid_json, invalid_json]);
    let batch = create_record_batch(input);

    let results = execute_jsonb_parse(batch).await;

    assert_eq!(results.len(), 2);
    assert!(results[0].is_some()); // Valid JSON parsed
    assert!(results[1].is_none()); // Invalid JSON returns null
}

#[tokio::test]
async fn test_jsonb_parse_various_json_types() {
    // Test different JSON value types
    let json_object = r#"{"a": 1, "b": 2}"#;
    let json_array = r#"[1, 2, 3]"#;
    let json_string = r#""hello""#;
    let json_number = r#"42"#;
    let json_bool = r#"true"#;
    let json_null = r#"null"#;

    let input = create_string_array(vec![
        json_object,
        json_array,
        json_string,
        json_number,
        json_bool,
        json_null,
    ]);
    let batch = create_record_batch(input);

    let results = execute_jsonb_parse(batch).await;

    assert_eq!(results.len(), 6);

    // All should be successfully parsed
    for (i, result) in results.iter().enumerate() {
        assert!(result.is_some(), "Result {i} should not be null");
    }
}
