use datafusion::{
    arrow::{
        array::{Float64Array, Float64Builder, UInt64Array},
        datatypes::DataType,
    },
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDF, Volatility},
    prelude::*,
    scalar::ScalarValue,
};
use std::sync::Arc;

use super::histogram_udaf::{make_histogram_arrow_type, HistogramArray};

fn estimate_quantile(
    ratio: f64,
    start: f64,
    end: f64,
    count_values: u64,
    bins: &UInt64Array,
) -> f64 {
    let quant_count = count_values as f64 * ratio;
    let mut count = 0;
    for ibin in 0..bins.len() {
        let this_bucket_count = bins.value(ibin);
        count += this_bucket_count;
        if count as f64 >= quant_count && this_bucket_count > 0 {
            let pop_bucket_start = (count - bins.value(ibin)) as f64;
            let pop_bucket_end = count as f64;
            let bucket_ratio =
                (quant_count - pop_bucket_start) / (pop_bucket_end - pop_bucket_start);
            let histo_width = end - start;
            let bucket_width = histo_width / bins.len() as f64;
            let begin_bucket = start + ibin as f64 * bucket_width;
            let end_bucket = start + (ibin as f64 + 1.0) * bucket_width;
            let estimate = (1.0 - bucket_ratio) * begin_bucket + bucket_ratio * end_bucket;
            return estimate;
        }
    }
    end
}

fn quantile_from_histogram(values: &[ColumnarValue]) -> Result<ColumnarValue, DataFusionError> {
    if values.len() != 2 {
        return Err(DataFusionError::Execution(
            "wrong number of arguments to quantile_from_histogram".into(),
        ));
    }

    let histo_array: HistogramArray = (&values[0]).try_into()?;
    let mut result_builder = Float64Builder::with_capacity(histo_array.len());
    for index_histo in 0..histo_array.len() {
        let ratio = match &values[1] {
            ColumnarValue::Array(array) => array
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?
                .value(index_histo),
            ColumnarValue::Scalar(scalar_value) => {
                if let ScalarValue::Float64(Some(ratio)) = scalar_value {
                    *ratio
                } else {
                    return Err(DataFusionError::Execution(format!(
                        "bad ratio {scalar_value:?} in quantile_from_histogram"
                    )));
                }
            }
        };

        let bins = histo_array.get_bins(index_histo)?;
        result_builder.append_value(estimate_quantile(
            ratio,
            histo_array.get_start(index_histo)?,
            histo_array.get_end(index_histo)?,
            histo_array.get_count(index_histo)?,
            &bins,
        ));
    }

    Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
}

pub fn make_quantile_from_histogram_udf() -> ScalarUDF {
    create_udf(
        "quantile_from_histogram",
        vec![make_histogram_arrow_type(), DataType::Float64],
        DataType::Float64,
        Volatility::Immutable,
        Arc::new(&quantile_from_histogram),
    )
}
