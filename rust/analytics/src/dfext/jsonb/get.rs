use datafusion::arrow::array::{Array, GenericBinaryArray, GenericBinaryBuilder, StringArray};
use datafusion::{
    arrow::datatypes::DataType,
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
};
use jsonb::RawJsonb;
use std::sync::Arc;

fn jsonb_get(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 2 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to jsonb_get".into(),
        ));
    }
    let src_arrays = ColumnarValue::values_to_arrays(values)?;
    let jsonb_array: &GenericBinaryArray<i32> =
        src_arrays[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Execution("error casting jsonb as GenericBinaryArray".into())
        })?;
    let names: &StringArray = src_arrays[1].as_any().downcast_ref::<_>().ok_or_else(|| {
        DataFusionError::Execution("error casting second argument as StringArray".into())
    })?;
    let mut builder = GenericBinaryBuilder::<i32>::new();
    for index in 0..jsonb_array.len() {
        let src_buffer = jsonb_array.value(index);
        let name = names.value(index);
        let jsonb = RawJsonb::new(src_buffer);
        if let Some(value) = jsonb
            .get_by_name(name, true)
            .map_err(|e| DataFusionError::External(e.into()))?
        {
            builder.append_value(value.to_vec());
        } else {
            builder.append_null();
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

pub fn make_jsonb_get_udf() -> ScalarUDF {
    create_udf(
        "jsonb_get",
        vec![DataType::Binary, DataType::Utf8],
        DataType::Binary,
        Volatility::Immutable,
        Arc::new(&jsonb_get),
    )
}
