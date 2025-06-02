use crate::property_set::PropertySet;
use anyhow::{Context, Result};
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, ListBuilder, StringBuilder, StructArray, StructBuilder,
};
use micromegas_telemetry::property::Property;
use std::collections::HashMap;
use std::sync::Arc;

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
