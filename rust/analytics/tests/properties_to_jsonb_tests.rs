use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericBinaryArray, Int32Array, ListBuilder, StringBuilder,
    StructBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type};
use datafusion::config::ConfigOptions;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl};
use jsonb::RawJsonb;
use micromegas_analytics::properties::properties_to_dict_udf::PropertiesLength;
use micromegas_analytics::properties::properties_to_jsonb_udf::PropertiesToJsonb;
use std::sync::Arc;

fn create_test_properties_array(properties: Vec<Vec<(&str, &str)>>) -> ArrayRef {
    let key_field = Field::new("key", DataType::Utf8, false);
    let value_field = Field::new("value", DataType::Utf8, false);
    let struct_fields = Fields::from(vec![key_field, value_field]);

    let mut list_builder = ListBuilder::new(StructBuilder::new(
        struct_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(StringBuilder::new()),
        ],
    ));

    for props in properties {
        let struct_builder = list_builder.values();
        for (key, value) in props {
            struct_builder
                .field_builder::<StringBuilder>(0)
                .unwrap()
                .append_value(key);
            struct_builder
                .field_builder::<StringBuilder>(1)
                .unwrap()
                .append_value(value);
            struct_builder.append(true);
        }
        list_builder.append(true);
    }

    Arc::new(list_builder.finish())
}

fn create_jsonb_scalar_args(input: ArrayRef, num_rows: usize) -> ScalarFunctionArgs {
    ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(input)],
        arg_fields: vec![Arc::new(Field::new(
            "properties",
            DataType::List(Arc::new(Field::new(
                "item",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, false),
                ])),
                false,
            ))),
            true,
        ))],
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            true,
        )),
        number_rows: num_rows,
        config_options: Arc::new(ConfigOptions::default()),
    }
}

fn create_length_scalar_args(input: ArrayRef, num_rows: usize) -> ScalarFunctionArgs {
    let data_type = input.data_type().clone();
    ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(input)],
        arg_fields: vec![Arc::new(Field::new("properties", data_type, true))],
        return_field: Arc::new(Field::new("result", DataType::Int32, true)),
        number_rows: num_rows,
        config_options: Arc::new(ConfigOptions::default()),
    }
}

fn extract_jsonb_from_dictionary_result(result: ColumnarValue, index: usize) -> String {
    match result {
        ColumnarValue::Array(array) => {
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .unwrap();

            let values_array = dict_array.values();
            let binary_values = values_array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            if dict_array.is_null(index) {
                panic!("Expected non-null value at index {}", index);
            }

            let key_index = dict_array.keys().value(index) as usize;
            RawJsonb::new(binary_values.value(key_index)).to_string()
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_empty_properties() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![vec![]]);
    let args = create_jsonb_scalar_args(input, 1);

    let result = udf.invoke_with_args(args).unwrap();
    let jsonb_str = extract_jsonb_from_dictionary_result(result, 0);
    assert_eq!(jsonb_str, "{}");
}

#[test]
fn test_single_property() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![vec![("key1", "value1")]]);
    let args = create_jsonb_scalar_args(input, 1);

    let result = udf.invoke_with_args(args).unwrap();
    let jsonb_str = extract_jsonb_from_dictionary_result(result, 0);
    assert_eq!(jsonb_str, r#"{"key1":"value1"}"#);
}

#[test]
fn test_multiple_properties() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![
        vec![("key1", "value1"), ("key2", "value2")],
        vec![("key3", "value3")],
    ]);

    let args = create_jsonb_scalar_args(input, 2);

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 2);

            let jsonb_str1 =
                extract_jsonb_from_dictionary_result(ColumnarValue::Array(array.clone()), 0);
            let jsonb_str2 = extract_jsonb_from_dictionary_result(ColumnarValue::Array(array), 1);

            // Note: BTreeMap ensures sorted keys
            assert_eq!(jsonb_str1, r#"{"key1":"value1","key2":"value2"}"#);
            assert_eq!(jsonb_str2, r#"{"key3":"value3"}"#);
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_special_characters() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![vec![
        ("key with spaces", "value\"with\"quotes"),
        ("key🚀", "value\nwith\nnewlines"),
    ]]);

    let args = create_jsonb_scalar_args(input, 1);

    let result = udf.invoke_with_args(args).unwrap();
    let jsonb_str = extract_jsonb_from_dictionary_result(result, 0);
    // BTreeMap ensures sorted keys, and special characters should be properly escaped
    assert_eq!(
        jsonb_str,
        r#"{"key with spaces":"value\"with\"quotes","key🚀":"value\nwith\nnewlines"}"#
    );
}

