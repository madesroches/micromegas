use anyhow::{Result, anyhow};
use datafusion::arrow::array::{Array, ArrayRef, GenericListArray, RecordBatch};
use datafusion::arrow::datatypes::DataType;
use std::sync::Arc;

use crate::arrow_properties::{read_property_list, serialize_properties_to_jsonb};
use crate::dfext::binary_column_accessor::{BinaryColumnAccessor, create_binary_accessor};
use std::collections::HashMap;

/// Trait for accessing properties columns in a format-agnostic way.
///
/// This trait provides unified access to properties data regardless of the underlying
/// Arrow format - either JSONB dictionary format (`Dictionary(Int32, Binary)`) or
/// legacy struct array format (`GenericListArray<StructArray>`).
///
/// All methods return JSONB bytes for consistent processing downstream.
pub trait PropertiesColumnAccessor: Send + std::fmt::Debug {
    /// Get JSONB bytes for properties at the given index.
    ///
    /// For JSONB format: Returns the raw JSONB bytes directly.
    /// For struct array format: Converts struct array to JSONB on-the-fly.
    fn jsonb_value(&self, index: usize) -> Result<Vec<u8>>;

    /// Get the number of rows in this column.
    fn len(&self) -> usize;

    /// Check if the value at the given index is null.
    fn is_null(&self, index: usize) -> bool;

    /// Check if this column is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Accessor for JSONB format properties (both dictionary-encoded and plain binary)
struct JsonbColumnAccessor {
    binary_accessor: Box<dyn BinaryColumnAccessor + Send>,
}

impl JsonbColumnAccessor {
    fn new(binary_accessor: Box<dyn BinaryColumnAccessor + Send>) -> Self {
        Self { binary_accessor }
    }
}

impl PropertiesColumnAccessor for JsonbColumnAccessor {
    fn jsonb_value(&self, index: usize) -> Result<Vec<u8>> {
        // For JSONB format, return the raw bytes directly
        Ok(self.binary_accessor.value(index).to_vec())
    }

    fn len(&self) -> usize {
        self.binary_accessor.len()
    }

    fn is_null(&self, index: usize) -> bool {
        self.binary_accessor.is_null(index)
    }
}

impl std::fmt::Debug for JsonbColumnAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonbColumnAccessor")
            .field("len", &self.binary_accessor.len())
            .finish()
    }
}

/// Accessor for legacy struct array format: `GenericListArray<StructArray>`
#[derive(Debug)]
struct StructArrayAccessor {
    array: Arc<GenericListArray<i32>>,
}

impl StructArrayAccessor {
    fn new(array: Arc<GenericListArray<i32>>) -> Self {
        Self { array }
    }
}

impl PropertiesColumnAccessor for StructArrayAccessor {
    fn jsonb_value(&self, index: usize) -> Result<Vec<u8>> {
        // Convert struct array to PropertySet, then to HashMap, then to JSONB
        let property_list_array = self.array.value(index);
        let properties = read_property_list(property_list_array)?;

        // Convert Vec<Property> to HashMap<String, String>
        let properties_map: HashMap<String, String> = properties
            .into_iter()
            .map(|prop| (prop.key_str().to_string(), prop.value_str().to_string()))
            .collect();

        // Serialize to JSONB bytes
        serialize_properties_to_jsonb(&properties_map)
    }

    fn len(&self) -> usize {
        self.array.len()
    }

    fn is_null(&self, index: usize) -> bool {
        self.array.is_null(index)
    }
}

/// Creates a properties column accessor that automatically detects the format.
///
/// Supports:
/// - JSONB dictionary format: `Dictionary(Int32, Binary)` - used by current analytics tables
/// - Plain binary format: `Binary` - JSONB data without dictionary encoding
/// - Struct array format: `GenericListArray<StructArray>` - legacy format from replication
///
/// Returns a unified accessor that always provides JSONB bytes.
pub fn create_properties_accessor(
    array: &ArrayRef,
) -> Result<Box<dyn PropertiesColumnAccessor + Send>> {
    match array.data_type() {
        // Modern JSONB dictionary format
        DataType::Dictionary(key_type, value_type) => {
            if matches!(key_type.as_ref(), DataType::Int32)
                && matches!(value_type.as_ref(), DataType::Binary)
            {
                let binary_accessor = create_binary_accessor(array)?;
                Ok(Box::new(JsonbColumnAccessor::new(binary_accessor)))
            } else {
                Err(anyhow!(
                    "Unsupported dictionary format for properties: key={:?}, value={:?}",
                    key_type,
                    value_type
                ))
            }
        }

        // Legacy struct array format
        DataType::List(field) => {
            // Verify this is a list of structs with key/value fields
            if let DataType::Struct(struct_fields) = field.data_type() {
                let has_key = struct_fields.iter().any(|f| f.name() == "key");
                let has_value = struct_fields.iter().any(|f| f.name() == "value");

                if has_key && has_value {
                    let list_array = array
                        .as_any()
                        .downcast_ref::<GenericListArray<i32>>()
                        .ok_or_else(|| anyhow!("Failed to downcast to GenericListArray<i32>"))?
                        .clone();
                    Ok(Box::new(StructArrayAccessor::new(Arc::new(list_array))))
                } else {
                    Err(anyhow!(
                        "List array does not contain struct with key/value fields"
                    ))
                }
            } else {
                Err(anyhow!(
                    "List array does not contain struct elements: {:?}",
                    field.data_type()
                ))
            }
        }

        // Direct binary format (less common, but possible)
        DataType::Binary => {
            let binary_accessor = create_binary_accessor(array)?;
            Ok(Box::new(JsonbColumnAccessor::new(binary_accessor)))
        }

        _ => Err(anyhow!(
            "Unsupported array type for properties accessor: {:?}",
            array.data_type()
        )),
    }
}

/// Convenience function to get a properties column accessor by name from a RecordBatch.
pub fn properties_column_by_name(
    batch: &RecordBatch,
    name: &str,
) -> Result<Box<dyn PropertiesColumnAccessor + Send>> {
    let column = batch
        .column_by_name(name)
        .ok_or_else(|| anyhow!("Column '{}' not found", name))?;
    create_properties_accessor(column)
}
