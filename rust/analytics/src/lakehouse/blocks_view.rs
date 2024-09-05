use super::{
    metadata_partition_spec::fetch_metadata_partition_spec,
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::{DataType, Field, Fields, Schema, TimeUnit},
    catalog::TableProvider,
    execution::context::SessionContext,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

const VIEW_SET_NAME: &str = "blocks";
const VIEW_INSTANCE_ID: &str = "global";

pub struct BlocksViewMaker {}

impl ViewMaker for BlocksViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(BlocksView::new(view_instance_id)?))
    }
}

pub struct BlocksView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    data_sql: Arc<String>,
}

impl BlocksView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id != "global" {
            anyhow::bail!("only global view instance id is supported for metadata views");
        }

        let data_sql = Arc::new(String::from(
        "SELECT block_id, streams.stream_id, processes.process_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.object_offset, blocks.payload_size, blocks.insert_time,
           streams.dependencies_metadata, streams.objects_metadata, streams.tags, streams.properties,
           processes.start_time, processes.start_ticks, processes.tsc_frequency, processes.exe, processes.username, processes.realname, processes.computer, processes.distro, processes.cpu_brand, processes.insert_time, processes.parent_process_id, processes.properties as process_properties
         FROM blocks, streams, processes
         WHERE blocks.stream_id = streams.stream_id
         AND streams.process_id = processes.process_id
         AND blocks.insert_time >= $1
         AND blocks.insert_time < $2
         ORDER BY blocks.insert_time, blocks.block_id
         ;",
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
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
            file_schema: self.get_file_schema(),
        };
        Ok(Arc::new(
            fetch_metadata_partition_spec(
                pool,
                "blocks",
                self.data_sql.clone(),
                view_meta,
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
        Arc::new(blocks_view_schema())
    }

    async fn jit_update(
        &self,
        _lake: Arc<DataLakeConnection>,
        _begin_query: DateTime<Utc>,
        _end_query: DateTime<Utc>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        anyhow::bail!("not supported");
    }

    async fn make_filtering_table_provider(
        &self,
        ctx: &SessionContext,
        full_table_name: &str,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Arc<dyn TableProvider>> {
        let row_filter = ctx
            .sql(&format!(
                "SELECT * from {full_table_name} WHERE insert_time BETWEEN '{}' AND '{}';",
                begin.to_rfc3339(),
                end.to_rfc3339(),
            ))
            .await?;
        Ok(row_filter.into_view())
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
