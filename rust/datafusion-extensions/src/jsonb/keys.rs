use crate::jsonb::list_udfs::{binary_accessor_or_err, binary_dict, dict_fast_path};
use datafusion::arrow::array::{ArrayRef, ListBuilder, StringBuilder};
use datafusion::arrow::datatypes::{DataType, Field};
use datafusion::common::{Result, exec_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use std::sync::Arc;

/// A scalar UDF that extracts the keys from a JSONB object.
///
/// Accepts both `Binary` and `Dictionary<Int32, Binary>` inputs.
/// Returns `List<Utf8>` containing the object keys, or null if input is not an object.
/// A `Dictionary<Int32, Binary>` input takes a fast path that extracts each unique blob once
/// (`take`-expanded to row count), since JSONB values (especially `properties`) are often
/// repeated; the returned list is never dictionary-encoded — `unnest()` and `SELECT DISTINCT`
/// need a plain list to work directly, without an `arrow_cast` workaround.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbObjectKeys {
    signature: Signature,
}

impl JsonbObjectKeys {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbObjectKeys {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_keys_from_jsonb(jsonb_bytes: &[u8]) -> Result<Option<Vec<String>>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.object_keys() {
        Ok(Some(keys_jsonb)) => {
            // keys_jsonb is a JSONB array of string keys
            let keys_raw = keys_jsonb.as_raw();
            match keys_raw.array_values() {
                Ok(Some(values)) => {
                    let mut keys = Vec::with_capacity(values.len());
                    for value in values {
                        let raw = value.as_raw();
                        match raw.as_str() {
                            Ok(Some(s)) => keys.push(s.to_string()),
                            Ok(None) => {
                                // Key is not a string (shouldn't happen for object keys)
                                return Ok(None);
                            }
                            Err(e) => return Err(DataFusionError::External(e.into())),
                        }
                    }
                    Ok(Some(keys))
                }
                Ok(None) => Ok(Some(Vec::new())), // Empty array
                Err(e) => Err(DataFusionError::External(e.into())),
            }
        }
        Ok(None) => Ok(None), // Input is not an object
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

impl ScalarUDFImpl for JsonbObjectKeys {
    fn name(&self) -> &str {
        "jsonb_object_keys"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::List(Arc::new(Field::new_list_field(
            DataType::Utf8,
            true,
        ))))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return exec_err!("wrong number of arguments to jsonb_object_keys()");
        }

        Ok(ColumnarValue::Array(build_keys_list(&args[0])?))
    }
}

fn append_keys_slot(list_builder: &mut ListBuilder<StringBuilder>, keys: Option<Vec<String>>) {
    match keys {
        Some(keys) => {
            for key in keys {
                list_builder.values().append_value(key);
            }
            list_builder.append(true);
        }
        None => list_builder.append(false),
    }
}

/// Build `jsonb_object_keys`'s list array: dictionary fast path when the input is
/// `Dictionary<Int32, Binary>`, row-by-row via `create_binary_accessor` otherwise.
fn build_keys_list(array: &ArrayRef) -> Result<ArrayRef> {
    if let Some(dict_array) = binary_dict(array) {
        let list_builder = ListBuilder::new(StringBuilder::new());
        return dict_fast_path(dict_array, list_builder, |builder, bytes| {
            append_keys_slot(builder, extract_keys_from_jsonb(bytes)?);
            Ok(())
        });
    }

    let accessor = binary_accessor_or_err(array, "jsonb_object_keys")?;
    let mut list_builder = ListBuilder::new(StringBuilder::new());
    for i in 0..accessor.len() {
        if accessor.is_null(i) {
            list_builder.append(false);
        } else {
            append_keys_slot(
                &mut list_builder,
                extract_keys_from_jsonb(accessor.value(i))?,
            );
        }
    }
    Ok(Arc::new(list_builder.finish()))
}

/// Creates a user-defined function to extract the keys from a JSONB object.
pub fn make_jsonb_object_keys_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbObjectKeys::new())
}
