use super::{
    partition_cache::QueryPartitionProvider,
    partitioned_execution_plan::make_partitioned_execution_plan, view::View,
};
use crate::time::TimeRange;
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::{Session, TableProvider},
    datasource::TableType,
    error::DataFusionError,
    execution::runtime_env::RuntimeEnv,
    logical_expr::{Expr, TableProviderFilterPushDown},
    physical_plan::ExecutionPlan,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::{any::Any, sync::Arc};

/// A DataFusion `TableProvider` for materialized views.
#[derive(Debug)]
pub struct MaterializedView {
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    object_store: Arc<dyn ObjectStore>,
    view: Arc<dyn View>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

impl MaterializedView {
    pub fn new(
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        object_store: Arc<dyn ObjectStore>,
        view: Arc<dyn View>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
    ) -> Self {
        Self {
            runtime,
            lake,
            object_store,
            view,
            part_provider,
            query_range,
        }
    }

    pub fn get_view(&self) -> Arc<dyn View> {
        self.view.clone()
    }
}

#[async_trait]
impl TableProvider for MaterializedView {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.view.get_file_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    #[span_fn]
    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        self.view
            .jit_update(self.runtime.clone(), self.lake.clone(), self.query_range)
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;

        let partitions = self
            .part_provider
            .fetch(
                &self.view.get_view_set_name(),
                &self.view.get_view_instance_id(),
                self.query_range,
                self.view.get_file_schema_hash(),
            )
            .await
            .map_err(|e| datafusion::error::DataFusionError::External(e.into()))?;
        trace!("MaterializedView::scan nb_partitions={}", partitions.len());

        make_partitioned_execution_plan(
            self.schema(),
            self.object_store.clone(),
            state,
            projection,
            filters,
            limit,
            Arc::new(partitions),
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
