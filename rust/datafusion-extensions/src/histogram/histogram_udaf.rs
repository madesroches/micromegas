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
    let start_arg = args
        .exprs
        .first()
        .ok_or_else(|| DataFusionError::Execution("Reading first argument".into()))?
        .as_any()
        .downcast_ref::<Literal>()
        .ok_or_else(|| DataFusionError::Execution("Downcasting first argument to Literal".into()))?
        .value();
    let start = if let ScalarValue::Float64(Some(start_value)) = start_arg {
        start_value
    } else {
        return Err(DataFusionError::Execution(format!(
            "arg 0 should be a float64, found {start_arg:?}"
        )));
    };

    let end_arg = args
        .exprs
        .get(1)
        .ok_or_else(|| DataFusionError::Execution("Reading argument 1".into()))?
        .as_any()
        .downcast_ref::<Literal>()
        .ok_or_else(|| DataFusionError::Execution("Downcasting argument 1 to Literal".into()))?
        .value();
    let end = if let ScalarValue::Float64(Some(end_value)) = end_arg {
        end_value
    } else {
        return Err(DataFusionError::Execution(format!(
            "arg 0 should be a float64, found {end_arg:?}"
        )));
    };

    let nb_bins_arg = args
        .exprs
        .get(2)
        .ok_or_else(|| DataFusionError::Execution("Reading argument 2".into()))?
        .as_any()
        .downcast_ref::<Literal>()
        .ok_or_else(|| DataFusionError::Execution("Downcasting argument 2 to Literal".into()))?
        .value();
    let nb_bins = if let ScalarValue::Int64(Some(nb_bins_value)) = nb_bins_arg {
        nb_bins_value
    } else {
        return Err(DataFusionError::Execution(format!(
            "arg 0 should be a int64, found {nb_bins_arg:?}"
        )));
    };

    Ok(Box::new(HistogramAccumulator::new(
        *start,
        *end,
        *nb_bins as usize,
    )))
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
