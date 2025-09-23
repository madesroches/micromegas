use anyhow::Result;
use datafusion::arrow::array::Array;
use micromegas_analytics::properties::property_set::PropertySet;
use micromegas_analytics::properties::property_set_jsonb_dictionary_builder::PropertySetJsonbDictionaryBuilder;
use micromegas_transit::value::{Object, Value};
use std::sync::Arc;

fn create_test_property_set(props: Vec<(&str, &str)>) -> PropertySet {
    let members = props
        .into_iter()
        .map(|(k, v)| {
            (
                Arc::new(k.to_string()),
                Value::String(Arc::new(v.to_string())),
            )
        })
        .collect();

    let obj = Arc::new(Object {
        type_name: Arc::new("TestPropertySet".to_string()),
        members,
    });

    PropertySet::new(obj)
}

#[test]
fn test_empty_builder() {
    let builder = PropertySetJsonbDictionaryBuilder::new(0);
    assert!(builder.is_empty());
    assert_eq!(builder.len(), 0);
}

#[test]
fn test_single_property_set() -> Result<()> {
    let mut builder = PropertySetJsonbDictionaryBuilder::new(1);
    let props = create_test_property_set(vec![("key1", "value1")]);

    builder.append_property_set(&props)?;

    let dict_array = builder.finish()?;
    assert_eq!(dict_array.len(), 1);
    assert_eq!(dict_array.values().len(), 1); // One unique value in dictionary

    Ok(())
}

#[test]
fn test_duplicate_property_sets() -> Result<()> {
    let mut builder = PropertySetJsonbDictionaryBuilder::new(3);
    let props1 = create_test_property_set(vec![("key1", "value1")]);
    let props2 = create_test_property_set(vec![("key2", "value2")]);
    let props1_again = props1.clone(); // Same Arc<Object> pointer

    builder.append_property_set(&props1)?;
    builder.append_property_set(&props2)?;
    builder.append_property_set(&props1_again)?; // Should reuse dictionary index

    let dict_array = builder.finish()?;
    assert_eq!(dict_array.len(), 3); // Three entries
    assert_eq!(dict_array.values().len(), 2); // Two unique values in dictionary

    // Verify the keys point to correct dictionary indices
    let keys = dict_array.keys();
    assert_eq!(keys.value(0), 0); // First props1
    assert_eq!(keys.value(1), 1); // props2
    assert_eq!(keys.value(2), 0); // Second props1 (reused index)

    Ok(())
}

#[test]
fn test_null_values() -> Result<()> {
    let mut builder = PropertySetJsonbDictionaryBuilder::new(2);
    let props = create_test_property_set(vec![("key1", "value1")]);

    builder.append_null();
    builder.append_property_set(&props)?;

    let dict_array = builder.finish()?;
    assert_eq!(dict_array.len(), 2);

    let keys = dict_array.keys();
    assert!(keys.is_null(0)); // First entry is null
    assert!(!keys.is_null(1)); // Second entry is not null
    assert_eq!(keys.value(1), 0); // Points to first dictionary entry

    Ok(())
}

#[test]
fn test_pointer_based_deduplication() -> Result<()> {
    let mut builder = PropertySetJsonbDictionaryBuilder::new(4);

    // Create two PropertySets with same content but different Arc<Object> instances
    let props1 = create_test_property_set(vec![("key1", "value1")]);
    let props2 = create_test_property_set(vec![("key1", "value1")]); // Same content, different Arc

    builder.append_property_set(&props1)?;
    builder.append_property_set(&props2)?;
    builder.append_property_set(&props1)?; // Same Arc as first
    builder.append_property_set(&props2)?; // Same Arc as second

    let dict_array = builder.finish()?;
    assert_eq!(dict_array.len(), 4); // Four entries
    assert_eq!(dict_array.values().len(), 2); // Two unique Arc pointers = two dictionary values

    let keys = dict_array.keys();
    assert_eq!(keys.value(0), 0); // First props1
    assert_eq!(keys.value(1), 1); // First props2 (different Arc)
    assert_eq!(keys.value(2), 0); // Second props1 (same Arc as first)
    assert_eq!(keys.value(3), 1); // Second props2 (same Arc as first props2)

    Ok(())
}
