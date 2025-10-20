use crate::dfext::binary_column_accessor::create_binary_accessor;
use datafusion::arrow::array::StringBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use std::any::Any;
use std::sync::Arc;

/// A scalar UDF that formats JSONB binary data as a JSON string.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs, making it compatible
/// with dictionary-encoded JSONB columns and the output of `properties_to_jsonb`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbFormatJson {
    signature: Signature,
}

impl JsonbFormatJson {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbFormatJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for JsonbFormatJson {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_format_json"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Utf8)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_format_json");
        }

        // Use BinaryColumnAccessor to handle both Binary and Dictionary<Int32, Binary>
        let binary_accessor = create_binary_accessor(&args[0])
            .map_err(|e| datafusion::error::DataFusionError::Execution(
                format!("Invalid input type for jsonb_format_json: {}. Expected Binary or Dictionary<Int32, Binary>", e)
            ))?;

        let mut builder = StringBuilder::with_capacity(binary_accessor.len(), 1024);

        for index in 0..binary_accessor.len() {
            if binary_accessor.is_null(index) {
                builder.append_null();
            } else {
                let src_buffer = binary_accessor.value(index);
                let jsonb = RawJsonb::new(src_buffer);
                builder.append_value(jsonb.to_string());
            }
        }

        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates a user-defined function to format a JSONB value as a JSON string.
///
/// This function accepts both `Binary` and `Dictionary<Int32, Binary>` inputs,
/// allowing it to work seamlessly with dictionary-encoded JSONB columns.
pub fn make_jsonb_format_json_udf() -> datafusion::logical_expr::ScalarUDF {
    datafusion::logical_expr::ScalarUDF::new_from_impl(JsonbFormatJson::new())
}
