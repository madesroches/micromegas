use super::{partition::Partition, reader_factory::ReaderFactory};
use crate::dfext::predicate::filters_to_predicate;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::{Session, memory::DataSourceExec},
    datasource::{
        listing::PartitionedFile,
        physical_plan::{FileScanConfigBuilder, ParquetSource},
    },
    execution::object_store::ObjectStoreUrl,
    physical_plan::ExecutionPlan,
    prelude::*,
};
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::sync::Arc;

/// Creates a partitioned execution plan for scanning Parquet files.
#[expect(clippy::too_many_arguments)]
#[span_fn]
pub fn make_partitioned_execution_plan(
    schema: SchemaRef,
    object_store: Arc<dyn ObjectStore>,
    state: &dyn Session,
    projection: Option<&Vec<usize>>,
    filters: &[Expr],
    limit: Option<usize>,
    partitions: Arc<Vec<Partition>>,
    pool: sqlx::PgPool,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let predicate = filters_to_predicate(schema.clone(), state, filters)?;

    // Filter out empty partitions (num_rows = 0, file_path = None)
    let mut file_group = vec![];
    for part in &*partitions {
        if !part.is_empty() {
            let file_path = part.file_path.as_ref().ok_or_else(|| {
                datafusion::error::DataFusionError::Internal(format!(
                    "non-empty partition has no file_path: num_rows={}",
                    part.num_rows
                ))
            })?;
            file_group.push(PartitionedFile::new(file_path, part.file_size as u64));
        }
    }

    // If all partitions are empty, return EmptyExec with projected schema
    if file_group.is_empty() {
        use datafusion::physical_plan::empty::EmptyExec;
        let projected_schema = if let Some(projection) = projection {
            Arc::new(schema.project(projection)?)
        } else {
            schema
        };
        return Ok(Arc::new(EmptyExec::new(projected_schema)));
    }

    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let reader_factory = Arc::new(ReaderFactory::new(object_store, pool));
    let source = Arc::new(
        ParquetSource::default()
            .with_predicate(predicate)
            .with_parquet_file_reader_factory(reader_factory),
    );
    let file_scan_config = FileScanConfigBuilder::new(object_store_url, schema, source)
        .with_limit(limit)
        .with_projection_indices(projection.cloned())
        .with_file_groups(vec![file_group.into()])
        .build();
    Ok(Arc::new(DataSourceExec::new(Arc::new(file_scan_config))))
}
