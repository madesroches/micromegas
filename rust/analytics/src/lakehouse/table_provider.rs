use super::{partition_cache::QueryPartitionProvider, view::View};
use crate::{lakehouse::reader_factory::ReaderFactory, time::TimeRange};
use async_trait::async_trait;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::{Session, TableProvider},
    common::DFSchema,
    datasource::{
        listing::PartitionedFile,
        physical_plan::{parquet::ParquetExecBuilder, FileScanConfig},
        TableType,
    },
    execution::object_store::ObjectStoreUrl,
    logical_expr::{utils::conjunction, Expr, TableProviderFilterPushDown},
    physical_plan::{expressions, ExecutionPlan, PhysicalExpr},
};
use object_store::ObjectStore;
use std::{any::Any, sync::Arc};

pub struct MaterializedView {
    object_store: Arc<dyn ObjectStore>,
    view: Arc<dyn View>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: TimeRange,
}

impl MaterializedView {
    pub fn new(
        object_store: Arc<dyn ObjectStore>,
        view: Arc<dyn View>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: TimeRange,
    ) -> Self {
        Self {
            object_store,
            view,
            part_provider,
            query_range,
        }
    }

    // from datafusion/datafusion-examples/examples/advanced_parquet_index.rs
    fn filters_to_predicate(
        &self,
        state: &dyn Session,
        filters: &[Expr],
    ) -> datafusion::error::Result<Arc<dyn PhysicalExpr>> {
        let df_schema = DFSchema::try_from(self.schema())?;
        let predicate = conjunction(filters.to_vec());
        let predicate = predicate
            .map(|predicate| state.create_physical_expr(predicate, &df_schema))
            .transpose()?
            // if there are no filters, use a literal true to have a predicate
            // that always evaluates to true we can pass to the index
            .unwrap_or_else(|| expressions::lit(true));

        Ok(predicate)
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

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let predicate = self.filters_to_predicate(state, filters)?;
        let mut file_group = vec![];
        let partitions = self
            .part_provider
            .fetch(
                &self.view.get_view_set_name(),
                &self.view.get_view_instance_id(),
                self.query_range.begin,
                self.query_range.end,
                self.view.get_file_schema_hash(),
            )
            .await
            .map_err(|e| datafusion::error::DataFusionError::External(e.into()))?;

        for part in &partitions {
            file_group.push(PartitionedFile::new(&part.file_path, part.file_size as u64));
        }

        let schema = self.schema();
        let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
        let file_scan_config = FileScanConfig::new(object_store_url, schema)
            .with_limit(limit)
            .with_projection(projection.cloned())
            .with_file_groups(vec![file_group]);
        let reader_factory =
            ReaderFactory::new(Arc::clone(&self.object_store), Arc::new(partitions));
        Ok(ParquetExecBuilder::new(file_scan_config)
            .with_predicate(predicate)
            .with_parquet_file_reader_factory(Arc::new(reader_factory))
            .build_arc())
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
