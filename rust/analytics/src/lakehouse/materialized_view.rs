use super::{
    audience_guard::AudienceGuard, lakehouse_context::LakehouseContext,
    partition_cache::QueryPartitionProvider,
    partitioned_execution_plan::make_partitioned_execution_plan, reader_factory::ReaderFactory,
    view::View,
};
use crate::time::TimeRange;
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::{Session, TableProvider},
    datasource::TableType,
    error::DataFusionError,
    logical_expr::{Expr, TableProviderFilterPushDown},
    physical_plan::ExecutionPlan,
};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A DataFusion `TableProvider` for materialized views.
#[derive(Debug)]
pub struct MaterializedView {
    lakehouse: Arc<LakehouseContext>,
    reader_factory: Arc<ReaderFactory>,
    view: Arc<dyn View>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    /// #1486: the `view_instance(...)` scan-time audience check, run before `jit_update`.
    /// `Some` only when this provider was built by `ViewInstanceTableFunction` -- a
    /// caller-named instance. `None` for every server-constructed `MaterializedView`: the
    /// implicitly-registered global tables (`query.rs::register_table`) and `OwnershipRewrite`'s
    /// own `processes`/`streams` sources, which are Prong A's job to filter row-by-row and must
    /// never be denied wholesale.
    instance_guard: Option<Arc<AudienceGuard>>,
}

impl MaterializedView {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        reader_factory: Arc<ReaderFactory>,
        view: Arc<dyn View>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
        instance_guard: Option<Arc<AudienceGuard>>,
    ) -> Self {
        Self {
            lakehouse,
            reader_factory,
            view,
            part_provider,
            query_range,
            instance_guard,
        }
    }

    pub fn get_view(&self) -> Arc<dyn View> {
        self.view.clone()
    }

    /// Whether `AudienceGuard::authorize_view_instance` will resolve this view's instance id
    /// against the caller's scope, and deny, before `scan` yields a row. True only for a
    /// caller-named, non-`'global'` `view_instance(...)`: those take the guard's Uuid arm (or its
    /// fail-closed fallthrough). `'global'` is excluded because the guard passes it unconditionally
    /// -- global instances are row-filtered instead.
    ///
    /// Kept in step with `AudienceGuard::authorize_view_instance`'s arms; changing those means
    /// revisiting this.
    pub fn instance_is_audience_guarded(&self) -> bool {
        self.instance_guard.is_some() && self.view.get_view_instance_id().as_str() != "global"
    }
}

#[async_trait]
impl TableProvider for MaterializedView {
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
        if let Some(guard) = &self.instance_guard {
            guard
                .authorize_view_instance(
                    &self.view.get_view_set_name(),
                    &self.view.get_view_instance_id(),
                )
                .await?;
        }

        self.view
            .jit_update(self.lakehouse.clone(), self.query_range)
            .await
            .map_err(|e| DataFusionError::External(format!("{e:#}").into()))?;

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
            self.reader_factory.clone(),
            state,
            projection,
            filters,
            limit,
            Arc::new(partitions),
            &self.view.get_scan_output_ordering(),
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
