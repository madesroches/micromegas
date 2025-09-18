use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericListArray, StringArray, StructArray,
};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Int32Type};
use datafusion::config::ConfigOptions;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl};
use datafusion::prelude::*;
use micromegas_analytics::properties::property_get::PropertyGet;
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
