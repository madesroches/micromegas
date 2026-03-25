use async_trait::async_trait;
use datafusion::arrow::array::{Array, ArrayRef, BinaryArray, DictionaryArray, GenericBinaryArray};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{LogicalPlan, LogicalPlanBuilder};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use datafusion::scalar::ScalarValue;
use jsonb::RawJsonb;
use std::any::Any;
use std::sync::Arc;

/// A DataFusion `TableFunctionImpl` that expands a JSONB array into rows with a single `value` column.
///
/// Usage:
/// ```sql
/// SELECT jsonb_as_string(elem.value)
/// FROM jsonb_array_elements(jsonb_parse('[1, 2, 3]')) as elem
/// ```
#[derive(Debug)]
pub struct JsonbArrayElementsTableFunction {}

impl JsonbArrayElementsTableFunction {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for JsonbArrayElementsTableFunction {
    fn default() -> Self {
        Self::new()
    }
}

/// The source of JSONB data — either a literal value or a subquery/expression to evaluate.
#[derive(Debug, Clone)]
enum JsonbSource {
    Literal(ScalarValue),
    Subquery(Arc<LogicalPlan>),
}

impl TableFunctionImpl for JsonbArrayElementsTableFunction {
    fn call(&self, args: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        if args.len() != 1 {
            return Err(DataFusionError::Plan(
                "jsonb_array_elements requires exactly one argument (a JSONB array)".into(),
            ));
        }

        let source = match &args[0] {
            Expr::Literal(scalar, _metadata) => JsonbSource::Literal(scalar.clone()),
            Expr::ScalarSubquery(subquery) => JsonbSource::Subquery(subquery.subquery.clone()),
            other => {
                let plan = LogicalPlanBuilder::empty(true)
                    .project(vec![other.clone()])?
                    .build()?;
                JsonbSource::Subquery(Arc::new(plan))
            }
        };

        Ok(Arc::new(JsonbArrayElementsTableProvider { source }))
    }
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        "value",
        DataType::Binary,
        false,
    )]))
}

/// Extract element values from a JSONB array.
fn extract_elements_from_jsonb(jsonb_bytes: &[u8]) -> Result<Vec<Vec<u8>>, DataFusionError> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.array_values() {
        Ok(Some(values)) => Ok(values.into_iter().map(|v| v.as_ref().to_vec()).collect()),
        Ok(None) => Err(DataFusionError::Execution(
            "jsonb_array_elements: input is not a JSONB array".into(),
        )),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

fn elements_to_batch(elements: &[Vec<u8>]) -> Result<RecordBatch, DataFusionError> {
    if elements.is_empty() {
        return Ok(RecordBatch::new_empty(output_schema()));
    }

    let values: Vec<&[u8]> = elements.iter().map(|v| v.as_slice()).collect();
    let value_array: ArrayRef = Arc::new(BinaryArray::from(values));

    RecordBatch::try_new(output_schema(), vec![value_array])
        .map_err(|e| DataFusionError::External(e.into()))
}

fn scalar_to_elements(scalar: &ScalarValue) -> Result<Vec<Vec<u8>>, DataFusionError> {
    match scalar {
        ScalarValue::Binary(Some(bytes)) => extract_elements_from_jsonb(bytes),
        ScalarValue::Binary(None) => Ok(vec![]),
        ScalarValue::Dictionary(_, inner) => scalar_to_elements(inner.as_ref()),
        _ => Err(DataFusionError::Plan(format!(
            "jsonb_array_elements argument must be Binary (JSONB), got: {:?}",
            scalar.data_type()
        ))),
    }
}

/// Extract JSONB bytes from all rows of a column, handling both plain Binary
/// and Dictionary<Int32, Binary> encodings.
fn extract_all_jsonb_bytes_from_column(column: &ArrayRef) -> Result<Vec<Vec<u8>>, DataFusionError> {
    match column.data_type() {
        DataType::Binary => {
            let binary_array = column
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .ok_or_else(|| {
                    DataFusionError::Execution("failed to cast column to BinaryArray".into())
                })?;
            Ok((0..binary_array.len())
                .filter(|&i| !binary_array.is_null(i))
                .map(|i| binary_array.value(i).to_vec())
                .collect())
        }
        DataType::Dictionary(_, value_type) if matches!(value_type.as_ref(), DataType::Binary) => {
            let dict_array = column
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .ok_or_else(|| {
                    DataFusionError::Execution(
                        "failed to cast column to DictionaryArray<Int32, Binary>".into(),
                    )
                })?;
            let binary_values = dict_array
                .values()
                .as_any()
                .downcast_ref::<GenericBinaryArray<i32>>()
                .ok_or_else(|| {
                    DataFusionError::Execution("dictionary values are not a binary array".into())
                })?;
            Ok((0..dict_array.len())
                .filter(|&i| !dict_array.is_null(i))
                .map(|i| {
                    let key_index = dict_array.keys().value(i) as usize;
                    binary_values.value(key_index).to_vec()
                })
                .collect())
        }
        other => Err(DataFusionError::Execution(format!(
            "jsonb_array_elements subquery must return a Binary or Dictionary<Int32, Binary> column, got: {other:?}"
        ))),
    }
}

/// Table provider for expanding JSONB arrays into value rows.
#[derive(Debug)]
pub struct JsonbArrayElementsTableProvider {
    source: JsonbSource,
}

impl JsonbArrayElementsTableProvider {
    /// Creates a new provider from a JSONB scalar value (for testing).
    pub fn from_scalar(scalar: ScalarValue) -> Result<Self, DataFusionError> {
        if !matches!(&scalar, ScalarValue::Binary(Some(_))) {
            return Err(DataFusionError::Plan(format!(
                "jsonb_array_elements argument must be Binary (JSONB), got: {:?}",
                scalar.data_type()
            )));
        }
        Ok(Self {
            source: JsonbSource::Literal(scalar),
        })
    }
}

#[async_trait]
impl TableProvider for JsonbArrayElementsTableProvider {
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
        let elements = match &self.source {
            JsonbSource::Literal(scalar) => scalar_to_elements(scalar)?,
            JsonbSource::Subquery(plan) => {
                let physical_plan = state.create_physical_plan(plan).await?;
                let task_ctx = state.task_ctx();
                let batches = datafusion::physical_plan::collect(physical_plan, task_ctx).await?;

                if batches.is_empty() || batches.iter().all(|b| b.num_rows() == 0) {
                    return Err(DataFusionError::Execution(
                        "jsonb_array_elements subquery returned no rows".into(),
                    ));
                }

                let mut all_elements = Vec::new();
                for batch in &batches {
                    if batch.num_columns() != 1 {
                        return Err(DataFusionError::Execution(format!(
                            "jsonb_array_elements subquery must return exactly one column, got {}",
                            batch.num_columns()
                        )));
                    }
                    for jsonb_bytes in extract_all_jsonb_bytes_from_column(batch.column(0))? {
                        all_elements.extend(extract_elements_from_jsonb(&jsonb_bytes)?);
                    }
                }
                all_elements
            }
        };

        let mut record_batch = elements_to_batch(&elements)?;

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
