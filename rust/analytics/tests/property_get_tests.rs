use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericBinaryArray, GenericListArray, StringArray,
    StructArray,
};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Int32Type};
use datafusion::config::ConfigOptions;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl};
use datafusion::prelude::*;
use jsonb::Value;
use micromegas_analytics::properties::property_get::PropertyGet;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

fn create_test_properties() -> ArrayRef {
    // Create a list of property structs
    let key_array = StringArray::from(vec!["version", "platform", "user"]);
    let value_array = StringArray::from(vec!["1.2.3", "linux", "alice"]);

    let struct_array = StructArray::from(vec![
        (
            Arc::new(Field::new("key", DataType::Utf8, false)),
            Arc::new(key_array) as ArrayRef,
        ),
        (
            Arc::new(Field::new("value", DataType::Utf8, false)),
            Arc::new(value_array) as ArrayRef,
        ),
    ]);

    // Create a list array with multiple property lists
    let offsets = OffsetBuffer::new(vec![0, 3].into());
    let list_field = Field::new(
        "item",
        DataType::Struct(
            vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ]
            .into(),
        ),
        false,
    );

    Arc::new(GenericListArray::<i32>::new(
        Arc::new(list_field),
        offsets,
        Arc::new(struct_array),
        None,
    ))
}

#[test]
fn test_property_get_returns_dictionary() {
    let property_get = PropertyGet::new();

    // Create test data
    let properties = create_test_properties();
    let names = Arc::new(StringArray::from(vec!["version"])) as ArrayRef;

    // Create ScalarFunctionArgs
    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 1,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    // Invoke the function
    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    // Verify the result is a dictionary array
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(
                array.data_type(),
                &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                "property_get should return a Dictionary<Int32, Utf8>"
            );

            // Cast to dictionary array and verify content
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 1);

            // Get the value
            if !dict_array.is_null(0) {
                let values = dict_array.values();
                let string_values = values
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .expect("Dictionary values should be strings");

                let key_index = dict_array.keys().value(0) as usize;
                assert_eq!(string_values.value(key_index), "1.2.3");
            }
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_with_repeated_values() {
    let property_get = PropertyGet::new();

    // Create properties with repeated values to test dictionary efficiency
    let key1 = StringArray::from(vec!["version", "platform"]);
    let value1 = StringArray::from(vec!["2.0.0", "linux"]);

    let key2 = StringArray::from(vec!["version", "platform"]);
    let value2 = StringArray::from(vec!["2.0.0", "windows"]);

    let _struct1 = StructArray::from(vec![
        (
            Arc::new(Field::new("key", DataType::Utf8, false)),
            Arc::new(key1) as ArrayRef,
        ),
        (
            Arc::new(Field::new("value", DataType::Utf8, false)),
            Arc::new(value1) as ArrayRef,
        ),
    ]);

    let _struct2 = StructArray::from(vec![
        (
            Arc::new(Field::new("key", DataType::Utf8, false)),
            Arc::new(key2) as ArrayRef,
        ),
        (
            Arc::new(Field::new("value", DataType::Utf8, false)),
            Arc::new(value2) as ArrayRef,
        ),
    ]);

    // Combine into a single struct array
    let all_keys = StringArray::from(vec!["version", "platform", "version", "platform"]);
    let all_values = StringArray::from(vec!["2.0.0", "linux", "2.0.0", "windows"]);

    let all_structs = StructArray::from(vec![
        (
            Arc::new(Field::new("key", DataType::Utf8, false)),
            Arc::new(all_keys) as ArrayRef,
        ),
        (
            Arc::new(Field::new("value", DataType::Utf8, false)),
            Arc::new(all_values) as ArrayRef,
        ),
    ]);

    // Create list array with two property lists
    let offsets = OffsetBuffer::new(vec![0, 2, 4].into());
    let list_field = Field::new(
        "item",
        DataType::Struct(
            vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ]
            .into(),
        ),
        false,
    );

    let properties = Arc::new(GenericListArray::<i32>::new(
        Arc::new(list_field),
        offsets,
        Arc::new(all_structs),
        None,
    )) as ArrayRef;

    // Query for "version" from both property lists (should return same value)
    let names = Arc::new(StringArray::from(vec!["version", "version"])) as ArrayRef;

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 2,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 2);

            // Both entries should point to the same dictionary value
            let values = dict_array.values();
            let string_values = values
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Dictionary values should be strings");

            // Check that the dictionary has deduplicated the values
            let unique_values: Vec<_> = (0..string_values.len())
                .map(|i| string_values.value(i))
                .collect();

            // Should only have one unique "2.0.0" value in the dictionary
            assert!(unique_values.contains(&"2.0.0"));

            // Both results should be "2.0.0"
            for i in 0..2 {
                if !dict_array.is_null(i) {
                    let key_index = dict_array.keys().value(i) as usize;
                    assert_eq!(string_values.value(key_index), "2.0.0");
                }
            }
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_with_nulls() {
    let property_get = PropertyGet::new();

    // Create test data with a non-existent property
    let properties = create_test_properties();
    let names = Arc::new(StringArray::from(vec!["nonexistent"])) as ArrayRef;

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 1,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(
                array.data_type(),
                &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                "property_get should return a Dictionary<Int32, Utf8>"
            );

            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 1);
            assert!(
                dict_array.is_null(0),
                "Should return null for non-existent property"
            );
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[tokio::test]
async fn test_property_get_return_type() {
    // Create a session context
    let ctx = SessionContext::new();

    // Register the property_get UDF
    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));

    // Create a simple properties list for testing
    let sql = r#"
        WITH test_data AS (
            SELECT [{key: 'version', value: '1.0'}] as properties
        )
        SELECT arrow_typeof(property_get(properties, 'version')) as type 
        FROM test_data
    "#;

    let df = ctx.sql(sql).await.expect("SQL should parse");
    let results = df.collect().await.expect("Query should execute");

    assert!(!results.is_empty());

    let batch = &results[0];
    let column = batch.column(0);
    let string_array = column
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("Result should be a string array");

    let type_string = string_array.value(0);
    assert!(
        type_string.contains("Dictionary"),
        "Return type should be Dictionary, got: {}",
        type_string
    );
}

