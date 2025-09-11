use datafusion::arrow::array::{Array, GenericListArray, ListArray, StringBuilder, StructBuilder};
use datafusion::arrow::buffer::OffsetBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Fields};
use micromegas_analytics::properties_to_dict_udf::build_dictionary_from_properties;
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
