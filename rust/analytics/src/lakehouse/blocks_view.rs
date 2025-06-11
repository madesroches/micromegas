use super::{
    metadata_partition_spec::fetch_metadata_partition_spec,
    partition_cache::PartitionCache,
    view::{PartitionSpec, View, ViewMetadata},
};
use crate::time::TimeRange;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Field, Fields, Schema, TimeUnit},
    execution::runtime_env::RuntimeEnv,
    logical_expr::{col, Expr},
    prelude::*,
    scalar::ScalarValue,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "blocks";
const VIEW_INSTANCE_ID: &str = "global";
lazy_static::lazy_static! {
    static ref INSERT_TIME_COLUMN: Arc<String> = Arc::new( String::from("insert_time"));
}

#[derive(Debug)]
pub struct BlocksView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    data_sql: Arc<String>,
    event_time_column: Arc<String>,
}

impl BlocksView {
    pub fn new() -> Result<Self> {
        let data_sql = Arc::new(String::from(
        "SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size, blocks.insert_time as block_insert_time,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties, streams.insert_time,
           processes.start_time, processes.start_ticks, processes.tsc_frequency, processes.exe, processes.username, processes.realname, processes.computer, processes.distro, processes.cpu_brand, processes.insert_time as process_insert_time, processes.parent_process_id, processes.properties as process_properties
         FROM blocks, streams, processes
         WHERE blocks.stream_id = streams.stream_id
         AND blocks.process_id = processes.process_id
         AND blocks.insert_time >= $1
         AND blocks.insert_time < $2
         ORDER BY blocks.insert_time, blocks.block_id
         ;",
        ));
        let event_time_column = Arc::new(String::from("block_insert_time"));

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(VIEW_INSTANCE_ID)),
            data_sql,
            event_time_column,
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
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
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
                self.event_time_column.clone(),
                self.data_sql.clone(),
                view_meta,
                self.get_file_schema(),
                begin_insert,
                end_insert,
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
        let utc: Arc<str> = Arc::from("+00:00");
        Ok(vec![
            col("begin_time").lt_eq(lit(ScalarValue::TimestampNanosecond(
                end.timestamp_nanos_opt(),
                Some(utc.clone()),
            ))),
            col("end_time").gt_eq(lit(ScalarValue::TimestampNanosecond(
                begin.timestamp_nanos_opt(),
                Some(utc.clone()),
            ))),
        ])
    }

    fn get_min_event_time_column_name(&self) -> Arc<String> {
        INSERT_TIME_COLUMN.clone()
    }

    fn get_max_event_time_column_name(&self) -> Arc<String> {
        INSERT_TIME_COLUMN.clone()
    }

    fn get_update_group(&self) -> Option<i32> {
        Some(1000)
    }
}

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

pub fn blocks_file_schema_hash() -> Vec<u8> {
    vec![1]
}
