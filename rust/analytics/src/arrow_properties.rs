use anyhow::{Context, Result};
use datafusion::arrow::array::{Array, ArrayRef, AsArray, StructArray};
use micromegas_telemetry::property::Property;
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
