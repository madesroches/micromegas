use super::view_factory::ViewFactory;
use crate::lakehouse::catalog::list_view_sets;
use async_trait::async_trait;
use datafusion::arrow::array::{ArrayRef, BinaryArray, BooleanArray, StringArray};
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::MemorySourceConfig;
use datafusion::error::DataFusionError;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use std::any::Any;
use std::sync::Arc;

/// A DataFusion `TableFunctionImpl` for listing view sets with their current schema information.
#[derive(Debug)]
pub struct ListViewSetsTableFunction {
    view_factory: Arc<ViewFactory>,
}

impl ListViewSetsTableFunction {
    pub fn new(view_factory: Arc<ViewFactory>) -> Self {
        Self { view_factory }
    }
}

impl TableFunctionImpl for ListViewSetsTableFunction {
    fn call(
        &self,
        _args: &[datafusion::prelude::Expr],
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListViewSetsTableProvider {
            view_factory: self.view_factory.clone(),
        }))
    }
}

/// A DataFusion `TableProvider` for listing view sets with their current schema information.
#[derive(Debug)]
pub struct ListViewSetsTableProvider {
    pub view_factory: Arc<ViewFactory>,
}

#[async_trait]
impl TableProvider for ListViewSetsTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("view_set_name", DataType::Utf8, false),
            Field::new("current_schema_hash", DataType::Binary, false),
            Field::new("schema", DataType::Utf8, false),
            Field::new("has_view_maker", DataType::Boolean, false),
            Field::new("global_instance_available", DataType::Boolean, false),
        ]))
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        // Get current schema versions from the view factory
        let schema_infos =
            list_view_sets(&self.view_factory).map_err(|e| DataFusionError::External(e.into()))?;

        // Convert to Arrow arrays
        let view_set_names: Vec<String> = schema_infos
            .iter()
            .map(|info| info.view_set_name.clone())
            .collect();
        let schema_hashes: Vec<&[u8]> = schema_infos
            .iter()
            .map(|info| info.current_schema_hash.as_slice())
            .collect();
        let schemas: Vec<String> = schema_infos
            .iter()
            .map(|info| info.schema.clone())
            .collect();
        let has_view_makers: Vec<bool> = schema_infos
            .iter()
            .map(|info| info.has_view_maker)
            .collect();
        let global_instances: Vec<bool> = schema_infos
            .iter()
            .map(|info| info.global_instance_available)
            .collect();

        let view_set_name_array: ArrayRef = Arc::new(StringArray::from(view_set_names));
        let schema_hash_array: ArrayRef = Arc::new(BinaryArray::from(schema_hashes));
        let schema_array: ArrayRef = Arc::new(StringArray::from(schemas));
        let has_view_maker_array: ArrayRef = Arc::new(BooleanArray::from(has_view_makers));
        let global_instance_array: ArrayRef = Arc::new(BooleanArray::from(global_instances));

        let columns = vec![
            view_set_name_array,
            schema_hash_array,
            schema_array,
            has_view_maker_array,
            global_instance_array,
        ];

        let record_batch = RecordBatch::try_new(self.schema(), columns)
            .map_err(|e| DataFusionError::External(e.into()))?;

        Ok(MemorySourceConfig::try_new_exec(
            &[vec![record_batch]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?)
    }
}
