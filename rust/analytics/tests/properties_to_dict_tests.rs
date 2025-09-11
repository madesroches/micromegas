use datafusion::arrow::array::{
    Array, GenericListArray, Int32Array, ListArray, StringBuilder, StructBuilder,
};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Fields};
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl};
use micromegas_analytics::properties_to_dict_udf::{
    PropertiesLength, build_dictionary_from_properties,
};
use std::sync::Arc;

fn create_test_properties() -> ListArray {
    let mut struct_builder = StructBuilder::from_fields(
        vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ],
        8,
    );

    // First property list: env=production, version=1.0.0
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("env");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("production");
    struct_builder.append(true);

    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("version");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("1.0.0");
    struct_builder.append(true);

    // Third property list: same as first (env=production, version=1.0.0)
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("env");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("production");
    struct_builder.append(true);

    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("version");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("1.0.0");
    struct_builder.append(true);

    let struct_array = Arc::new(struct_builder.finish());

    // Three lists: [0,2), [2,2), [2,4) - first has 2 items, second is empty, third has 2 items
    let offsets = OffsetBuffer::from_lengths([2, 0, 2]);
    ListArray::new(
        Arc::new(Field::new(
            "Property",
            DataType::Struct(Fields::from(vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ])),
            false,
        )),
        offsets,
        struct_array,
        None,
    )
}

fn create_scalar_function_args(args: Vec<ColumnarValue>) -> ScalarFunctionArgs {
    let num_rows = match &args[0] {
        ColumnarValue::Array(array) => array.len(),
        ColumnarValue::Scalar(_) => 1,
    };

    let arg_fields = args
        .iter()
        .enumerate()
        .map(|(i, arg)| {
            let data_type = match arg {
                ColumnarValue::Array(array) => array.data_type().clone(),
                ColumnarValue::Scalar(scalar) => scalar.data_type(),
            };
            Arc::new(Field::new(format!("arg_{}", i), data_type, true))
        })
        .collect();

    let return_field = Arc::new(Field::new("result", DataType::Int32, true));

    ScalarFunctionArgs {
        args,
        arg_fields,
        number_rows: num_rows,
        return_field,
    }
}

#[test]
fn test_properties_to_dict_basic() {
    let properties = create_test_properties();
    let list_array = properties
        .as_any()
        .downcast_ref::<GenericListArray<i32>>()
        .unwrap();

    let result = build_dictionary_from_properties(list_array).expect("Should build dictionary");

    assert_eq!(result.len(), 3);

    let keys = result.keys();
    assert_eq!(keys.value(0), 0); // First properties list
    assert_eq!(keys.value(1), 1); // Empty list gets its own dictionary entry 
    assert_eq!(keys.value(2), 0); // Same properties as first
}

#[test]
fn test_dictionary_deduplication() {
    let properties = create_test_properties();
    let list_array = properties
        .as_any()
        .downcast_ref::<GenericListArray<i32>>()
        .unwrap();

    let result = build_dictionary_from_properties(list_array).expect("Should build dictionary");

    let values = result.values();
    let list_values = values
        .as_any()
        .downcast_ref::<GenericListArray<i32>>()
        .expect("Dictionary values should be a list array");

    // Should have 2 entries: one for properties list [env=production, version=1.0.0] and one for empty list
    assert_eq!(
        list_values.len(),
        2,
        "Should have 2 unique property sets: non-empty and empty"
    );
}

#[test]
fn test_properties_length_with_regular_array() {
    let properties = create_test_properties();
    let list_array = Arc::new(properties);

    let properties_length = PropertiesLength::new();
    let args = create_scalar_function_args(vec![ColumnarValue::Array(list_array.clone())]);

    let result = properties_length
        .invoke_with_args(args)
        .expect("Should get lengths");

    match result {
        ColumnarValue::Array(array) => {
            let int_array = array
                .as_any()
                .downcast_ref::<Int32Array>()
                .expect("Result should be Int32Array");

            // First list has 2 properties, second is empty (0), third has 2 properties
            assert_eq!(int_array.len(), 3);
            assert_eq!(int_array.value(0), 2);
            assert_eq!(int_array.value(1), 0);
            assert_eq!(int_array.value(2), 2);
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_properties_length_with_dictionary_array() {
    let properties = create_test_properties();
    let list_array = properties
        .as_any()
        .downcast_ref::<GenericListArray<i32>>()
        .unwrap();

    // First create dictionary
    let dict_array = build_dictionary_from_properties(list_array).expect("Should build dictionary");
    let dict_array_ref = Arc::new(dict_array);

    let properties_length = PropertiesLength::new();
    let args = create_scalar_function_args(vec![ColumnarValue::Array(dict_array_ref)]);

    let result = properties_length
        .invoke_with_args(args)
        .expect("Should get lengths");

    match result {
        ColumnarValue::Array(array) => {
            let int_array = array
                .as_any()
                .downcast_ref::<Int32Array>()
                .expect("Result should be Int32Array");

            // Same expected results as with regular array
            assert_eq!(int_array.len(), 3);
            assert_eq!(int_array.value(0), 2);
            assert_eq!(int_array.value(1), 0);
            assert_eq!(int_array.value(2), 2);
        }
        _ => panic!("Expected array result"),
    }
}

#[test]
fn test_properties_length_with_nulls() {
    // Create array with nulls
    let mut struct_builder = StructBuilder::from_fields(
        vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ],
        2,
    );

    // One property pair
    struct_builder
        .field_builder::<StringBuilder>(0)
        .unwrap()
        .append_value("test");
    struct_builder
        .field_builder::<StringBuilder>(1)
        .unwrap()
        .append_value("value");
    struct_builder.append(true);

    let struct_array = Arc::new(struct_builder.finish());

    // Two lists: [0,1) and null
    let offsets = OffsetBuffer::from_lengths([1, 0]);
    let null_buffer = datafusion::arrow::buffer::NullBuffer::from(vec![true, false]);

    let list_array = ListArray::new(
        Arc::new(Field::new(
            "Property",
            DataType::Struct(Fields::from(vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ])),
            false,
        )),
        offsets,
        struct_array,
        Some(null_buffer),
    );

    let properties_length = PropertiesLength::new();
    let args = create_scalar_function_args(vec![ColumnarValue::Array(Arc::new(list_array))]);

    let result = properties_length
        .invoke_with_args(args)
        .expect("Should get lengths");

    match result {
        ColumnarValue::Array(array) => {
            let int_array = array
                .as_any()
                .downcast_ref::<Int32Array>()
                .expect("Result should be Int32Array");

            assert_eq!(int_array.len(), 2);
            assert_eq!(int_array.value(0), 1); // First list has 1 property
            assert!(int_array.is_null(1)); // Second list is null
        }
        _ => panic!("Expected array result"),
    }
}
