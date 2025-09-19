use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericBinaryArray, Int32Array, ListBuilder, StringBuilder,
    StructBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type};
use datafusion::config::ConfigOptions;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl};
use jsonb::RawJsonb;
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

fn create_scalar_args(input: ArrayRef, num_rows: usize) -> ScalarFunctionArgs {
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
        return_field: Arc::new(Field::new("result", DataType::Binary, true)),
        number_rows: num_rows,
        config_options: Arc::new(ConfigOptions::default()),
    }
}

#[test]
fn test_empty_properties() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![vec![]]);
    let args = create_scalar_args(input, 1);

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 1);
            assert!(!array.is_null(0));

            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            // Should produce empty JSONB object {}
            let jsonb_str = RawJsonb::new(binary_array.value(0)).to_string();
            assert_eq!(jsonb_str, "{}");
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_single_property() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![vec![("key1", "value1")]]);
    let args = create_scalar_args(input, 1);

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 1);
            assert!(!array.is_null(0));

            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            // Should produce JSONB object {"key1": "value1"}
            let jsonb_str = RawJsonb::new(binary_array.value(0)).to_string();
            assert_eq!(jsonb_str, r#"{"key1":"value1"}"#);
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_multiple_properties() {
    let udf = PropertiesToJsonb::new();
    let input = create_test_properties_array(vec![
        vec![("key1", "value1"), ("key2", "value2")],
        vec![("key3", "value3")],
    ]);

    let args = ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(input)],
        arg_fields: vec![],
        return_field: Arc::new(Field::new("result", DataType::Binary, true)),
        number_rows: 2,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 2);
            assert!(!array.is_null(0));
            assert!(!array.is_null(1));

            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            // Should produce JSONB objects with multiple properties
            let jsonb_str1 = RawJsonb::new(binary_array.value(0)).to_string();
            let jsonb_str2 = RawJsonb::new(binary_array.value(1)).to_string();

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

    let args = ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(input)],
        arg_fields: vec![],
        return_field: Arc::new(Field::new("result", DataType::Binary, true)),
        number_rows: 1,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 1);
            assert!(!array.is_null(0));

            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            // Should handle special characters correctly in JSONB
            let jsonb_str = RawJsonb::new(binary_array.value(0)).to_string();
            // BTreeMap ensures sorted keys, and special characters should be properly escaped
            assert_eq!(
                jsonb_str,
                r#"{"key with spaces":"value\"with\"quotes","key🚀":"value\nwith\nnewlines"}"#
            );
        }
        _ => panic!("Expected array result"),
    }
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

    let args = ScalarFunctionArgs {
        args: vec![ColumnarValue::Array(input)],
        arg_fields: vec![],
        return_field: Arc::new(Field::new("result", DataType::Binary, true)),
        number_rows: 2,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 2);
            assert!(!array.is_null(0));
            assert!(array.is_null(1)); // null property list should result in null JSONB

            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            // First entry should be valid JSONB
            let jsonb_str = RawJsonb::new(binary_array.value(0)).to_string();
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
        return_field: Arc::new(Field::new("result", DataType::Binary, true)),
        number_rows: 4,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 4);
            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            // Verify each JSONB output
            let jsonb_str1 = RawJsonb::new(binary_array.value(0)).to_string();
            let jsonb_str2 = RawJsonb::new(binary_array.value(1)).to_string();
            let jsonb_str3 = RawJsonb::new(binary_array.value(2)).to_string();
            let jsonb_str4 = RawJsonb::new(binary_array.value(3)).to_string();

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
        return_field: Arc::new(Field::new("result", DataType::Binary, true)),
        number_rows: 3,
        config_options: Arc::new(ConfigOptions::default()),
    };

    let result = udf.invoke_with_args(args).unwrap();
    match result {
        ColumnarValue::Array(array) => {
            assert_eq!(array.len(), 3);
            assert!(!array.is_null(0));
            assert!(array.is_null(1)); // Should be null
            assert!(!array.is_null(2));

            let binary_array = array
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .unwrap();

            let jsonb_str1 = RawJsonb::new(binary_array.value(0)).to_string();
            let jsonb_str3 = RawJsonb::new(binary_array.value(2)).to_string();

            assert_eq!(jsonb_str1, r#"{"key1":"value1"}"#);
            assert_eq!(jsonb_str3, r#"{"key2":"value2"}"#);
        }
        _ => panic!("Expected array result"),
    }
}
