use super::{partition::Partition, reader_factory::ReaderFactory};
use crate::dfext::predicate::filters_to_predicate;
use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::Session,
    datasource::{
        listing::PartitionedFile,
        physical_plan::{FileScanConfig, ParquetSource},
    },
    execution::object_store::ObjectStoreUrl,
    physical_plan::ExecutionPlan,
    prelude::*,
};
use object_store::ObjectStore;
use std::sync::Arc;

pub fn make_partitioned_execution_plan(
    schema: SchemaRef,
    object_store: Arc<dyn ObjectStore>,
    state: &dyn Session,
    projection: Option<&Vec<usize>>,
    filters: &[Expr],
    limit: Option<usize>,
    partitions: Arc<Vec<Partition>>,
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
    let predicate = filters_to_predicate(schema.clone(), state, filters)?;
    let mut file_group = vec![];
    for part in &*partitions {
        file_group.push(PartitionedFile::new(&part.file_path, part.file_size as u64));
    }

    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let reader_factory = Arc::new(ReaderFactory::new(object_store, partitions));
    let source = Arc::new(
        ParquetSource::default()
            .with_predicate(schema.clone(), predicate)
            .with_parquet_file_reader_factory(reader_factory),
    );
    Ok(FileScanConfig::new(object_store_url, schema, source)
        .with_limit(limit)
        .with_projection(projection.cloned())
        .with_file_groups(vec![file_group])
        .build())
}
