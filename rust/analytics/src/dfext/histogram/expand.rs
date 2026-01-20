use super::histogram_udaf::HistogramArray;
use async_trait::async_trait;
use datafusion::arrow::array::{ArrayRef, Float64Array, StructArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::LogicalPlan;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use datafusion::scalar::ScalarValue;
use std::any::Any;
use std::sync::Arc;

/// A DataFusion `TableFunctionImpl` that expands a histogram struct into rows of (bin_center, count).
///
/// Usage:
/// ```sql
/// SELECT bin_center, count
/// FROM expand_histogram(
///   (SELECT make_histogram(0.0, 100.0, 100, value)
///    FROM measures WHERE name = 'cpu_usage')
/// )
/// ```
#[derive(Debug)]
pub struct ExpandHistogramTableFunction {}

impl ExpandHistogramTableFunction {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for ExpandHistogramTableFunction {
    fn default() -> Self {
        Self::new()
    }
}

/// The source of histogram data - either a literal value or a subquery to evaluate.
#[derive(Debug, Clone)]
enum HistogramSource {
    Literal(ScalarValue),
    Subquery(Arc<LogicalPlan>),
}

impl TableFunctionImpl for ExpandHistogramTableFunction {
    fn call(&self, args: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        if args.len() != 1 {
            return Err(DataFusionError::Plan(
                "expand_histogram requires exactly one argument (a histogram)".into(),
            ));
        }

        // Extract the histogram from the expression
        let source = match &args[0] {
            Expr::Literal(scalar, _metadata) => HistogramSource::Literal(scalar.clone()),
            Expr::ScalarSubquery(subquery) => HistogramSource::Subquery(subquery.subquery.clone()),
            other => {
                return Err(DataFusionError::Plan(format!(
                    "expand_histogram argument must be a histogram literal or subquery, got: {other:?}"
                )));
            }
        };

        Ok(Arc::new(ExpandHistogramTableProvider { source }))
    }
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("bin_center", DataType::Float64, false),
        Field::new("count", DataType::UInt64, false),
    ]))
}

fn expand_histogram_to_batch(
    histo_array: &HistogramArray,
    index: usize,
) -> Result<RecordBatch, DataFusionError> {
    let start = histo_array.get_start(index)?;
    let end = histo_array.get_end(index)?;
    let bins = histo_array.get_bins(index)?;

    let num_bins = bins.len();
    if num_bins == 0 {
        return Ok(RecordBatch::new_empty(output_schema()));
    }

    // Handle edge case where start == end (all values in a single point)
    let bin_width = if (end - start).abs() < f64::EPSILON {
        1.0 // Use unit width when range is zero
    } else {
        (end - start) / (num_bins as f64)
    };

    let mut bin_centers = Vec::with_capacity(num_bins);
    let mut counts = Vec::with_capacity(num_bins);

    for i in 0..num_bins {
        let bin_center = start + (i as f64 + 0.5) * bin_width;
        bin_centers.push(bin_center);
        counts.push(bins.value(i));
    }

    let bin_center_array: ArrayRef = Arc::new(Float64Array::from(bin_centers));
    let count_array: ArrayRef = Arc::new(UInt64Array::from(counts));

    RecordBatch::try_new(output_schema(), vec![bin_center_array, count_array])
        .map_err(|e| DataFusionError::External(e.into()))
}

fn extract_histogram_from_struct(
    struct_array: &Arc<StructArray>,
) -> Result<RecordBatch, DataFusionError> {
    let histo_array = HistogramArray::new(struct_array.clone());
    if histo_array.is_empty() {
        return Ok(RecordBatch::new_empty(output_schema()));
    }
    expand_histogram_to_batch(&histo_array, 0)
}

fn scalar_to_batch(scalar: &ScalarValue) -> Result<RecordBatch, DataFusionError> {
    if let ScalarValue::Struct(struct_array) = scalar {
        extract_histogram_from_struct(struct_array)
    } else {
        Err(DataFusionError::Plan(format!(
            "expand_histogram argument must be a struct (histogram), got: {:?}",
            scalar.data_type()
        )))
    }
}

/// Table provider for expanding histogram data.
#[derive(Debug)]
pub struct ExpandHistogramTableProvider {
    source: HistogramSource,
}

impl ExpandHistogramTableProvider {
    /// Creates a new provider from a histogram scalar value.
    pub fn from_scalar(scalar: ScalarValue) -> Result<Self, DataFusionError> {
        if !matches!(scalar, ScalarValue::Struct(_)) {
            return Err(DataFusionError::Plan(format!(
                "expand_histogram argument must be a struct (histogram), got: {:?}",
                scalar.data_type()
            )));
        }
        Ok(Self {
            source: HistogramSource::Literal(scalar),
        })
    }
}

#[async_trait]
impl TableProvider for ExpandHistogramTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        output_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let mut record_batch = match &self.source {
            HistogramSource::Literal(scalar) => scalar_to_batch(scalar)?,
            HistogramSource::Subquery(plan) => {
                // Execute the subquery to get the histogram scalar
                let physical_plan = state.create_physical_plan(plan).await?;
                let task_ctx = state.task_ctx();
                let batches = datafusion::physical_plan::collect(physical_plan, task_ctx).await?;

                if batches.is_empty() || batches[0].num_rows() == 0 {
                    return Err(DataFusionError::Execution(
                        "expand_histogram subquery returned no rows".into(),
                    ));
                }

                let batch = &batches[0];
                if batch.num_columns() != 1 {
                    return Err(DataFusionError::Execution(format!(
                        "expand_histogram subquery must return exactly one column, got {}",
                        batch.num_columns()
                    )));
                }

                // Extract the struct from the first row
                let column = batch.column(0);
                let struct_array = column.as_any().downcast_ref::<StructArray>().ok_or_else(
                    || {
                        DataFusionError::Execution(format!(
                            "expand_histogram subquery must return a struct (histogram), got {:?}",
                            column.data_type()
                        ))
                    },
                )?;

                let histo_array = HistogramArray::new(Arc::new(struct_array.clone()));
                if histo_array.is_empty() {
                    RecordBatch::new_empty(output_schema())
                } else {
                    expand_histogram_to_batch(&histo_array, 0)?
                }
            }
        };

        // Apply limit if specified
        if let Some(n) = limit
            && n < record_batch.num_rows()
        {
            record_batch = record_batch.slice(0, n);
        }

        let source = MemorySourceConfig::try_new(
            &[vec![record_batch]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
