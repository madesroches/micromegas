use super::{
    partition::Partition,
    partitioned_execution_plan::{OrderingBounds, make_partitioned_execution_plan},
    reader_factory::ReaderFactory,
    view::ScanSortColumn,
};
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::{Session, TableProvider},
    datasource::TableType,
    logical_expr::TableProviderFilterPushDown,
    physical_plan::ExecutionPlan,
    prelude::*,
};
use std::sync::Arc;

/// A DataFusion `TableProvider` for a set of pre-defined partitions.
pub struct PartitionedTableProvider {
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,
    partitions: Arc<Vec<Partition>>,
    output_ordering: Vec<ScanSortColumn>,
    ordering_bounds: OrderingBounds,
}

impl std::fmt::Debug for PartitionedTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartitionedTableProvider")
            .field("schema", &self.schema)
            .field("partitions_count", &self.partitions.len())
            .finish()
    }
}

impl PartitionedTableProvider {
    pub fn new(
        schema: SchemaRef,
        reader_factory: Arc<ReaderFactory>,
        partitions: Arc<Vec<Partition>>,
    ) -> Self {
        Self {
            schema,
            reader_factory,
            partitions,
            output_ordering: vec![],
            ordering_bounds: OrderingBounds::EventTime,
        }
    }

    /// Builds a `PartitionedTableProvider` that declares `output_ordering` as an ordering the
    /// scan's rows already satisfy, checked against `ordering_bounds` (see
    /// `make_partitioned_execution_plan`).
    pub fn with_ordering(
        schema: SchemaRef,
        reader_factory: Arc<ReaderFactory>,
        partitions: Arc<Vec<Partition>>,
        output_ordering: Vec<ScanSortColumn>,
        ordering_bounds: OrderingBounds,
    ) -> Self {
        Self {
            schema,
            reader_factory,
            partitions,
            output_ordering,
            ordering_bounds,
        }
    }
}

#[async_trait]
impl TableProvider for PartitionedTableProvider {
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
            self.reader_factory.clone(),
            state,
            projection,
            filters,
            limit,
            self.partitions.clone(),
            &self.output_ordering,
            self.ordering_bounds,
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
