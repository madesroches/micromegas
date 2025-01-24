use super::{partition::Partition, partitioned_execution_plan::make_partitioned_execution_plan};
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::{Session, TableProvider},
    datasource::TableType,
    logical_expr::TableProviderFilterPushDown,
    physical_plan::ExecutionPlan,
    prelude::*,
};
use object_store::ObjectStore;
use std::{any::Any, sync::Arc};

// unlike MaterializedView, the partition list is fixed at construction
#[derive(Debug)]
pub struct PartitionedTableProvider {
    schema: SchemaRef,
    object_store: Arc<dyn ObjectStore>,
    partitions: Arc<Vec<Partition>>,
}

impl PartitionedTableProvider {
    pub fn new(
        schema: SchemaRef,
        object_store: Arc<dyn ObjectStore>,
        partitions: Arc<Vec<Partition>>,
    ) -> Self {
        Self {
            schema,
            object_store,
            partitions,
        }
    }
}

#[async_trait]
impl TableProvider for PartitionedTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        make_partitioned_execution_plan(
            self.schema(),
            self.object_store.clone(),
            state,
            projection,
            filters,
            limit,
            self.partitions.clone(),
        )
    }

    /// Tell DataFusion to push filters down to the scan method
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> datafusion::error::Result<Vec<TableProviderFilterPushDown>> {
        // Inexact because the pruning can't handle all expressions and pruning
        // is not done at the row level -- there may be rows in returned files
        // that do not pass the filter
        Ok(vec![TableProviderFilterPushDown::Inexact; filters.len()])
    }
}
