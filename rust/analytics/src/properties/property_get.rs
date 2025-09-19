use anyhow::Context;
use datafusion::arrow::array::{Array, StringDictionaryBuilder};
use datafusion::arrow::array::{ArrayRef, DictionaryArray, GenericListArray, StringArray};
use datafusion::arrow::array::{AsArray, StructArray};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

/// A scalar UDF that retrieves a property from a list of properties.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct PropertyGet {
    signature: Signature,
}

impl PropertyGet {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}

impl Default for PropertyGet {
    fn default() -> Self {
        Self::new()
    }
}

fn find_property_in_list(properties: ArrayRef, name: &str) -> anyhow::Result<Option<String>> {
    let properties: &StructArray = properties.as_struct();
    let (key_index, _key_field) = properties
        .fields()
        .find("key")
        .with_context(|| "getting key field")?;
    let (value_index, _value_field) = properties
        .fields()
        .find("value")
        .with_context(|| "getting value field")?;
    for i in 0..properties.len() {
        let key = properties.column(key_index).as_string::<i32>().value(i);
        if key.eq_ignore_ascii_case(name) {
            let value = properties.column(value_index).as_string::<i32>().value(i);
            return Ok(Some(value.into()));
        }
    }
    Ok(None)
}

impl ScalarUDFImpl for PropertyGet {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn name(&self) -> &str {
        "property_get"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        ))
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 2 {
            return internal_err!("wrong number of arguments to property_get()");
        }

        let names = args[1]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| DataFusionError::Execution("downcasting names in PropertyGet".into()))?;

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

                if prop_lists.len() != names.len() {
                    return internal_err!("arrays of different lengths in property_get()");
                }

                let mut dict_builder = StringDictionaryBuilder::<Int32Type>::new();
                for i in 0..prop_lists.len() {
                    let name = names.value(i);
                    if let Some(value) = find_property_in_list(prop_lists.value(i), name)
                        .map_err(|e| DataFusionError::Internal(format!("{e:?}")))?
                    {
                        dict_builder.append_value(value);
                    } else {
                        dict_builder.append_null();
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
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

                        if dict_array.len() != names.len() {
                            return internal_err!("arrays of different lengths in property_get()");
                        }

                        let values_array = dict_array.values();
                        let list_values = values_array
                            .as_any()
                            .downcast_ref::<GenericListArray<i32>>()
                            .ok_or_else(|| {
                                DataFusionError::Internal(
                                    "dictionary values are not a list array".into(),
                                )
                            })?;

                        let mut dict_builder = StringDictionaryBuilder::<Int32Type>::new();
                        for i in 0..dict_array.len() {
                            let name = names.value(i);

                            if dict_array.is_null(i) {
                                dict_builder.append_null();
                            } else {
                                let key_index = dict_array.keys().value(i) as usize;
                                if key_index < list_values.len() {
                                    let property_list = list_values.value(key_index);
                                    if let Some(value) = find_property_in_list(property_list, name)
                                        .map_err(|e| DataFusionError::Internal(format!("{e:?}")))?
                                    {
                                        dict_builder.append_value(value);
                                    } else {
                                        dict_builder.append_null();
                                    }
                                } else {
                                    return internal_err!(
                                        "Dictionary key index out of bounds in property_get"
                                    );
                                }
                            }
                        }
                        Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
                    }
                    _ => internal_err!("property_get: unsupported dictionary value type"),
                }
            }
            _ => internal_err!(
                "property_get: unsupported input type, expected List or Dictionary<Int32, List>"
            ),
        }
    }
}
