use super::{materialized_view::MaterializedView, partition_cache::PartitionCache};
use crate::{response_writer::Logger, time::TimeRange};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{arrow::datatypes::Schema, logical_expr::Expr, prelude::*, sql::TableReference};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

#[async_trait]
pub trait PartitionSpec: Send + Sync {
    fn get_source_data_hash(&self) -> Vec<u8>;
    async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct ViewMetadata {
    pub view_set_name: Arc<String>,
    pub view_instance_id: Arc<String>,
    pub file_schema_hash: Vec<u8>,
}

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
        lake: Arc<DataLakeConnection>,
        existing_partitions: Arc<PartitionCache>,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>>;

    /// get_file_schema_hash returns a hash (can be a version number, version string, etc.) that allows
    /// to identify out of date partitions.
    fn get_file_schema_hash(&self) -> Vec<u8>;

    /// get_file_schema returns the schema of the partition file in object storage
    fn get_file_schema(&self) -> Arc<Schema>;

    /// jit_update creates or updates process-specific partitions before a query
    async fn jit_update(
        &self,
        lake: Arc<DataLakeConnection>,
        query_range: Option<TimeRange>,
    ) -> Result<()>;

    /// make_time_filter returns a set of expressions that will filter out the rows of the partition
    /// outside the time range requested.
    fn make_time_filter(&self, _begin: DateTime<Utc>, _end: DateTime<Utc>) -> Result<Vec<Expr>>;

    /// name of the column to take the min() of to get the first event timestamp in a dataframe
    fn get_min_event_time_column_name(&self) -> Arc<String>;

    /// name of the column to take the max() of to get the last event timestamp in a dataframe
    fn get_max_event_time_column_name(&self) -> Arc<String>;

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

    fn get_merge_partitions_query(&self) -> Arc<String> {
        Arc::new(String::from("SELECT * FROM {source};"))
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
