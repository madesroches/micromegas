use datafusion::arrow::array::{
    Array, ArrayRef, DictionaryArray, GenericBinaryArray, Int32Array, ListBuilder, StringBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// A scalar UDF that extracts the keys from a JSONB object.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs.
/// Returns Dictionary<Int32, List<Utf8>> containing the object keys, or null if input is not an object.
/// Dictionary encoding is used because JSONB values (especially properties) are often repeated.
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
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_object_keys"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::List(Arc::new(Field::new_list_field(
                DataType::Utf8,
                true,
            )))),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_object_keys()");
        }

        match args[0].data_type() {
            DataType::Binary => {
                let binary_array = args[0]
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting to binary array".into())
                    })?;

                let result = build_dict_list_array(binary_array.len(), |i| {
                    if binary_array.is_null(i) {
                        Ok(None)
                    } else {
                        extract_keys_from_jsonb(binary_array.value(i))
                    }
                })?;
                Ok(ColumnarValue::Array(result))
            }
            DataType::Dictionary(_, value_type)
                if matches!(value_type.as_ref(), DataType::Binary) =>
            {
                let dict_array = args[0]
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting dictionary array".into())
                    })?;

                let binary_values = dict_array
                    .values()
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("dictionary values are not a binary array".into())
                    })?;

                let result = build_dict_list_array(dict_array.len(), |i| {
                    if dict_array.is_null(i) {
                        Ok(None)
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < binary_values.len() {
                            extract_keys_from_jsonb(binary_values.value(key_index))
                        } else {
                            internal_err!("Dictionary key index out of bounds in jsonb_object_keys")
                        }
                    }
                })?;
                Ok(ColumnarValue::Array(result))
            }
            _ => internal_err!(
                "jsonb_object_keys: unsupported input type, expected Binary or Dictionary<Int32, Binary>"
            ),
        }
    }
}

/// Build a Dictionary<Int32, List<Utf8>> array from a function that returns keys for each index.
/// Uses a HashMap to deduplicate identical key lists for memory efficiency.
fn build_dict_list_array<F>(len: usize, mut get_keys: F) -> Result<ArrayRef>
where
    F: FnMut(usize) -> Result<Option<Vec<String>>>,
{
    // Map from key list to dictionary index
    let mut unique_lists: HashMap<Option<Vec<String>>, i32> = HashMap::new();
    let mut key_indices: Vec<Option<i32>> = Vec::with_capacity(len);
    let mut ordered_lists: Vec<Option<Vec<String>>> = Vec::new();

    // First pass: collect all values and deduplicate
    for i in 0..len {
        let keys = get_keys(i)?;
        if let Some(idx) = unique_lists.get(&keys) {
            key_indices.push(Some(*idx));
        } else {
            let idx = ordered_lists.len() as i32;
            unique_lists.insert(keys.clone(), idx);
            key_indices.push(Some(idx));
            ordered_lists.push(keys);
        }
    }

    // Build the values array (List<Utf8>) from unique lists
    let mut list_builder = ListBuilder::new(StringBuilder::new());
    for list_opt in &ordered_lists {
        match list_opt {
            Some(keys) => {
                for key in keys {
                    list_builder.values().append_value(key);
                }
                list_builder.append(true);
            }
            None => {
                list_builder.append_null();
            }
        }
    }
    let values_array = Arc::new(list_builder.finish());

    // Build the keys array
    let keys_array = Int32Array::from(key_indices);

    // Construct the dictionary array
    let dict_array =
        DictionaryArray::<Int32Type>::try_new(keys_array, values_array).map_err(|e| {
            DataFusionError::Internal(format!("Failed to create dictionary array: {e}"))
        })?;

    Ok(Arc::new(dict_array))
}

/// Creates a user-defined function to extract the keys from a JSONB object.
pub fn make_jsonb_object_keys_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbObjectKeys::new())
}
