//! Utility functions for converting between property formats

use anyhow::{Context, Result};
use jsonb::RawJsonb;
use std::collections::HashMap;

use crate::dfext::binary_column_accessor::BinaryColumnAccessor;

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

/// Extract properties from a binary column accessor at the given row index.
/// Returns an empty HashMap if the column value is null, otherwise deserializes the JSONB.
pub fn extract_properties_from_binary_column(
    column: &dyn BinaryColumnAccessor,
    row_index: usize,
) -> Result<HashMap<String, String>> {
    if column.is_null(row_index) {
        Ok(HashMap::new())
    } else {
        let jsonb_bytes = column.value(row_index);
        jsonb_to_property_map(jsonb_bytes)
    }
}
