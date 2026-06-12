use datafusion::{
    arrow::{
        array::{Array, ArrayRef, Float64Array, ListArray, StructArray, UInt64Array},
        datatypes::{DataType, Fields},
    },
    error::DataFusionError,
    logical_expr::{
        Accumulator, AggregateUDF, ColumnarValue, Volatility, function::AccumulatorArgs,
    },
    physical_plan::expressions::Literal,
    prelude::*,
    scalar::ScalarValue,
};
use std::sync::Arc;

use super::accumulator::{HistogramAccumulator, state_arrow_fields};

/// An array of histograms.
#[derive(Debug)]
pub struct HistogramArray {
    inner: Arc<StructArray>,
}

impl HistogramArray {
    pub fn new(inner: Arc<StructArray>) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> Arc<StructArray> {
        self.inner.clone()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn is_null_at(&self, index: usize) -> bool {
        self.inner.is_null(index)
    }

    pub fn get_start(&self, index: usize) -> Result<f64, DataFusionError> {
        let starts = self
            .inner
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
        Ok(starts.value(index))
    }

    pub fn get_end(&self, index: usize) -> Result<f64, DataFusionError> {
        let ends = self
            .inner
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
        Ok(ends.value(index))
    }

    pub fn get_min(&self, index: usize) -> Result<f64, DataFusionError> {
        let mins = self
            .inner
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
        Ok(mins.value(index))
    }

    pub fn get_max(&self, index: usize) -> Result<f64, DataFusionError> {
        let maxs = self
            .inner
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
        Ok(maxs.value(index))
    }

    pub fn get_sum(&self, index: usize) -> Result<f64, DataFusionError> {
        let sums = self
            .inner
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
        Ok(sums.value(index))
    }

    pub fn get_sum_sq(&self, index: usize) -> Result<f64, DataFusionError> {
        let sums_sq = self
            .inner
            .column(5)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
        Ok(sums_sq.value(index))
    }

    pub fn get_count(&self, index: usize) -> Result<u64, DataFusionError> {
        let counts = self
            .inner
            .column(6)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to UInt64Array".into()))?;
        Ok(counts.value(index))
    }

    pub fn get_bins(&self, index: usize) -> Result<UInt64Array, DataFusionError> {
        let bins_list = self
            .inner
            .column(7)
            .as_any()
            .downcast_ref::<ListArray>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to ListArray".into()))?;
        let bins = bins_list.value(index);
        let bins = bins
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to UInt64Array".into()))?;
        Ok(bins.clone())
    }
}

impl TryFrom<&ArrayRef> for HistogramArray {
    type Error = DataFusionError;

    fn try_from(value: &ArrayRef) -> Result<Self, Self::Error> {
        let struct_array = value
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to StructArray".into()))?;
        let inner = Arc::new(struct_array.clone());
        Ok(Self { inner })
    }
}

impl TryFrom<&dyn Array> for HistogramArray {
    type Error = DataFusionError;

    fn try_from(value: &dyn Array) -> Result<Self, Self::Error> {
        let struct_array = value
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(|| DataFusionError::Execution("downcasting to StructArray".into()))?;
        let inner = Arc::new(struct_array.clone());
        Ok(Self { inner })
    }
}

impl TryFrom<&ColumnarValue> for HistogramArray {
    type Error = DataFusionError;

    fn try_from(value: &ColumnarValue) -> Result<Self, Self::Error> {
        match value {
            ColumnarValue::Array(array) => array.try_into(),
            ColumnarValue::Scalar(scalar_value) => {
                if let ScalarValue::Struct(array) = scalar_value {
                    Ok(Self::new(array.clone()))
                } else {
                    Err(DataFusionError::Execution( "Can't convert ColumnarValue into HistogramArray: ScalarValue is not a struct".into()))
                }
            }
        }
    }
}

fn make_state(args: AccumulatorArgs) -> Result<Box<dyn Accumulator>, DataFusionError> {
    let start_literal = args
        .exprs
        .first()
        .and_then(|e| e.as_any().downcast_ref::<Literal>())
        .and_then(|l| {
            if let ScalarValue::Float64(Some(v)) = l.value() {
                Some(*v)
            } else {
                None
            }
        });
    let end_literal = args
        .exprs
        .get(1)
        .and_then(|e| e.as_any().downcast_ref::<Literal>())
        .and_then(|l| {
            if let ScalarValue::Float64(Some(v)) = l.value() {
                Some(*v)
            } else {
                None
            }
        });
    let nb_bins_literal = args
        .exprs
        .get(2)
        .and_then(|e| e.as_any().downcast_ref::<Literal>())
        .and_then(|l| {
            if let ScalarValue::Int64(Some(v)) = l.value() {
                Some(*v)
            } else {
                None
            }
        });

    let mut acc = HistogramAccumulator::new_non_configured();
    if let (Some(start), Some(end), Some(nb_bins)) = (start_literal, end_literal, nb_bins_literal) {
        acc.configure_from_params(start, end, nb_bins)?;
    }
    Ok(Box::new(acc))
}

pub fn make_histogram_arrow_type() -> DataType {
    DataType::Struct(Fields::from(state_arrow_fields()))
}

/// Creates a user-defined aggregate function to compute histograms.
pub fn make_histo_udaf() -> AggregateUDF {
    create_udaf(
        "make_histogram",
        vec![
            DataType::Float64,
            DataType::Float64,
            DataType::Int64,
            DataType::Float64,
        ],
        Arc::new(make_histogram_arrow_type()),
        Volatility::Immutable,
        Arc::new(&make_state),
        Arc::new(vec![make_histogram_arrow_type()]),
    )
}
