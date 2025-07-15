use super::histogram_udaf::{HistogramArray, make_histogram_arrow_type};
use datafusion::{
    arrow::{
        array::{Float64Builder, UInt64Builder},
        datatypes::DataType,
    },
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
};
use std::sync::Arc;

fn sum_from_histogram(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to sum_from_histogram".into(),
        ));
    }

    let histo_array: HistogramArray = (&values[0]).try_into()?;
    let mut result_builder = Float64Builder::with_capacity(histo_array.len());
    for index_histo in 0..histo_array.len() {
        result_builder.append_value(histo_array.get_sum(index_histo)?);
    }

    Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
}

pub fn make_sum_from_histogram_udf() -> ScalarUDF {
    create_udf(
        "sum_from_histogram",
        vec![make_histogram_arrow_type()],
        DataType::Float64,
        Volatility::Immutable,
        Arc::new(&sum_from_histogram),
    )
}

fn count_from_histogram(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 1 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to count_from_histogram".into(),
        ));
    }

    let histo_array: HistogramArray = (&values[0]).try_into()?;
    let mut result_builder = UInt64Builder::with_capacity(histo_array.len());
    for index_histo in 0..histo_array.len() {
        result_builder.append_value(histo_array.get_count(index_histo)?);
    }

    Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
}

pub fn make_count_from_histogram_udf() -> ScalarUDF {
    create_udf(
        "count_from_histogram",
        vec![make_histogram_arrow_type()],
        DataType::UInt64,
        Volatility::Immutable,
        Arc::new(&count_from_histogram),
    )
}
