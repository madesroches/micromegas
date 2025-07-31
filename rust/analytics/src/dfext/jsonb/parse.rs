use datafusion::arrow::array::Array;
use datafusion::{
    arrow::{
        array::{GenericBinaryBuilder, StringArray},
        datatypes::DataType,
    },
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
};
use jsonb::parse_value;
use micromegas_tracing::warn;
use std::sync::Arc;

fn parse_json_into_jsonb(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to jsonb_parse".into(),
        ));
    }
    let src_arrays = ColumnarValue::values_to_arrays(values)?;
    let string_array: &StringArray = src_arrays[0]
        .as_any()
        .downcast_ref::<_>()
        .ok_or_else(|| DataFusionError::Execution("error casting json as StringArray".into()))?;
    let mut builder = GenericBinaryBuilder::<i32>::new();
    let mut buffer = vec![];
    for index in 0..string_array.len() {
        let src_value = string_array.value(index);
        match parse_value(src_value.as_bytes()) {
            Ok(parsed) => {
                buffer.clear();
                parsed.write_to_vec(&mut buffer);
                builder.append_value(&buffer);
            }
            Err(e) => {
                warn!("error parsing json={src_value} error={e:?}");
                builder.append_null();
            }
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

/// Creates a user-defined function to parse a JSON string into a JSONB value.
pub fn make_jsonb_parse_udf() -> ScalarUDF {
    create_udf(
        "jsonb_parse",
        vec![DataType::Utf8],
        DataType::Binary,
        Volatility::Immutable,
        Arc::new(&parse_json_into_jsonb),
    )
}
