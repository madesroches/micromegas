use super::{accumulator::HistogramAccumulator, histogram_udaf::make_histogram_arrow_type};
use datafusion::{
    error::DataFusionError,
    logical_expr::{function::AccumulatorArgs, Accumulator, AggregateUDF, Volatility},
    prelude::*,
};
use std::sync::Arc;

fn make_empty_accumulator(_args: AccumulatorArgs) -> Result<Box<dyn Accumulator>, DataFusionError> {
    Ok(Box::new(HistogramAccumulator::new_non_configured()))
}

pub fn sum_histograms_udaf() -> AggregateUDF {
    create_udaf(
        "sum_histograms",
        vec![make_histogram_arrow_type()],
        Arc::new(make_histogram_arrow_type()),
        Volatility::Immutable,
        Arc::new(&make_empty_accumulator),
        Arc::new(vec![make_histogram_arrow_type()]),
    )
}
