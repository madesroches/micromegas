use crate::time::TimeRange;

use super::{
    metadata_partition_spec::fetch_metadata_partition_spec,
    partition_cache::PartitionCache,
    view::{PartitionSpec, View, ViewMetadata},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Field, Fields, Schema, TimeUnit},
    logical_expr::{col, Between, Expr},
    scalar::ScalarValue,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "streams";
const VIEW_INSTANCE_ID: &str = "global";
lazy_static::lazy_static! {
    static ref INSERT_TIME_COLUMN: Arc<String> = Arc::new( String::from("insert_time"));
}

#[derive(Debug)]
pub struct StreamsView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    data_sql: Arc<String>,
    event_time_column: Arc<String>,
}

impl StreamsView {
    pub fn new() -> Result<Self> {
        let data_sql = Arc::new(String::from(
            "SELECT stream_id,
                    process_id,
                    dependencies_metadata,
                    objects_metadata,
                    tags,
                    properties,
                    insert_time
             FROM streams
             WHERE insert_time >= $1
             AND insert_time < $2
             ORDER BY insert_time;",
        ));
        let event_time_column = Arc::new(String::from("insert_time"));

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(VIEW_INSTANCE_ID)),
            data_sql,
            event_time_column,
        })
    }
}

#[async_trait]
impl View for StreamsView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
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
        Ok(Arc::new(
            fetch_metadata_partition_spec(
                &lake.db_pool,
                "streams",
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
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(streams_view_schema())
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
        Ok(vec![Expr::Between(Between::new(
            col("insert_time").into(),
            false,
            Expr::Literal(ScalarValue::TimestampNanosecond(
                begin.timestamp_nanos_opt(),
                Some(utc.clone()),
            ))
            .into(),
            Expr::Literal(ScalarValue::TimestampNanosecond(
                end.timestamp_nanos_opt(),
                Some(utc.clone()),
            ))
            .into(),
        ))])
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

pub fn streams_view_schema() -> Schema {
    Schema::new(vec![
        Field::new("stream_id", DataType::Utf8, false),
        Field::new("process_id", DataType::Utf8, false),
        Field::new("dependencies_metadata", DataType::Binary, false),
        Field::new("objects_metadata", DataType::Binary, false),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("tag", DataType::Utf8, false))),
            true,
        ),
        Field::new(
            "properties",
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
            "insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
    ])
}
