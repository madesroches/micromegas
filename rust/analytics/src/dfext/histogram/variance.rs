use super::histogram_udaf::{make_histogram_arrow_type, HistogramArray};
use datafusion::{
    arrow::{array::Float64Builder, datatypes::DataType},
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
};
use std::sync::Arc;

fn compute_variance(n: f64, sum: f64, sum_sq: f64) -> f64 {
    let mean = sum / n;
    ((sum_sq / n) - (mean * mean)) * (n / (n - 1.0))
}

fn variance_from_histogram(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to variance_from_histogram".into(),
        ));
    }

    let histo_array: HistogramArray = (&values[0]).try_into()?;
    let mut result_builder = Float64Builder::with_capacity(histo_array.len());
    for index_histo in 0..histo_array.len() {
        result_builder.append_value(compute_variance(
            histo_array.get_count(index_histo)? as f64,
            histo_array.get_sum(index_histo)?,
            histo_array.get_sum_sq(index_histo)?,
        ));
    }

    Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
}
pub fn make_variance_from_histogram_udf() -> ScalarUDF {
    create_udf(
        "variance_from_histogram",
        vec![make_histogram_arrow_type()],
        DataType::Float64,
        Volatility::Immutable,
        Arc::new(&variance_from_histogram),
    )
}
