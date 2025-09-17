//! Property-related User Defined Functions (UDFs) for DataFusion
//!
//! This module contains all UDFs for working with properties in Micromegas:
//! - `PropertyGet`: Extract values from property lists
//! - `PropertiesToDict`: Convert properties to dictionary encoding for memory efficiency
//! - `PropertiesToArray`: Convert dictionary-encoded properties back to arrays
//! - `PropertiesLength`: Get the length of properties (supports both formats)

use datafusion::arrow::datatypes::{DataType, Field, Fields};
use std::sync::Arc;

pub mod dictionary_builder;
pub mod properties_to_dict_udf;
pub mod property_get;

/// Creates the standard properties field schema with dictionary encoding
/// Returns a Field with type Dictionary<Int32, List<Struct>>
pub fn properties_field_schema(field_name: &str) -> Field {
    Field::new(
        field_name,
        DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::List(Arc::new(Field::new(
                "Property",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, false),
                ])),
                false,
            )))),
        ),
        false,
    )
}
