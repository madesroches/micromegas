use super::{
    batch_update::PartitionCreationStrategy,
    dataframe_time_bounds::DataFrameTimeBounds,
    materialized_view::MaterializedView,
    merge::{PartitionMerger, QueryMerger},
    partition::Partition,
    partition_cache::PartitionCache,
    session_configurator::NoOpSessionConfigurator,
    view_factory::ViewFactory,
};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::Schema,
    execution::{SendableRecordBatchStream, runtime_env::RuntimeEnv},
    logical_expr::Expr,
    prelude::*,
    sql::TableReference,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::fmt::Debug;
use std::sync::Arc;

/// A trait for defining a partition specification.
#[async_trait]
pub trait PartitionSpec: Send + Sync + Debug {
    /// Returns true if the partition is empty.
    fn is_empty(&self) -> bool;
    /// Returns a hash of the source data.
    fn get_source_data_hash(&self) -> Vec<u8>;
    /// Writes the partition to the data lake.
    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()>;
}

/// Metadata about a view.
#[derive(Debug, Clone)]
pub struct ViewMetadata {
    pub view_set_name: Arc<String>,
    pub view_instance_id: Arc<String>,
    pub file_schema_hash: Vec<u8>,
}

/// A trait for defining a view.
#[async_trait]
pub trait View: std::fmt::Debug + Send + Sync {
    /// name of the table from the user's perspective
    fn get_view_set_name(&self) -> Arc<String>;

    /// get_view_instance_id can be a process_id, a stream_id or 'global'.
    fn get_view_instance_id(&self) -> Arc<String>;

    /// make_batch_partition_spec determines what should be found in an up to date partition.
    /// The resulting PartitionSpec can be used to validate existing partitions are create a new one.
    async fn make_batch_partition_spec(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        existing_partitions: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>>;

    /// get_file_schema_hash returns a hash (can be a version number, version string, etc.) that allows
    /// to identify out of date partitions.
    fn get_file_schema_hash(&self) -> Vec<u8>;

    /// get_file_schema returns the schema of the partition file in object storage
    fn get_file_schema(&self) -> Arc<Schema>;

    /// jit_update creates or updates process-specific partitions before a query
    async fn jit_update(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        query_range: Option<TimeRange>,
    ) -> Result<()>;

    /// make_time_filter returns a set of expressions that will filter out the rows of the partition
    /// outside the time range requested.
    fn make_time_filter(&self, _begin: DateTime<Utc>, _end: DateTime<Utc>) -> Result<Vec<Expr>>;

    // a view must provide a way to compute the time bounds of a DataFrame corresponding to its schema
    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds>;

    /// register the table in the SessionContext
    async fn register_table(&self, ctx: &SessionContext, table: MaterializedView) -> Result<()> {
        let view_set_name = self.get_view_set_name().to_string();
        ctx.register_table(
            TableReference::Bare {
                table: view_set_name.into(),
            },
            Arc::new(table),
        )?;
        Ok(())
    }

    async fn merge_partitions(
        &self,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        partitions_to_merge: Arc<Vec<Partition>>,
        partitions_all_views: Arc<PartitionCache>,
    ) -> Result<SendableRecordBatchStream> {
        let merge_query = Arc::new(String::from("SELECT * FROM source;"));
        let empty_view_factory = Arc::new(ViewFactory::new(vec![]));
        let merger = QueryMerger::new(
            runtime,
            empty_view_factory,
            Arc::new(NoOpSessionConfigurator),
            self.get_file_schema(),
            merge_query,
        );
        merger
            .execute_merge_query(lake, partitions_to_merge, partitions_all_views)
            .await
    }

    /// tells the daemon which view should be materialized and in what order
    fn get_update_group(&self) -> Option<i32>;

    /// allow the view to subdivide the requested partition
    fn get_max_partition_time_delta(&self, _strategy: &PartitionCreationStrategy) -> TimeDelta {
        TimeDelta::days(1)
    }
}

impl dyn View {
    pub fn get_meta(&self) -> ViewMetadata {
        ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        }
    }
}
