//! Property-related User Defined Functions (UDFs) for DataFusion
//!
//! This module contains all UDFs for working with properties in Micromegas:
//! - `PropertyGet`: Extract values from property lists
//! - `PropertiesToDict`: Convert properties to dictionary encoding for memory efficiency
//! - `PropertiesToArray`: Convert dictionary-encoded properties back to arrays
//! - `PropertiesLength`: Get the length of properties (supports both formats)
//! - `PropertiesToJsonb`: Convert properties to JSONB binary format

pub mod properties_to_dict_udf;
pub mod properties_to_jsonb_udf;
pub mod property_get;