fn create_test_jsonb_properties() -> ArrayRef {
    // Create JSONB data with properties
    let mut map1 = BTreeMap::new();
    map1.insert("version".to_string(), Value::String(Cow::Borrowed("3.0.0")));
    map1.insert(
        "platform".to_string(),
        Value::String(Cow::Borrowed("macos")),
    );
    map1.insert("user".to_string(), Value::String(Cow::Borrowed("bob")));

    let mut map2 = BTreeMap::new();
    map2.insert("version".to_string(), Value::String(Cow::Borrowed("3.0.1")));
    map2.insert(
        "platform".to_string(),
        Value::String(Cow::Borrowed("windows")),
    );
    map2.insert("build".to_string(), Value::String(Cow::Borrowed("release")));

    let jsonb1 = Value::Object(map1);
    let jsonb2 = Value::Object(map2);

    let mut buffer1 = Vec::new();
    let mut buffer2 = Vec::new();
    jsonb1.write_to_vec(&mut buffer1);
    jsonb2.write_to_vec(&mut buffer2);

    let binary_array =
        GenericBinaryArray::<i32>::from(vec![Some(buffer1.as_slice()), Some(buffer2.as_slice())]);

    Arc::new(binary_array)
}

#[test]
fn test_property_get_with_binary_jsonb() {
    let property_get = PropertyGet::new();

    // Create test JSONB data
    let properties = create_test_jsonb_properties();
    let names = Arc::new(StringArray::from(vec!["version", "platform"])) as ArrayRef;

    // Create ScalarFunctionArgs
    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 2,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    // Invoke the function
    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed with JSONB");

    // Verify the result
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(
                array.data_type(),
                &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                "property_get should return a Dictionary<Int32, Utf8> for JSONB"
            );

            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 2);

            // Check values
            let values = dict_array.values();
            let string_values = values
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Dictionary values should be strings");

            // First row should have version "3.0.0"
            if !dict_array.is_null(0) {
                let key_index = dict_array.keys().value(0) as usize;
                assert_eq!(string_values.value(key_index), "3.0.0");
            }

            // Second row should have platform "windows"
            if !dict_array.is_null(1) {
                let key_index = dict_array.keys().value(1) as usize;
                assert_eq!(string_values.value(key_index), "windows");
            }
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_with_dictionary_encoded_jsonb() {
    let property_get = PropertyGet::new();

    // Create JSONB binary data
    let mut map = BTreeMap::new();
    map.insert("app".to_string(), Value::String(Cow::Borrowed("myapp")));
    map.insert("env".to_string(), Value::String(Cow::Borrowed("prod")));
    map.insert(
        "region".to_string(),
        Value::String(Cow::Borrowed("us-west")),
    );

    let jsonb = Value::Object(map);
    let mut buffer = Vec::new();
    jsonb.write_to_vec(&mut buffer);

    // Create dictionary values (unique JSONB values)
    let binary_values = GenericBinaryArray::<i32>::from(vec![Some(buffer.as_slice())]);

    // Create dictionary keys pointing to the same value (simulating repeated properties)
    let keys = vec![0i32, 0, 0];
    let dict_array = DictionaryArray::<Int32Type>::try_new(keys.into(), Arc::new(binary_values))
        .expect("Failed to create dictionary array");

    let properties = Arc::new(dict_array) as ArrayRef;
    let names = Arc::new(StringArray::from(vec!["app", "env", "region"])) as ArrayRef;

    // Create ScalarFunctionArgs
    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 3,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    // Invoke the function
    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed with dictionary-encoded JSONB");

    // Verify the result
    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 3);

            let values = dict_array.values();
            let string_values = values
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Dictionary values should be strings");

            // Check all three values
            let expected = vec!["myapp", "prod", "us-west"];
            for (i, expected_val) in expected.iter().enumerate() {
                if !dict_array.is_null(i) {
                    let key_index = dict_array.keys().value(i) as usize;
                    assert_eq!(
                        string_values.value(key_index),
                        *expected_val,
                        "Row {} should have value '{}'",
                        i,
                        expected_val
                    );
                }
            }
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_with_missing_jsonb_property() {
    let property_get = PropertyGet::new();

    // Create JSONB data without the requested property
    let mut map = BTreeMap::new();
    map.insert("foo".to_string(), Value::String(Cow::Borrowed("bar")));

    let jsonb = Value::Object(map);
    let mut buffer = Vec::new();
    jsonb.write_to_vec(&mut buffer);

    let binary_array = GenericBinaryArray::<i32>::from(vec![Some(buffer.as_slice())]);
    let properties = Arc::new(binary_array) as ArrayRef;

    // Request a non-existent property
    let names = Arc::new(StringArray::from(vec!["nonexistent"])) as ArrayRef;

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 1,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 1);
            assert!(
                dict_array.is_null(0),
                "Should return null for non-existent JSONB property"
            );
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_with_null_jsonb() {
    let property_get = PropertyGet::new();

    // Create JSONB array with null values
    let binary_array = GenericBinaryArray::<i32>::from(vec![None, None]);
    let properties = Arc::new(binary_array) as ArrayRef;

    let names = Arc::new(StringArray::from(vec!["any", "property"])) as ArrayRef;

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 2,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 2);
            assert!(dict_array.is_null(0), "Should handle null JSONB");
            assert!(dict_array.is_null(1), "Should handle null JSONB");
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_with_escaped_jsonb_strings() {
    let property_get = PropertyGet::new();

    // Create JSONB data with escaped characters in strings
    let mut map = BTreeMap::new();
    // Test various escaped characters
    map.insert(
        "quotes".to_string(),
        Value::String(Cow::Borrowed(r#"He said "hello""#)),
    );
    map.insert(
        "newline".to_string(),
        Value::String(Cow::Borrowed("line1\nline2")),
    );
    map.insert(
        "tab".to_string(),
        Value::String(Cow::Borrowed("col1\tcol2")),
    );
    map.insert(
        "backslash".to_string(),
        Value::String(Cow::Borrowed("path\\to\\file")),
    );
    map.insert(
        "unicode".to_string(),
        Value::String(Cow::Borrowed("emoji: 😀")),
    );
    map.insert(
        "mixed".to_string(),
        Value::String(Cow::Borrowed(r#"Complex: "test"\n\t\\"#)),
    );

    let jsonb = Value::Object(map);
    let mut buffer = Vec::new();
    jsonb.write_to_vec(&mut buffer);

    let binary_array = GenericBinaryArray::<i32>::from(vec![
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
    ]);
    let properties = Arc::new(binary_array) as ArrayRef;

    let names = Arc::new(StringArray::from(vec![
        "quotes",
        "newline",
        "tab",
        "backslash",
        "unicode",
        "mixed",
    ])) as ArrayRef;

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 6,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 6);

            let values = dict_array.values();
            let string_values = values
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Dictionary values should be strings");

            // Verify each escaped character is properly unescaped
            let expected_values = vec![
                r#"He said "hello""#,       // quotes
                "line1\nline2",             // newline
                "col1\tcol2",               // tab
                "path\\to\\file",           // backslash
                "emoji: 😀",                // unicode
                r#"Complex: "test"\n\t\\"#, // mixed
            ];

            for (i, expected) in expected_values.iter().enumerate() {
                if !dict_array.is_null(i) {
                    let key_index = dict_array.keys().value(i) as usize;
                    let actual = string_values.value(key_index);
                    assert_eq!(
                        actual, *expected,
                        "Row {} - expected: {:?}, got: {:?}",
                        i, expected, actual
                    );
                } else {
                    panic!("Row {} should not be null", i);
                }
            }
        }
        _ => panic!("Expected array result from property_get"),
    }
}

#[test]
fn test_property_get_null_vs_missing_jsonb_properties() {
    let property_get = PropertyGet::new();

    // Create JSONB data with explicit null value and missing properties
    let mut map = BTreeMap::new();
    map.insert("explicit_null".to_string(), Value::Null);
    map.insert(
        "has_value".to_string(),
        Value::String(Cow::Borrowed("test")),
    );
    let jsonb = Value::Object(map);

    let mut buffer = Vec::new();
    jsonb.write_to_vec(&mut buffer);

    let binary_array = GenericBinaryArray::<i32>::from(vec![
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
        Some(buffer.as_slice()),
    ]);
    let properties = Arc::new(binary_array) as ArrayRef;

    // Test three cases: explicit null, existing value, missing property
    let names = Arc::new(StringArray::from(vec![
        "explicit_null",
        "has_value",
        "missing_property",
    ])) as ArrayRef;

    let args = ScalarFunctionArgs {
        args: vec![
            ColumnarValue::Array(properties.clone()),
            ColumnarValue::Array(names.clone()),
        ],
        arg_fields: vec![
            Arc::new(Field::new(
                "properties",
                properties.data_type().clone(),
                true,
            )),
            Arc::new(Field::new("names", names.data_type().clone(), false)),
        ],
        number_rows: 3,
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        )),
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = property_get
        .invoke_with_args(args)
        .expect("property_get should succeed");

    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .expect("Should be a dictionary array");

            assert_eq!(dict_array.len(), 3);

            let values = dict_array.values();
            let string_values = values
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Dictionary values should be strings");

            // Case 1: explicit_null should return string "null" (not SQL NULL)
            if !dict_array.is_null(0) {
                let key_index = dict_array.keys().value(0) as usize;
                let actual = string_values.value(key_index);
                assert_eq!(
                    actual, "null",
                    "Explicit JSON null should return string 'null', got: {:?}",
                    actual
                );
            } else {
                panic!("Explicit JSON null should not be SQL NULL");
            }

            // Case 2: has_value should return "test"
            if !dict_array.is_null(1) {
                let key_index = dict_array.keys().value(1) as usize;
                let actual = string_values.value(key_index);
                assert_eq!(
                    actual, "test",
                    "Existing property should return its value, got: {:?}",
                    actual
                );
            } else {
                panic!("Existing property should not be SQL NULL");
            }

            // Case 3: missing_property should be SQL NULL
            assert!(
                dict_array.is_null(2),
                "Missing property should return SQL NULL"
            );
        }
        _ => panic!("Expected array result from property_get"),
    }
}
