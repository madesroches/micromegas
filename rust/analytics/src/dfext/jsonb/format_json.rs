use datafusion::arrow::array::{Array, GenericBinaryArray, StringBuilder};
use datafusion::{
    arrow::datatypes::DataType,
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
};
use jsonb::RawJsonb;
use std::sync::Arc;

fn jsonb_format_json(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to jsonb_format_json".into(),
        ));
    }
    let src_arrays = ColumnarValue::values_to_arrays(values)?;
    let jsonb_array: &GenericBinaryArray<i32> =
        src_arrays[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Execution("error casting jsonb as GenericBinaryArray".into())
        })?;
    let mut builder = StringBuilder::with_capacity(jsonb_array.len(), 1024);
    for index in 0..jsonb_array.len() {
        let src_buffer = jsonb_array.value(index);
        let jsonb = RawJsonb::new(src_buffer);
        builder.append_value(jsonb.to_string());
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

pub fn make_jsonb_format_json_udf() -> ScalarUDF {
    create_udf(
        "jsonb_format_json",
        vec![DataType::Binary],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(&jsonb_format_json),
    )
}
