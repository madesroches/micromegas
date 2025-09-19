//! Utility functions for converting between property formats

use anyhow::{Context, Result};
use datafusion::arrow::array::{Array, AsArray, DictionaryArray};
use datafusion::arrow::datatypes::Int32Type;
use jsonb::RawJsonb;
use std::collections::HashMap;

/// Convert JSONB bytes to a property HashMap
pub fn jsonb_to_property_map(jsonb_bytes: &[u8]) -> Result<HashMap<String, String>> {
    if jsonb_bytes.is_empty() {
        return Ok(HashMap::new());
    }

    let jsonb = RawJsonb::new(jsonb_bytes);
    let mut map = HashMap::new();

    // Use object_each to get key-value pairs
    if let Some(pairs) = jsonb
        .object_each()
        .with_context(|| "getting JSONB object pairs")?
    {
        for (key, value) in pairs {
            let value_str = if let Ok(Some(str_val)) = value.as_raw().as_str() {
                str_val.to_string()
            } else {
                value.as_raw().to_string()
            };
            map.insert(key, value_str);
        }
    }

    Ok(map)
}

/// Extract properties from a dictionary-encoded JSONB column at the given row index.
/// Returns an empty HashMap if the column value is null, otherwise deserializes the JSONB.
pub fn extract_properties_from_dict_column(
    column: &DictionaryArray<Int32Type>,
    row_index: usize,
) -> Result<HashMap<String, String>> {
    if column.is_null(row_index) {
        Ok(HashMap::new())
    } else {
        let key_index = column.keys().value(row_index) as usize;
        let values_array = column.values();
        let binary_array = values_array.as_binary::<i32>();
        let jsonb_bytes = binary_array.value(key_index);
        jsonb_to_property_map(jsonb_bytes)
    }
}
