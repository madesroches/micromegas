use datafusion::{
    arrow::{
        array::{
            Array, ArrayBuilder, ArrayRef, Float64Array, ListArray, ListBuilder, PrimitiveBuilder,
            StructArray, StructBuilder, UInt64Array, UInt64Builder,
        },
        datatypes::{DataType, Field, Fields, Float64Type, UInt64Type},
    },
    error::DataFusionError,
    logical_expr::{
        function::AccumulatorArgs, Accumulator, AggregateUDF, ColumnarValue, Volatility,
    },
    physical_plan::expressions::Literal,
    prelude::*,
    scalar::ScalarValue,
};
use std::sync::Arc;

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

#[derive(Debug)]
struct HistogramAccumulator {
    start: f64,
    end: f64,
    min: f64,
    max: f64,
    sum: f64,
    sum_sq: f64,
    count: u64,
    bins: Vec<u64>,
}

impl HistogramAccumulator {
    pub fn new(start: f64, end: f64, nb_bins: usize) -> Self {
        let bins = vec![0; nb_bins];
        Self {
            start,
            end,
            bins,
            min: f64::MAX,
            max: f64::MIN,
            sum: 0.0,
            sum_sq: 0.0,
            count: 0,
        }
    }
}

impl Accumulator for HistogramAccumulator {
    fn update_batch(
        &mut self,
        values: &[datafusion::arrow::array::ArrayRef],
    ) -> datafusion::error::Result<()> {
        // values[0] is an array of start values
        // values[1] is an array of end values
        // values[2] is an array of bin counts
        // values[3] is the actual data we need to process
        if values.len() != 4 {
            return Err(DataFusionError::Execution(
                "invalid arguments to HistogramAccumulator::update_batch".into(),
            ));
        }
        let values = values[3]
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                DataFusionError::Execution("values[3] should ne a Float64Array".into())
            })?;
        if values.null_count() > 0 {
            return Err(DataFusionError::Execution(
                "null values not supported for histogram".into(),
            ));
        }
        let range = self.end - self.start;
        let bin_width = range / (self.bins.len() as f64);
        for i in 0..values.len() {
            let v = values.value(i);
            self.min = self.min.min(v);
            self.max = self.max.max(v);
            self.sum += v;
            self.sum_sq += v * v;
            self.count += 1;
            let bin_index = (((v - self.start) / bin_width).floor()) as usize;
            let bin_index = bin_index.clamp(0, self.bins.len() - 1);
            self.bins[bin_index] += 1;
        }
        Ok(())
    }

    fn evaluate(&mut self) -> datafusion::error::Result<datafusion::scalar::ScalarValue> {
        let fields = state_arrow_fields();
        let mut struct_builder = StructBuilder::from_fields(fields, 1);
        let start_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(0)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to start builder".into()))?;
        start_builder.append_value(self.start);

        let end_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(1)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to end builder".into()))?;
        end_builder.append_value(self.end);

        let min_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(2)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to min builder".into()))?;
        min_builder.append_value(self.min);

        let max_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(3)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to max builder".into()))?;
        max_builder.append_value(self.max);

        let sum_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(4)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to sum builder".into()))?;
        sum_builder.append_value(self.sum);

        let sum_sq_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(5)
            .ok_or_else(|| {
                DataFusionError::Execution("Error accessing to sum_sq builder".into())
            })?;
        sum_sq_builder.append_value(self.sum_sq);

        let count_builder = struct_builder
            .field_builder::<PrimitiveBuilder<UInt64Type>>(6)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to count builder".into()))?;
        count_builder.append_value(self.count);

        let bins_builder = struct_builder
            .field_builder::<ListBuilder<Box<dyn ArrayBuilder>>>(7)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to bins builder".into()))?;
        let bin_array_builder = bins_builder
            .values()
            .as_any_mut()
            .downcast_mut::<UInt64Builder>()
            .ok_or_else(|| {
                DataFusionError::Execution("Error accessing to bins array builder".into())
            })?;
        bin_array_builder.append_slice(&self.bins);
        bins_builder.append(true);
        struct_builder.append(true);
        Ok(ScalarValue::Struct(Arc::new(struct_builder.finish())))
    }

    fn size(&self) -> usize {
        size_of_val(self) + size_of_val(&self.bins)
    }

    fn state(&mut self) -> datafusion::error::Result<Vec<datafusion::scalar::ScalarValue>> {
        Ok(vec![self.evaluate()?])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> datafusion::error::Result<()> {
        for state in states {
            let histo_array: HistogramArray = state.try_into()?;
            for index_histo in 0..histo_array.len() {
                let start = histo_array.get_start(index_histo)?;
                if self.start != start {
                    return Err(DataFusionError::Execution(
                        "Error merging incompatible histograms".into(),
                    ));
                }
                let end = histo_array.get_end(index_histo)?;
                if self.end != end {
                    return Err(DataFusionError::Execution(
                        "Error merging incompatible histograms".into(),
                    ));
                }

                let min = histo_array.get_min(index_histo)?;
                let max = histo_array.get_max(index_histo)?;
                let sum = histo_array.get_sum(index_histo)?;
                let sum_sq = histo_array.get_sum_sq(index_histo)?;
                let count = histo_array.get_count(index_histo)?;
                let bins = histo_array.get_bins(index_histo)?;
                if bins.len() != self.bins.len() {
                    return Err(DataFusionError::Execution(
                        "Error merging incompatible histograms".into(),
                    ));
                }
                self.min = self.min.min(min);
                self.max = self.max.max(max);
                self.sum += sum;
                self.sum_sq += sum_sq;
                self.count += count;

                // optim opportunity: use arrow compute
                for i in 0..self.bins.len() {
                    self.bins[i] += bins.value(i);
                }
            }
        }
        Ok(())
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

fn state_arrow_fields() -> Vec<Field> {
    vec![
        Field::new("start", DataType::Float64, false),
        Field::new("end", DataType::Float64, false),
        Field::new("min", DataType::Float64, false),
        Field::new("max", DataType::Float64, false),
        Field::new("sum", DataType::Float64, false),
        Field::new("sum_sq", DataType::Float64, false),
        Field::new("count", DataType::UInt64, false),
        Field::new(
            "bins",
            DataType::List(Arc::new(Field::new("bin", DataType::UInt64, false))),
            false,
        ),
    ]
}

pub fn make_histogram_arrow_type() -> DataType {
    DataType::Struct(Fields::from(state_arrow_fields()))
}

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
