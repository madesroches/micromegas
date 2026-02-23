use datafusion::arrow::array::{
    Array, BinaryDictionaryBuilder, DictionaryArray, GenericBinaryArray, StringArray,
};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use std::any::Any;
use std::sync::Arc;

/// A scalar UDF that retrieves a value from a JSONB object by name.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs.
/// Returns Dictionary<Int32, Binary> for memory efficiency.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbGet {
    signature: Signature,
}

impl JsonbGet {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}

impl Default for JsonbGet {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_jsonb_value(jsonb_bytes: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.get_by_name(name, true) {
        Ok(Some(value)) => Ok(Some(value.to_vec())),
        Ok(None) => Ok(None),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

impl ScalarUDFImpl for JsonbGet {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_get"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Binary),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 2 {
            return internal_err!("wrong number of arguments to jsonb_get()");
        }

        let names = args[1]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Execution("second argument must be a string array".into())
            })?;

        match args[0].data_type() {
            DataType::Binary => {
                // Handle plain Binary JSONB array
                let binary_array = args[0]
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting to binary array".into())
                    })?;

                if binary_array.len() != names.len() {
                    return internal_err!("arrays of different lengths in jsonb_get()");
                }

                let mut dict_builder = BinaryDictionaryBuilder::<Int32Type>::new();
                for i in 0..binary_array.len() {
                    if binary_array.is_null(i) {
                        dict_builder.append_null();
                    } else {
                        let jsonb_bytes = binary_array.value(i);
                        let name = names.value(i);
                        if let Some(value) = extract_jsonb_value(jsonb_bytes, name)? {
                            dict_builder.append_value(&value);
                        } else {
                            dict_builder.append_null();
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
            }
            DataType::Dictionary(_, value_type)
                if matches!(value_type.as_ref(), DataType::Binary) =>
            {
                // Handle dictionary-encoded JSONB array
                let dict_array = args[0]
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting dictionary array".into())
                    })?;

                if dict_array.len() != names.len() {
                    return internal_err!("arrays of different lengths in jsonb_get()");
                }

                let binary_values = dict_array
                    .values()
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("dictionary values are not a binary array".into())
                    })?;

                let mut dict_builder = BinaryDictionaryBuilder::<Int32Type>::new();
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        dict_builder.append_null();
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < binary_values.len() {
                            let jsonb_bytes = binary_values.value(key_index);
                            let name = names.value(i);
                            if let Some(value) = extract_jsonb_value(jsonb_bytes, name)? {
                                dict_builder.append_value(&value);
                            } else {
                                dict_builder.append_null();
                            }
                        } else {
                            return internal_err!(
                                "Dictionary key index out of bounds in jsonb_get"
                            );
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
            }
            _ => internal_err!(
                "jsonb_get: unsupported input type, expected Binary or Dictionary<Int32, Binary>"
            ),
        }
    }
}

/// Creates a user-defined function to get a value from a JSONB object by name.
pub fn make_jsonb_get_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbGet::new())
}