#[test]
fn test_null_properties_list() {
    let udf = PropertiesToJsonb::new();

    // Create a list array with a null entry
    let key_field = Field::new("key", DataType::Utf8, false);
    let value_field = Field::new("value", DataType::Utf8, false);
    let struct_fields = Fields::from(vec![key_field, value_field]);

    let mut list_builder = ListBuilder::new(StructBuilder::new(
        struct_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(StringBuilder::new()),
        ],
    ));

    // Add one valid properties list and one null
    let struct_builder = list_builder.values();
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("key1");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("value1");
    struct_builder.append(true);
    list_builder.append(true);

    list_builder.append(false); // null entry

    let input = Arc::new(list_builder.finish());

    let args = create_jsonb_scalar_args(input, 2);

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 2);
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .unwrap();

            assert!(!dict_array.is_null(0));
            assert!(dict_array.is_null(1)); // null property list should result in null JSONB

            // First entry should be valid JSONB
            let jsonb_str = extract_jsonb_from_dictionary_result(ColumnarValue::Array(array), 0);
            assert_eq!(jsonb_str, r#"{"key1":"value1"}"#);
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_dictionary_encoded_properties() {
    let udf = PropertiesToJsonb::new();

    // First create a regular properties list array
    let properties_list = create_test_properties_array(vec![
        vec![("key1", "value1"), ("key2", "value2")],
        vec![("key3", "value3")],
        vec![("key1", "value1"), ("key2", "value2")], // Duplicate for dictionary efficiency
        vec![("key4", "value4"), ("key5", "value5")],
    ]);

    // Create dictionary-encoded array
    // Keys point to the indices in the values array
    // We want: [0, 1, 0, 3] to test deduplication (0 repeats for item 0 and 2)
    let keys_array = Int32Array::from(vec![0, 1, 0, 3]); // Indices 0,1,0,3 of properties_list
    let dict_array: DictionaryArray<Int32Type> =
        DictionaryArray::new(keys_array, properties_list.clone());

    let dict_array_ref: ArrayRef = Arc::new(dict_array);

    let args = ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(dict_array_ref)],
        arg_fields: vec![],
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            true,
        )),
        number_rows: 4,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 4);

            let jsonb_str1 =
                extract_jsonb_from_dictionary_result(ColumnarValue::Array(array.clone()), 0);
            let jsonb_str2 =
                extract_jsonb_from_dictionary_result(ColumnarValue::Array(array.clone()), 1);
            let jsonb_str3 =
                extract_jsonb_from_dictionary_result(ColumnarValue::Array(array.clone()), 2);
            let jsonb_str4 = extract_jsonb_from_dictionary_result(ColumnarValue::Array(array), 3);

            assert_eq!(jsonb_str1, r#"{"key1":"value1","key2":"value2"}"#);
            assert_eq!(jsonb_str2, r#"{"key3":"value3"}"#);
            assert_eq!(jsonb_str3, r#"{"key1":"value1","key2":"value2"}"#); // Same as first
            assert_eq!(jsonb_str4, r#"{"key4":"value4","key5":"value5"}"#);
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_dictionary_with_nulls() {
    let udf = PropertiesToJsonb::new();

    // Create a list array with null entry
    let key_field = Field::new("key", DataType::Utf8, false);
    let value_field = Field::new("value", DataType::Utf8, false);
    let struct_fields = Fields::from(vec![key_field, value_field]);

    let mut list_builder = ListBuilder::new(StructBuilder::new(
        struct_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(StringBuilder::new()),
        ],
    ));

    // Add valid properties
    let struct_builder = list_builder.values();
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("key1");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("value1");
    struct_builder.append(true);
    list_builder.append(true);

    // Add null
    list_builder.append(false);

    // Add more valid properties
    let struct_builder = list_builder.values();
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("key2");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("value2");
    struct_builder.append(true);
    list_builder.append(true);

    let properties_list = Arc::new(list_builder.finish());

    // Create dictionary array with nulls
    // Note: the keys reference indices in the list that was built: index 0, null, index 2
    let keys_array = Int32Array::from(vec![Some(0), None, Some(2)]); // null in middle
    let dict_array: DictionaryArray<Int32Type> =
        DictionaryArray::new(keys_array, properties_list.clone());

    let dict_array_ref: ArrayRef = Arc::new(dict_array);

    let args = ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(dict_array_ref)],
        arg_fields: vec![],
        return_field: Arc::new(Field::new(
            "result",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            true,
        )),
        number_rows: 3,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 3);
            let dict_array = array
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .unwrap();

            assert!(!dict_array.is_null(0));
            assert!(dict_array.is_null(1)); // Should be null
            assert!(!dict_array.is_null(2));

            let jsonb_str1 =
                extract_jsonb_from_dictionary_result(ColumnarValue::Array(array.clone()), 0);
            let jsonb_str3 = extract_jsonb_from_dictionary_result(ColumnarValue::Array(array), 2);

            assert_eq!(jsonb_str1, r#"{"key1":"value1"}"#);
            assert_eq!(jsonb_str3, r#"{"key2":"value2"}"#);
        }
        _ => panic!("Expected array result"),
    }
}

// New tests for properties_length with JSONB support

#[test]
fn test_properties_length_binary_jsonb() {
    let udf = PropertiesLength::new();

    // Create a properties array and convert to JSONB first
    let properties_array = create_test_properties_array(vec![
        vec![("key1", "value1"), ("key2", "value2")], // 2 properties
        vec![("key3", "value3")],                     // 1 property
        vec![],                                       // 0 properties
    ]);

    // Convert to JSONB first
    let jsonb_udf = PropertiesToJsonb::new();
    let jsonb_args = create_jsonb_scalar_args(properties_array, 3);
    let jsonb_result = jsonb_udf.invoke_with_args(jsonb_args).unwrap();

    // Now test properties_length on the dictionary JSONB result
    match jsonb_result {
        ColumnarValue::Array(jsonb_array) => {
            let args = create_length_scalar_args(jsonb_array, 3);
            let result = udf.invoke_with_args(args).unwrap();

            match result {
                ColumnarValue::Array(array) => {
                    assert_eq!(array.len(), 3);
                    let length_array = array.as_any().downcast_ref::<Int32Array>().unwrap();

                    assert_eq!(length_array.value(0), 2); // 2 properties
                    assert_eq!(length_array.value(1), 1); // 1 property
                    assert_eq!(length_array.value(2), 0); // 0 properties
                }
                _ => panic!("Expected array result"),
            }
        }
        _ => panic!("Expected JSONB array result"),
    }
}

#[test]
fn test_properties_length_binary_jsonb_with_nulls() {
    let udf = PropertiesLength::new();

    // Create properties array with null entry
    let key_field = Field::new("key", DataType::Utf8, false);
    let value_field = Field::new("value", DataType::Utf8, false);
    let struct_fields = Fields::from(vec![key_field, value_field]);

    let mut list_builder = ListBuilder::new(StructBuilder::new(
        struct_fields,
        vec![
            Box::new(StringBuilder::new()),
            Box::new(StringBuilder::new()),
        ],
    ));

    // Add valid properties
    let struct_builder = list_builder.values();
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("key1");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("value1");
    struct_builder.append(true);
    list_builder.append(true);

    // Add null
    list_builder.append(false);

    let properties_array = Arc::new(list_builder.finish());

    // Convert to JSONB first
    let jsonb_udf = PropertiesToJsonb::new();
    let jsonb_args = create_jsonb_scalar_args(properties_array, 2);
    let jsonb_result = jsonb_udf.invoke_with_args(jsonb_args).unwrap();

    // Now test properties_length on the dictionary JSONB result
    match jsonb_result {
        ColumnarValue::Array(jsonb_array) => {
            let args = create_length_scalar_args(jsonb_array, 2);
            let result = udf.invoke_with_args(args).unwrap();

            match result {
                ColumnarValue::Array(array) => {
                    assert_eq!(array.len(), 2);
                    let length_array = array.as_any().downcast_ref::<Int32Array>().unwrap();

                    assert_eq!(length_array.value(0), 1); // 1 property
                    assert!(length_array.is_null(1)); // null property list should result in null length
                }
                _ => panic!("Expected array result"),
            }
        }
        _ => panic!("Expected JSONB array result"),
    }
}

#[test]
fn test_properties_length_list_backward_compatibility() {
    let udf = PropertiesLength::new();

    // Test that properties_length still works with List<Struct> format
    let properties_array = create_test_properties_array(vec![
        vec![("key1", "value1"), ("key2", "value2"), ("key3", "value3")], // 3 properties
        vec![("key4", "value4")],                                         // 1 property
        vec![],                                                           // 0 properties
    ]);

    let args = create_length_scalar_args(properties_array, 3);
    let result = udf.invoke_with_args(args).unwrap();

    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 3);
            let length_array = array.as_any().downcast_ref::<Int32Array>().unwrap();

            assert_eq!(length_array.value(0), 3); // 3 properties
            assert_eq!(length_array.value(1), 1); // 1 property
            assert_eq!(length_array.value(2), 0); // 0 properties
        }
        _ => panic!("Expected array result"),
    }
}
