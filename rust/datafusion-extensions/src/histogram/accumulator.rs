use std::sync::Arc;

use datafusion::{
    arrow::{
        array::{
            Array, ArrayBuilder, ArrayRef, Float64Array, Int64Array, ListBuilder, PrimitiveBuilder,
            StructBuilder, UInt64Builder,
        },
        datatypes::{DataType, Field, Float64Type, UInt64Type},
    },
    error::DataFusionError,
    logical_expr::Accumulator,
    scalar::ScalarValue,
};

use super::histogram_udaf::HistogramArray;

/// An accumulator for computing histograms.
#[derive(Debug)]
pub struct HistogramAccumulator {
    start: Option<f64>,
    end: Option<f64>,
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
            start: Some(start),
            end: Some(end),
            bins,
            min: f64::MAX,
            max: f64::MIN,
            sum: 0.0,
            sum_sq: 0.0,
            count: 0,
        }
    }

    pub fn new_non_configured() -> Self {
        Self {
            start: None,
            end: None,
            min: f64::MAX,
            max: f64::MIN,
            sum: 0.0,
            sum_sq: 0.0,
            count: 0,
            bins: Vec::new(),
        }
    }

    /// Validates and sets histogram bounds from scalar parameters.
    pub fn configure_from_params(
        &mut self,
        start: f64,
        end: f64,
        nb_bins: i64,
    ) -> datafusion::error::Result<()> {
        if nb_bins < 1 {
            return Err(DataFusionError::Execution(format!(
                "make_histogram: nb_bins must be >= 1, got {nb_bins}"
            )));
        }
        if !start.is_finite() {
            return Err(DataFusionError::Execution(format!(
                "make_histogram: start must be finite, got {start}"
            )));
        }
        if !end.is_finite() {
            return Err(DataFusionError::Execution(format!(
                "make_histogram: end must be finite, got {end}"
            )));
        }
        if start > end {
            return Err(DataFusionError::Execution(format!(
                "make_histogram: start ({start}) must be <= end ({end})"
            )));
        }
        self.start = Some(start);
        self.end = Some(end);
        self.bins.resize(nb_bins as usize, 0);
        Ok(())
    }

    /// If not configured, scans for the first non-null row and uses it as a template.
    /// If already configured or if all rows are null, does nothing.
    pub fn configure(&mut self, histo_array: &HistogramArray) -> datafusion::error::Result<()> {
        if self.start.is_some() {
            return Ok(());
        }
        let Some(idx) = (0..histo_array.len()).find(|&i| !histo_array.is_null_at(i)) else {
            return Ok(());
        };
        self.start = Some(histo_array.get_start(idx)?);
        self.end = Some(histo_array.get_end(idx)?);
        self.bins.resize(histo_array.get_bins(idx)?.len(), 0);
        Ok(())
    }

    pub fn update_batch_scalars(
        &mut self,
        scalars: &Float64Array,
    ) -> datafusion::error::Result<()> {
        if self.start.is_none() || self.end.is_none() {
            return Err(DataFusionError::Execution(
                "can't record scalar in a non-configured histogram".into(),
            ));
        }
        let start = self.start.unwrap();
        let range = self.end.unwrap() - start;
        let bin_width = range / (self.bins.len() as f64);
        for i in 0..scalars.len() {
            if !scalars.is_null(i) {
                let v = scalars.value(i);
                self.min = self.min.min(v);
                self.max = self.max.max(v);
                self.sum += v;
                self.sum_sq += v * v;
                self.count += 1;
                let bin_index = (((v - start) / bin_width).floor()) as usize;
                let bin_index = bin_index.clamp(0, self.bins.len() - 1);
                self.bins[bin_index] += 1;
            }
        }
        Ok(())
    }

    pub fn merge_histograms(
        &mut self,
        histo_array: &HistogramArray,
    ) -> datafusion::error::Result<()> {
        self.configure(histo_array)?;
        for index_histo in 0..histo_array.len() {
            if histo_array.is_null_at(index_histo) {
                continue;
            }
            let start = histo_array.get_start(index_histo)?;
            if self.start.unwrap() != start {
                return Err(DataFusionError::Execution(
                    "Error merging incompatible histograms".into(),
                ));
            }
            let end = histo_array.get_end(index_histo)?;
            if self.end.unwrap() != end {
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
        Ok(())
    }
}

impl Accumulator for HistogramAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> datafusion::error::Result<()> {
        // we support two signatures
        // scalar case: [starts, ends, bin_counts, scalars_to_reduce]
        // merge case: [histograms]

        match values.len() {
            4 => {
                if values[0].is_empty() {
                    return Ok(());
                }
                for (i, name) in [(0usize, "start"), (1, "end"), (2, "nb_bins")] {
                    if values[i].is_null(0) {
                        return Err(DataFusionError::Execution(format!(
                            "make_histogram: {name} argument is null"
                        )));
                    }
                }
                let batch_start = values[0]
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| {
                        DataFusionError::Execution("values[0] should be a Float64Array".into())
                    })?
                    .value(0);
                let batch_end = values[1]
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| {
                        DataFusionError::Execution("values[1] should be a Float64Array".into())
                    })?
                    .value(0);
                let batch_nb_bins = values[2]
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| {
                        DataFusionError::Execution("values[2] should be an Int64Array".into())
                    })?
                    .value(0);
                if let Some(configured_start) = self.start {
                    let configured_end = self.end.expect("end is set whenever start is set");
                    if configured_start != batch_start
                        || configured_end != batch_end
                        || self.bins.len() != batch_nb_bins as usize
                    {
                        return Err(DataFusionError::Execution(
                            "make_histogram: bounds/bins changed between batches".into(),
                        ));
                    }
                } else {
                    self.configure_from_params(batch_start, batch_end, batch_nb_bins)?;
                }
                let scalars = values[3]
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| {
                        DataFusionError::Execution("values[3] should be a Float64Array".into())
                    })?;
                self.update_batch_scalars(scalars)
            }
            1 => {
                let histo_array: HistogramArray = values[0].as_ref().try_into()?;
                self.merge_histograms(&histo_array)
            }

            other => Err(DataFusionError::Execution(format!(
                "invalid arguments to HistogramAccumulator::update_batch, nb_values={other}"
            ))),
        }
    }

    fn evaluate(&mut self) -> datafusion::error::Result<datafusion::scalar::ScalarValue> {
        let fields = state_arrow_fields();
        let mut struct_builder = StructBuilder::from_fields(fields, 1);
        let start_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(0)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to start builder".into()))?;
        start_builder.append_value(self.start.unwrap_or(0.0));

        let end_builder = struct_builder
            .field_builder::<PrimitiveBuilder<Float64Type>>(1)
            .ok_or_else(|| DataFusionError::Execution("Error accessing to end builder".into()))?;
        end_builder.append_value(self.end.unwrap_or(0.0));

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
        struct_builder.append(self.start.is_some());
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
            self.merge_histograms(&histo_array)?;
        }
        Ok(())
    }
}

/// Returns the Arrow fields for the histogram state.
pub fn state_arrow_fields() -> Vec<Field> {
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
