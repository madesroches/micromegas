use datafusion::arrow::array::{
    Array, DictionaryArray, GenericListArray, Int32Array, ListBuilder, StringBuilder, StructArray,
    StructBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type};
use datafusion::common::Result;
use datafusion::error::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

/// Builder for creating dictionary-encoded properties arrays.
/// This builder converts properties from List<Struct<key: Utf8, value: Utf8>>
/// to Dictionary<Int32, List<Struct<key: Utf8, value: Utf8>>> for memory efficiency.
pub struct PropertiesDictionaryBuilder {
    map: HashMap<Vec<(String, String)>, usize>,
    values_builder: ListBuilder<StructBuilder>,
    keys: Vec<Option<i32>>,
}

impl PropertiesDictionaryBuilder {
    /// Create a new PropertiesDictionaryBuilder with the specified capacity.
    pub fn new(capacity: usize) -> Self {
        let prop_struct_fields = vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ];
        let prop_field = Arc::new(Field::new(
            "Property",
            DataType::Struct(Fields::from(prop_struct_fields.clone())),
            false,
        ));
        let values_builder =
            ListBuilder::new(StructBuilder::from_fields(prop_struct_fields, capacity))
                .with_field(prop_field);

        Self {
            map: HashMap::new(),
            values_builder,
            keys: Vec::with_capacity(capacity),
        }
    }

    /// Append a property list from a StructArray.
    pub fn append_property_list(&mut self, struct_array: &StructArray) -> Result<()> {
        let prop_vec = extract_properties_as_vec(struct_array)?;

        match self.map.get(&prop_vec) {
            Some(&index) => {
                self.keys.push(Some(index as i32));
            }
            None => {
                let new_index = self.map.len();
                self.add_to_values(&prop_vec)?;
                self.map.insert(prop_vec, new_index);
                self.keys.push(Some(new_index as i32));
            }
        }
        Ok(())
    }

    /// Append a null value.
    pub fn append_null(&mut self) {
        self.keys.push(None);
    }

    fn add_to_values(&mut self, properties: &[(String, String)]) -> Result<()> {
        let struct_builder = self.values_builder.values();
        for (key, value) in properties {
            struct_builder
                .field_builder::<StringBuilder>(0)
                .ok_or_else(|| DataFusionError::Internal("Failed to get key builder".to_string()))?
                .append_value(key);
            struct_builder
                .field_builder::<StringBuilder>(1)
                .ok_or_else(|| {
                    DataFusionError::Internal("Failed to get value builder".to_string())
                })?
                .append_value(value);
            struct_builder.append(true);
        }
        self.values_builder.append(true);
        Ok(())
    }

    /// Finish building and return the DictionaryArray.
    pub fn finish(mut self) -> Result<DictionaryArray<Int32Type>> {
        let keys = Int32Array::from(self.keys);
        let values = Arc::new(self.values_builder.finish());
        DictionaryArray::try_new(keys, values)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))
    }
}

fn extract_properties_as_vec(struct_array: &StructArray) -> Result<Vec<(String, String)>> {
    use datafusion::arrow::array::AsArray;

    let mut properties = Vec::with_capacity(struct_array.len());
    let key_array = struct_array.column(0).as_string::<i32>();
    let value_array = struct_array.column(1).as_string::<i32>();
    for i in 0..struct_array.len() {
        if struct_array.is_valid(i) {
            let key = key_array.value(i).to_string();
            let value = value_array.value(i).to_string();
            properties.push((key, value));
        }
    }

    Ok(properties)
}

/// Build a dictionary-encoded array from a properties list array.
///
/// This function converts a GenericListArray containing property structs
/// into a dictionary-encoded array for improved memory efficiency and query performance.
pub fn build_dictionary_from_properties_array(
    list_array: &GenericListArray<i32>,
) -> Result<DictionaryArray<Int32Type>> {
    use datafusion::arrow::array::AsArray;

    let mut builder = PropertiesDictionaryBuilder::new(list_array.len());
    for i in 0..list_array.len() {
        if list_array.is_null(i) {
            builder.append_null();
        } else {
            let start = list_array.value_offsets()[i] as usize;
            let end = list_array.value_offsets()[i + 1] as usize;
            let sliced_values = list_array.values().slice(start, end - start);
            let struct_array = sliced_values.as_struct();
            builder.append_property_list(struct_array)?;
        }
    }

    builder.finish()
}
