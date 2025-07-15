use datafusion::arrow::array::{
    Array, Float64Array, GenericBinaryArray, Int64Array, StringBuilder,
};
use datafusion::{
    arrow::datatypes::DataType,
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
};
use jsonb::RawJsonb;
use std::sync::Arc;

fn jsonb_as_string(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to jsonb_as_string".into(),
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
        if let Some(value) = jsonb
            .as_str()
            .map_err(|e| DataFusionError::External(e.into()))?
        {
            builder.append_value(value);
        } else {
            builder.append_null();
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

pub fn make_jsonb_as_string_udf() -> ScalarUDF {
    create_udf(
        "jsonb_as_string",
        vec![DataType::Binary],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(&jsonb_as_string),
    )
}

fn jsonb_as_f64(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to jsonb_as_f64".into(),
        ));
    }
    let src_arrays = ColumnarValue::values_to_arrays(values)?;
    let jsonb_array: &GenericBinaryArray<i32> =
        src_arrays[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Execution("error casting jsonb as GenericBinaryArray".into())
        })?;
    let mut builder = Float64Array::builder(jsonb_array.len());
    for index in 0..jsonb_array.len() {
        let src_buffer = jsonb_array.value(index);
        let jsonb = RawJsonb::new(src_buffer);
        if let Some(value) = jsonb
            .as_f64()
            .map_err(|e| DataFusionError::External(e.into()))?
        {
            builder.append_value(value);
        } else {
            builder.append_null();
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

pub fn make_jsonb_as_f64_udf() -> ScalarUDF {
    create_udf(
        "jsonb_as_f64",
        vec![DataType::Binary],
        DataType::Float64,
        Volatility::Immutable,
        Arc::new(&jsonb_as_f64),
    )
}

fn jsonb_as_i64(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to jsonb_as_i64".into(),
        ));
    }
    let src_arrays = ColumnarValue::values_to_arrays(values)?;
    let jsonb_array: &GenericBinaryArray<i32> =
        src_arrays[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Execution("error casting jsonb as GenericBinaryArray".into())
        })?;
    let mut builder = Int64Array::builder(jsonb_array.len());
    for index in 0..jsonb_array.len() {
        let src_buffer = jsonb_array.value(index);
        let jsonb = RawJsonb::new(src_buffer);
        if let Some(value) = jsonb
            .as_i64()
            .map_err(|e| DataFusionError::External(e.into()))?
        {
            builder.append_value(value);
        } else {
            builder.append_null();
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

pub fn make_jsonb_as_i64_udf() -> ScalarUDF {
    create_udf(
        "jsonb_as_i64",
        vec![DataType::Binary],
        DataType::Int64,
        Volatility::Immutable,
        Arc::new(&jsonb_as_i64),
    )
}
