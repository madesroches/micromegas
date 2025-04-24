use anyhow::Context;
use datafusion::arrow::array::{as_string_array, ArrayRef, GenericListArray};
use datafusion::arrow::array::{Array, StringBuilder};
use datafusion::arrow::array::{AsArray, StructArray};
use datafusion::arrow::datatypes::{Field, Fields};
use datafusion::common::{internal_err, Result};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Volatility};
use datafusion::{arrow::datatypes::DataType, logical_expr::Signature};
use std::any::Any;
use std::sync::Arc;

#[derive(Debug)]
pub struct PropertyGet {
    signature: Signature,
}

impl PropertyGet {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![
                    DataType::List(Arc::new(Field::new(
                        "Property",
                        DataType::Struct(Fields::from(vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Utf8, false),
                        ])),
                        false,
                    ))),
                    DataType::Utf8,
                ],
                Volatility::Immutable,
            ),
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
        Ok(DataType::Utf8)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 2 {
            return internal_err!("wrong number of arguments to property_get()");
        }
        let prop_lists = args[0]
            .as_any()
            .downcast_ref::<GenericListArray<i32>>()
            .ok_or_else(|| DataFusionError::Internal("error casting property list".into()))?;
        let names = as_string_array(&args[1]);
        if prop_lists.len() != names.len() {
            return internal_err!("arrays of different lengths in property_get()");
        }
        let mut values = StringBuilder::new();
        for i in 0..prop_lists.len() {
            let name = names.value(i);
            if let Some(value) = find_property_in_list(prop_lists.value(i), name)
                .map_err(|e| DataFusionError::Internal(format!("{e:?}")))?
            {
                values.append_value(value);
            } else {
                values.append_null();
            }
        }

        Ok(ColumnarValue::Array(Arc::new(values.finish())))
    }
}
