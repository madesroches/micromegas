//! Property-related User Defined Functions (UDFs) for DataFusion
//!
//! This module contains all UDFs for working with properties in Micromegas:
//! - `PropertyGet`: Extract values from property lists
//! - `PropertiesToDict`: Convert properties to dictionary encoding for memory efficiency
//! - `PropertiesToArray`: Convert dictionary-encoded properties back to arrays
//! - `PropertiesLength`: Get the length of properties (supports both formats)

mod properties_to_dict_udf;
mod property_get;

pub use properties_to_dict_udf::{PropertiesLength, PropertiesToArray, PropertiesToDict};
pub use property_get::PropertyGet;
