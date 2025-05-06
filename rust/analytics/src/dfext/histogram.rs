use datafusion::{
    arrow::{
        array::{
            Array, ArrayBuilder, Float64Array, ListArray, ListBuilder, PrimitiveBuilder,
            StructArray, StructBuilder, UInt64Array, UInt64Builder,
        },
        datatypes::{DataType, Field, Fields, Float64Type, UInt64Type},
    },
    error::DataFusionError,
    logical_expr::{function::AccumulatorArgs, Accumulator, AggregateUDF, Volatility},
    physical_plan::expressions::Literal,
    prelude::*,
    scalar::ScalarValue,
};
use std::sync::Arc;

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

    fn merge_batch(
        &mut self,
        states: &[datafusion::arrow::array::ArrayRef],
    ) -> datafusion::error::Result<()> {
        for state in states {
            if state.len() != 1 {
                return Err(DataFusionError::Execution(
                    "invalid state in HistogramAccumulator::merge_batch".into(),
                ));
            }
            let struct_array = state
                .as_any()
                .downcast_ref::<StructArray>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to StructArray".into()))?;
            let starts = struct_array
                .column(0)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
            let start = starts.value(0);
            if self.start != start {
                return Err(DataFusionError::Execution(
                    "Error merging incompatible histograms".into(),
                ));
            }
            let ends = struct_array
                .column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?;
            let end = ends.value(0);
            if self.end != end {
                return Err(DataFusionError::Execution(
                    "Error merging incompatible histograms".into(),
                ));
            }

            let min = struct_array
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?
                .value(0);
            let max = struct_array
                .column(3)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?
                .value(0);
            let sum = struct_array
                .column(4)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?
                .value(0);
            let sum_sq = struct_array
                .column(5)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to Float64Array".into()))?
                .value(0);
            let count = struct_array
                .column(6)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to UInt64Array".into()))?
                .value(0);
            let bins_list = struct_array
                .column(7)
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to ListArray".into()))?;
            let bins = bins_list.value(0);
            let bins = bins
                .as_any()
                .downcast_ref::<UInt64Array>()
                .ok_or_else(|| DataFusionError::Execution("downcasting to UInt64Array".into()))?;
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

fn make_state_arrow_type() -> DataType {
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
        Arc::new(make_state_arrow_type()),
        Volatility::Immutable,
        Arc::new(&make_state),
        Arc::new(vec![make_state_arrow_type()]),
    )
}
