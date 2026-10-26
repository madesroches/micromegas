use super::{as_binary_array, deserialize_sketch};
use datafusion::arrow::array::{Array, UInt64Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility};
use datafusion::prelude::create_udf;
use std::sync::Arc;

const NAME: &str = "hll_cardinality";

fn hll_cardinality(args: &[ColumnarValue]) -> Result<ColumnarValue> {
    let arrays = ColumnarValue::values_to_arrays(args)?;
    let [array] = arrays.as_slice() else {
        return Err(DataFusionError::Execution(format!(
            "wrong number of arguments to {NAME}"
        )));
    };
    let binary = as_binary_array(NAME, array)?;
    let mut builder = UInt64Builder::with_capacity(binary.len());
    for i in 0..binary.len() {
        if binary.is_null(i) {
            builder.append_null();
        } else {
            let sketch = deserialize_sketch(NAME, binary.value(i))?;
            builder.append_value(sketch.estimate().round() as u64);
        }
    }
    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

/// Creates a scalar UDF returning the estimated distinct count of a serialized HLL sketch.
pub fn make_hll_cardinality_udf() -> ScalarUDF {
    create_udf(
        NAME,
        vec![DataType::Binary],
        DataType::UInt64,
        Volatility::Immutable,
        Arc::new(&hll_cardinality),
    )
}
