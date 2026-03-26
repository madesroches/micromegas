use datafusion::arrow::array::{Array, DictionaryArray, GenericBinaryArray, Int64Array};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use std::any::Any;
use std::sync::Arc;

/// A scalar UDF that returns the number of elements in a JSONB array.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs.
/// Returns Int64 for arrays, NULL for non-array values.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbArrayLength {
    signature: Signature,
}

impl JsonbArrayLength {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbArrayLength {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_array_length_from_jsonb(jsonb_bytes: &[u8]) -> Result<Option<i64>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.array_length() {
        Ok(Some(len)) => Ok(Some(len as i64)),
        Ok(None) => Ok(None),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

impl ScalarUDFImpl for JsonbArrayLength {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_array_length"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Int64)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_array_length()");
        }

        match args[0].data_type() {
            DataType::Binary => {
                let binary_array = args[0]
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting to binary array".into())
                    })?;

                let mut builder = Int64Array::builder(binary_array.len());
                for i in 0..binary_array.len() {
                    if binary_array.is_null(i) {
                        builder.append_null();
                    } else {
                        let jsonb_bytes = binary_array.value(i);
                        if let Some(value) = extract_array_length_from_jsonb(jsonb_bytes)? {
                            builder.append_value(value);
                        } else {
                            builder.append_null();
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(builder.finish())))
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

                let mut builder = Int64Array::builder(dict_array.len());
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        builder.append_null();
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < binary_values.len() {
                            let jsonb_bytes = binary_values.value(key_index);
                            if let Some(value) = extract_array_length_from_jsonb(jsonb_bytes)? {
                                builder.append_value(value);
                            } else {
                                builder.append_null();
                            }
                        } else {
                            return internal_err!(
                                "Dictionary key index out of bounds in jsonb_array_length"
                            );
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(builder.finish())))
            }
            _ => internal_err!(
                "jsonb_array_length: unsupported input type, expected Binary or Dictionary<Int32, Binary>"
            ),
        }
    }
}

/// Creates a user-defined function to get the length of a JSONB array.
pub fn make_jsonb_array_length_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbArrayLength::new())
}
