use anyhow::Context;
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, DictionaryArray, GenericBinaryBuilder, GenericListArray, StructArray,
};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::Value;
use micromegas_tracing::warn;
use std::any::Any;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A scalar UDF that converts a list of properties to JSONB binary format.
///
/// Converts List<Struct<key: String, value: String>> to Binary (JSONB).
/// The output is a JSONB object in binary format like {"key1": "value1", "key2": "value2"}
#[derive(Debug)]
pub struct PropertiesToJsonb {
    signature: Signature,
}

impl PropertiesToJsonb {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for PropertiesToJsonb {
    fn default() -> Self {
        Self::new()
    }
}

fn convert_properties_list_to_jsonb(properties: ArrayRef) -> anyhow::Result<Vec<u8>> {
    let properties: &StructArray = properties.as_struct();
    let (key_index, _key_field) = properties
        .fields()
        .find("key")
        .with_context(|| "getting key field")?;
    let (value_index, _value_field) = properties
        .fields()
        .find("value")
        .with_context(|| "getting value field")?;

    let mut map = BTreeMap::new();
    let key_column = properties.column(key_index).as_string::<i32>();
    let value_column = properties.column(value_index).as_string::<i32>();

    for i in 0..properties.len() {
        if key_column.is_null(i) || value_column.is_null(i) {
            continue; // Skip null entries
        }
        let key = key_column.value(i);
        let value = value_column.value(i);
        map.insert(key.to_string(), Value::String(Cow::Borrowed(value)));
    }

    let jsonb_object = Value::Object(map);
    let mut buffer = Vec::new();
    jsonb_object.write_to_vec(&mut buffer);
    Ok(buffer)
}
impl ScalarUDFImpl for PropertiesToJsonb {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "properties_to_jsonb"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Binary)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to properties_to_jsonb()");
        }

        // Handle both regular arrays and dictionary arrays
        match args[0].data_type() {
            DataType::List(_) => {
                // Handle regular list array
                let prop_lists = args[0]
                    .as_any()
                    .downcast_ref::<GenericListArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting property list".into())
                    })?;

                let mut binary_builder = GenericBinaryBuilder::<i32>::new();
                for i in 0..prop_lists.len() {
                    if prop_lists.is_null(i) {
                        binary_builder.append_null();
                    } else {
                        match convert_properties_list_to_jsonb(prop_lists.value(i)) {
                            Ok(jsonb_bytes) => {
                                binary_builder.append_value(jsonb_bytes);
                            }
                            Err(e) => {
                                warn!(
                                    "error converting properties to JSONB at index {}: {:?}",
                                    i, e
                                );
                                binary_builder.append_null();
                            }
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(binary_builder.finish())))
            }
            DataType::Dictionary(_, value_type) => {
                // Handle dictionary array
                match value_type.as_ref() {
                    DataType::List(_) => {
                        let dict_array = args[0]
                            .as_any()
                            .downcast_ref::<DictionaryArray<Int32Type>>()
                            .ok_or_else(|| {
                                DataFusionError::Internal("error casting dictionary array".into())
                            })?;

                        let values_array = dict_array.values();
                        let list_values = values_array
                            .as_any()
                            .downcast_ref::<GenericListArray<i32>>()
                            .ok_or_else(|| {
                                DataFusionError::Internal(
                                    "dictionary values are not a list array".into(),
                                )
                            })?;

                        let mut binary_builder = GenericBinaryBuilder::<i32>::new();
                        for i in 0..dict_array.len() {
                            if dict_array.is_null(i) {
                                binary_builder.append_null();
                            } else {
                                let key_index = dict_array.keys().value(i) as usize;
                                if key_index < list_values.len() {
                                    let property_list = list_values.value(key_index);
                                    match convert_properties_list_to_jsonb(property_list) {
                                        Ok(jsonb_bytes) => {
                                            binary_builder.append_value(jsonb_bytes);
                                        }
                                        Err(e) => {
                                            warn!(
                                                "error converting properties to JSONB at dict index {}: {:?}",
                                                i, e
                                            );
                                            binary_builder.append_null();
                                        }
                                    }
                                } else {
                                    return internal_err!(
                                        "Dictionary key index out of bounds in properties_to_jsonb"
                                    );
                                }
                            }
                        }
                        Ok(ColumnarValue::Array(Arc::new(binary_builder.finish())))
                    }
                    _ => internal_err!("properties_to_jsonb: unsupported dictionary value type"),
                }
            }
            _ => internal_err!(
                "properties_to_jsonb: unsupported input type, expected List or Dictionary<Int32, List>"
            ),
        }
    }
}
