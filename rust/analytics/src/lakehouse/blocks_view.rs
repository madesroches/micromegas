use super::{
    batch_update::PartitionCreationStrategy,
    dataframe_time_bounds::{DataFrameTimeBounds, NamedColumnsTimeBounds},
    metadata_partition_spec::fetch_metadata_partition_spec,
    partition_cache::PartitionCache,
    view::{PartitionSpec, View, ViewMetadata},
};
use crate::time::{TimeRange, datetime_to_scalar};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Field, Fields, Schema, TimeUnit},
    execution::runtime_env::RuntimeEnv,
    logical_expr::{Expr, col},
    prelude::*,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "blocks";
const VIEW_INSTANCE_ID: &str = "global";
lazy_static::lazy_static! {
    static ref BEGIN_TIME_COLUMN: Arc<String> = Arc::new( String::from("begin_time"));
    static ref INSERT_TIME_COLUMN: Arc<String> = Arc::new( String::from("insert_time"));
}

/// A view of the `blocks` table, providing access to telemetry block metadata.
#[derive(Debug)]
pub struct BlocksView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    data_sql: Arc<String>,
}

impl BlocksView {
    pub fn new() -> Result<Self> {
        let data_sql = Arc::new(String::from(
            r#"SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size, blocks.insert_time,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties, streams.insert_time as stream_insert_time,
           processes.start_time, processes.start_ticks, processes.tsc_frequency, processes.exe, processes.username, processes.realname, processes.computer, processes.distro, processes.cpu_brand, processes.insert_time as process_insert_time, processes.parent_process_id, processes.properties as process_properties
         FROM blocks, streams, processes
         WHERE blocks.stream_id = streams.stream_id
         AND blocks.process_id = processes.process_id
         AND blocks.insert_time >= $1
         AND blocks.insert_time < $2
         ORDER BY blocks.insert_time, blocks.block_id
         ;"#,
        ));
        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(VIEW_INSTANCE_ID)),
            data_sql,
        })
    }
}

#[async_trait]
impl View for BlocksView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        _existing_partitions: Arc<PartitionCache>,
        insert_range: TimeRange,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
        };
        let source_count_query = "
             SELECT COUNT(*) as count
             FROM blocks, streams, processes
             WHERE blocks.stream_id = streams.stream_id
             AND blocks.process_id = processes.process_id
             AND blocks.insert_time >= $1
             AND blocks.insert_time < $2
             ;";
        Ok(Arc::new(
            fetch_metadata_partition_spec(
                &lake.db_pool,
                source_count_query,
                self.data_sql.clone(),
                view_meta,
                self.get_file_schema(),
                insert_range,
                self.get_time_bounds(),
            )
            .await
            .with_context(|| "fetch_metadata_partition_spec")?,
        ))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        blocks_file_schema_hash()
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(blocks_view_schema())
    }

    async fn jit_update(
        &self,
        _runtime: Arc<RuntimeEnv>,
        _lake: Arc<DataLakeConnection>,
        _query_range: Option<TimeRange>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        anyhow::bail!("not supported");
    }

    fn make_time_filter(&self, begin: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Expr>> {
        Ok(vec![
            col("begin_time").lt_eq(lit(datetime_to_scalar(end))),
            col("insert_time").gt_eq(lit(datetime_to_scalar(begin))),
        ])
    }

    fn get_time_bounds(&self) -> Arc<dyn DataFrameTimeBounds> {
        //todo: make more robust, by changing to [ min(begin, insert), max(end, insert) ]
        Arc::new(NamedColumnsTimeBounds::new(
            BEGIN_TIME_COLUMN.clone(),
            INSERT_TIME_COLUMN.clone(),
        ))
    }

    fn get_update_group(&self) -> Option<i32> {
        Some(1000)
    }

    fn get_max_partition_time_delta(&self, strategy: &PartitionCreationStrategy) -> TimeDelta {
        match strategy {
            PartitionCreationStrategy::Abort | PartitionCreationStrategy::CreateFromSource => {
                TimeDelta::hours(1)
            }
            PartitionCreationStrategy::MergeExisting(_partitions) => TimeDelta::days(1),
        }
    }
}

/// Returns the Arrow schema for the blocks view.
pub fn blocks_view_schema() -> Schema {
    Schema::new(vec![
        Field::new("block_id", DataType::Utf8, false),
        Field::new("stream_id", DataType::Utf8, false),
        Field::new("process_id", DataType::Utf8, false),
        Field::new(
            "begin_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("begin_ticks", DataType::Int64, false),
        Field::new(
            "end_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("end_ticks", DataType::Int64, false),
        Field::new("nb_objects", DataType::Int32, false),
        Field::new("object_offset", DataType::Int64, false),
        Field::new("payload_size", DataType::Int64, false),
        Field::new(
            "insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("streams.dependencies_metadata", DataType::Binary, false),
        Field::new("streams.objects_metadata", DataType::Binary, false),
        Field::new(
            "streams.tags",
            DataType::List(Arc::new(Field::new("tag", DataType::Utf8, false))),
            true,
        ),
        Field::new(
            "streams.properties",
            DataType::List(Arc::new(Field::new(
                "Property",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, false),
                ])),
                false,
            ))),
            false,
        ),
        Field::new(
            "streams.insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "processes.start_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("processes.start_ticks", DataType::Int64, false),
        Field::new("processes.tsc_frequency", DataType::Int64, false),
        Field::new("processes.exe", DataType::Utf8, false),
        Field::new("processes.username", DataType::Utf8, false),
        Field::new("processes.realname", DataType::Utf8, false),
        Field::new("processes.computer", DataType::Utf8, false),
        Field::new("processes.distro", DataType::Utf8, false),
        Field::new("processes.cpu_brand", DataType::Utf8, false),
        Field::new(
            "processes.insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("processes.parent_process_id", DataType::Utf8, false),
        Field::new(
            "processes.properties",
            DataType::List(Arc::new(Field::new(
                "Property",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, false),
                ])),
                false,
            ))),
            false,
        ),
    ])
}

/// Returns the file schema hash for the blocks view.
pub fn blocks_file_schema_hash() -> Vec<u8> {
    vec![1]
}
