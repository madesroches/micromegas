use datafusion::arrow::array::{
    Array, ArrayRef, GenericBinaryArray, ListBuilder, StringBuilder, StructBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields};
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
