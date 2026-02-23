use datafusion::arrow::array::{Array, BinaryDictionaryBuilder, DictionaryArray, StringArray};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::parse_value;
use micromegas_tracing::warn;
use std::any::Any;
use std::sync::Arc;

/// A scalar UDF that parses a JSON string into a JSONB value.
///
/// Accepts both Utf8 and Dictionary<Int32, Utf8> inputs.
/// Returns Dictionary<Int32, Binary> for memory efficiency.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbParse {
    signature: Signature,
}

impl JsonbParse {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbParse {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_json_to_jsonb(json_str: &str) -> Option<Vec<u8>> {
    match parse_value(json_str.as_bytes()) {
        Ok(parsed) => {
            let mut buffer = vec![];
            parsed.write_to_vec(&mut buffer);
            Some(buffer)
        }
        Err(e) => {
            warn!("error parsing json={json_str} error={e:?}");
            None
        }
    }
}

impl ScalarUDFImpl for JsonbParse {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_parse"
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
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_parse()");
        }

        match args[0].data_type() {
            DataType::Utf8 => {
                let string_array =
                    args[0]
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| {
                            DataFusionError::Internal("error casting to string array".into())
                        })?;

                let mut dict_builder = BinaryDictionaryBuilder::<Int32Type>::new();
                for i in 0..string_array.len() {
                    if string_array.is_null(i) {
                        dict_builder.append_null();
                    } else {
                        let json_str = string_array.value(i);
                        if let Some(jsonb_bytes) = parse_json_to_jsonb(json_str) {
                            dict_builder.append_value(&jsonb_bytes);
                        } else {
                            dict_builder.append_null();
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
            }
            DataType::Dictionary(_, value_type)
                if matches!(value_type.as_ref(), DataType::Utf8) =>
            {
                let dict_array = args[0]
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting dictionary array".into())
                    })?;

                let string_values = dict_array
                    .values()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("dictionary values are not a string array".into())
                    })?;

                let mut dict_builder = BinaryDictionaryBuilder::<Int32Type>::new();
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        dict_builder.append_null();
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < string_values.len() {
                            let json_str = string_values.value(key_index);
                            if let Some(jsonb_bytes) = parse_json_to_jsonb(json_str) {
                                dict_builder.append_value(&jsonb_bytes);
                            } else {
                                dict_builder.append_null();
                            }
                        } else {
                            return internal_err!(
                                "Dictionary key index out of bounds in jsonb_parse"
                            );
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
            }
            _ => internal_err!(
                "jsonb_parse: unsupported input type, expected Utf8 or Dictionary<Int32, Utf8>"
            ),
        }
    }
}

/// Creates a user-defined function to parse a JSON string into a JSONB value.
pub fn make_jsonb_parse_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbParse::new())
}
