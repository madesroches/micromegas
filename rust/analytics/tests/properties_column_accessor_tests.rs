use anyhow::Result;
use datafusion::arrow::array::{
    Array, ArrayRef, BinaryDictionaryBuilder, ListBuilder, StringBuilder, StructBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_analytics::arrow_properties::serialize_properties_to_jsonb;
use micromegas_analytics::dfext::properties_column_accessor::{
    create_properties_accessor, properties_column_by_name,
};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn test_jsonb_dictionary_accessor() -> Result<()> {
    // Create a JSONB dictionary column
    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();

    // Add some JSONB data
    let props1: HashMap<String, String> = [("key1".to_string(), "value1".to_string())].into();
    let jsonb1 = serialize_properties_to_jsonb(&props1)?;
    builder.append_value(&jsonb1);

    let props2: HashMap<String, String> = [("key2".to_string(), "value2".to_string())].into();
    let jsonb2 = serialize_properties_to_jsonb(&props2)?;
    builder.append_value(&jsonb2);

    let array = Arc::new(builder.finish()) as ArrayRef;
    let accessor = create_properties_accessor(&array)?;

    assert_eq!(accessor.len(), 2);
    assert!(!accessor.is_null(0));
    assert!(!accessor.is_null(1));

    let result1 = accessor.jsonb_value(0)?;
    let result2 = accessor.jsonb_value(1)?;

    assert_eq!(result1, jsonb1);
    assert_eq!(result2, jsonb2);

    Ok(())
}

#[test]
fn test_struct_array_accessor() -> Result<()> {
    // Create a struct array (legacy format)
    let key_field = Field::new("key", DataType::Utf8, false);
    let value_field = Field::new("value", DataType::Utf8, false);
    let struct_fields = Fields::from(vec![key_field, value_field]);
    let struct_builder = StructBuilder::new(
        struct_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(StringBuilder::new()),
        ],
    );

    let mut list_builder = ListBuilder::new(struct_builder);

    // Add first property set: {"key1": "value1", "key2": "value2"}
    let props_builder = list_builder.values();
    let key_builder = props_builder.field_builder::<StringBuilder>(0).unwrap();
    key_builder.append_value("key1");
    let value_builder = props_builder.field_builder::<StringBuilder>(1).unwrap();
    value_builder.append_value("value1");
    props_builder.append(true);

    let key_builder = props_builder.field_builder::<StringBuilder>(0).unwrap();
    key_builder.append_value("key2");
    let value_builder = props_builder.field_builder::<StringBuilder>(1).unwrap();
    value_builder.append_value("value2");
    props_builder.append(true);
    list_builder.append(true);

    // Add second property set: {"key3": "value3"}
    let props_builder = list_builder.values();
    let key_builder = props_builder.field_builder::<StringBuilder>(0).unwrap();
    key_builder.append_value("key3");
    let value_builder = props_builder.field_builder::<StringBuilder>(1).unwrap();
    value_builder.append_value("value3");
    props_builder.append(true);
    list_builder.append(true);

    let array = Arc::new(list_builder.finish()) as ArrayRef;
    let accessor = create_properties_accessor(&array)?;

    assert_eq!(accessor.len(), 2);
    assert!(!accessor.is_null(0));
    assert!(!accessor.is_null(1));

    // Verify the first property set converts to correct JSONB
    let result1 = accessor.jsonb_value(0)?;
    let expected1: HashMap<String, String> = [
        ("key1".to_string(), "value1".to_string()),
        ("key2".to_string(), "value2".to_string()),
    ]
    .into();
    let expected_jsonb1 = serialize_properties_to_jsonb(&expected1)?;
    assert_eq!(result1, expected_jsonb1);

    // Verify the second property set converts to correct JSONB
    let result2 = accessor.jsonb_value(1)?;
    let expected2: HashMap<String, String> = [("key3".to_string(), "value3".to_string())].into();
    let expected_jsonb2 = serialize_properties_to_jsonb(&expected2)?;
    assert_eq!(result2, expected_jsonb2);

    Ok(())
}

#[test]
fn test_properties_column_by_name() -> Result<()> {
    // Create a record batch with JSONB format
    let mut jsonb_builder = BinaryDictionaryBuilder::<Int32Type>::new();
    let props: HashMap<String, String> = [("test".to_string(), "value".to_string())].into();
    let jsonb = serialize_properties_to_jsonb(&props)?;
    jsonb_builder.append_value(&jsonb);
    let jsonb_array = Arc::new(jsonb_builder.finish());

    let schema = Arc::new(Schema::new(vec![Field::new(
        "properties",
        jsonb_array.data_type().clone(),
        false,
    )]));

    let batch = RecordBatch::try_new(schema, vec![jsonb_array])?;
    let accessor = properties_column_by_name(&batch, "properties")?;

    assert_eq!(accessor.len(), 1);
    let result = accessor.jsonb_value(0)?;
    assert_eq!(result, jsonb);

    Ok(())
}

#[test]
fn test_empty_properties() -> Result<()> {
    // Test empty properties in both formats
    let mut jsonb_builder = BinaryDictionaryBuilder::<Int32Type>::new();
    let empty_props: HashMap<String, String> = HashMap::new();
    let empty_jsonb = serialize_properties_to_jsonb(&empty_props)?;
    jsonb_builder.append_value(&empty_jsonb);

    let array = Arc::new(jsonb_builder.finish()) as ArrayRef;
    let accessor = create_properties_accessor(&array)?;

    assert_eq!(accessor.len(), 1);
    let result = accessor.jsonb_value(0)?;
    assert_eq!(result, empty_jsonb);

    Ok(())
}

#[test]
fn test_format_detection_error() -> Result<()> {
    // Test that unsupported formats return appropriate errors
    use datafusion::arrow::array::Int32Array;

    let array = Arc::new(Int32Array::from(vec![1, 2, 3])) as ArrayRef;
    let result = create_properties_accessor(&array);

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Unsupported array type")
    );

    Ok(())
}
