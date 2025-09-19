use crate::property_set::PropertySet;
use anyhow::{Context, Result};
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BinaryDictionaryBuilder, ListBuilder, StringBuilder, StructArray,
    StructBuilder,
};
use datafusion::arrow::datatypes::Int32Type;
use jsonb::Value;
use micromegas_telemetry::property::Property;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Reads a list of properties from an Arrow array.
///
/// The array is expected to be a `StructArray` with "key" and "value" fields.
pub fn read_property_list(value: ArrayRef) -> Result<Vec<Property>> {
    if value.is_empty() {
        return Ok(vec![]);
    }
    let properties: &StructArray = value
        .as_struct_opt()
        .with_context(|| format!("property list in not a struct array: {:?}", value.as_any()))?;
    let (key_index, _key_field) = properties
        .fields()
        .find("key")
        .with_context(|| "getting key field")?;
    let (value_index, _value_field) = properties
        .fields()
        .find("value")
        .with_context(|| "getting value field")?;
    let mut properties_vec = vec![];
    for i in 0..properties.len() {
        let key = properties.column(key_index).as_string::<i32>().value(i);
        let value = properties.column(value_index).as_string::<i32>().value(i);
        properties_vec.push(Property::new(Arc::new(key.into()), Arc::new(value.into())));
    }
    Ok(properties_vec)
}

/// Adds a set of properties from a `HashMap` to an Arrow list builder.
///
/// The properties are added as a new entry in the list builder.
pub fn add_properties_to_builder(
    properties: &HashMap<String, String>,
    property_list_builder: &mut ListBuilder<StructBuilder>,
) -> Result<()> {
    let properties_builder = property_list_builder.values();
    for (k, v) in properties.iter() {
        let key_builder = properties_builder
            .field_builder::<StringBuilder>(0)
            .with_context(|| "getting key field builder")?;
        key_builder.append_value(k);
        let value_builder = properties_builder
            .field_builder::<StringBuilder>(1)
            .with_context(|| "getting value field builder")?;
        value_builder.append_value(v);
        properties_builder.append(true);
    }
    property_list_builder.append(true);
    Ok(())
}

/// Adds a set of properties from a `PropertySet` to an Arrow list builder.
///
/// The properties are added as a new entry in the list builder.
pub fn add_property_set_to_builder(
    properties: &PropertySet,
    property_list_builder: &mut ListBuilder<StructBuilder>,
) -> Result<()> {
    let properties_builder = property_list_builder.values();
    properties.for_each_property(|prop| {
        let key_builder = properties_builder
            .field_builder::<StringBuilder>(0)
            .with_context(|| "getting key field builder")?;
        key_builder.append_value(prop.key_str());
        let value_builder = properties_builder
            .field_builder::<StringBuilder>(1)
            .with_context(|| "getting value field builder")?;
        value_builder.append_value(prop.value_str());
        properties_builder.append(true);
        Ok(())
    })?;
    property_list_builder.append(true);
    Ok(())
}

/// Adds a set of properties from a `HashMap` to a dictionary-encoded JSONB builder.
///
/// The properties are converted to JSONB format and added as a new entry in the dictionary builder.
pub fn add_properties_to_jsonb_builder(
    properties: &HashMap<String, String>,
    jsonb_builder: &mut BinaryDictionaryBuilder<Int32Type>,
) -> Result<()> {
    // Convert HashMap to BTreeMap for consistent ordering
    let btree_map: BTreeMap<String, Value> = properties
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(Cow::Borrowed(v))))
        .collect();

    let jsonb_value = Value::Object(btree_map);
    let mut jsonb_bytes = Vec::new();
    jsonb_value.write_to_vec(&mut jsonb_bytes);

    jsonb_builder.append_value(&jsonb_bytes);
    Ok(())
}

/// Adds a set of properties from a `PropertySet` to a dictionary-encoded JSONB builder.
///
/// The properties are converted to JSONB format and added as a new entry in the dictionary builder.
pub fn add_property_set_to_jsonb_builder(
    properties: &PropertySet,
    jsonb_builder: &mut BinaryDictionaryBuilder<Int32Type>,
) -> Result<()> {
    let mut btree_map = BTreeMap::new();

    properties.for_each_property(|prop| {
        btree_map.insert(
            prop.key_str().to_string(),
            Value::String(Cow::Owned(prop.value_str().to_string())),
        );
        Ok(())
    })?;

    let jsonb_value = Value::Object(btree_map);
    let mut jsonb_bytes = Vec::new();
    jsonb_value.write_to_vec(&mut jsonb_bytes);

    jsonb_builder.append_value(&jsonb_bytes);
    Ok(())
}
