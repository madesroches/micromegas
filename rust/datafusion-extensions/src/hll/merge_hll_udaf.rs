use super::{HllAccumulator, binary_type, merge_sketches};
use datafusion::arrow::array::ArrayRef;
use datafusion::error::Result;
use datafusion::logical_expr::{Accumulator, AggregateUDF, Volatility, function::AccumulatorArgs};
use datafusion::prelude::create_udaf;
use datasketches::hll::HllUnion;
use std::sync::Arc;

const NAME: &str = "merge_hll";

fn add_sketches(union: &mut HllUnion, array: &ArrayRef) -> Result<()> {
    merge_sketches(NAME, union, array)
}

fn make_accumulator(_args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
    Ok(Box::new(HllAccumulator::new(NAME, add_sketches)?))
}

/// Creates the aggregate function that unions serialized HLL sketches.
pub fn merge_hll_udaf() -> AggregateUDF {
    create_udaf(
        NAME,
        vec![binary_type()],
        Arc::new(binary_type()),
        Volatility::Immutable,
        Arc::new(&make_accumulator),
        Arc::new(vec![binary_type()]),
    )
}
