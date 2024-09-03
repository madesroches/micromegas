use super::{
    block_partition_spec::BlockPartitionSpec,
    log_block_processor::LogBlockProcessor,
    partition_source_data::{fetch_partition_source_data, PartitionSourceDataBlocks},
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use crate::{
    lakehouse::jit_partitions::{generate_jit_partitions, is_partition_up_to_date},
    log_entries_table::log_table_schema,
    metadata::{find_process, stream_from_row},
    response_writer::ResponseWriter,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::Schema, catalog::TableProvider, execution::context::SessionContext,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "log_entries";

pub struct LogViewMaker {}

impl ViewMaker for LogViewMaker {
    fn make_view(&self, view_instance_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(LogView::new(view_instance_id)?))
    }
}

pub struct LogView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    process_id: Option<sqlx::types::Uuid>,
}

impl LogView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        let process_id = if view_instance_id == "global" {
            None
        } else {
            Some(Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?)
        };

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(view_instance_id.into()),
            process_id,
        })
    }
}

#[async_trait]
impl View for LogView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_partition_spec(
        &self,
        pool: &sqlx::PgPool,
        begin_insert: DateTime<Utc>,
        end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        if *self.view_instance_id == "global" {
            anyhow::bail!("not supported for jit queries... yet?");
        }
        let source_data = fetch_partition_source_data(pool, begin_insert, end_insert, "log")
            .await
            .with_context(|| "fetch_partition_source_data")?;
        Ok(Arc::new(BlockPartitionSpec {
            view_metadata: ViewMetadata {
                view_set_name: self.view_set_name.clone(),
                view_instance_id: self.view_instance_id.clone(),
                file_schema: self.get_file_schema(),
                file_schema_hash: self.get_file_schema_hash(),
            },
            begin_insert,
            end_insert,
            source_data,
            block_processor: Arc::new(LogBlockProcessor {}),
        }))
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(log_table_schema())
    }

    async fn jit_update(
        &self,
        lake: Arc<DataLakeConnection>,
        begin_query: DateTime<Utc>,
        end_query: DateTime<Utc>,
    ) -> Result<()> {
        if *self.view_instance_id == "global" {
            // this view instance is updated using the deamon
            return Ok(());
        }
        let mut connection = lake.db_pool.acquire().await?;
        let process = Arc::new(
            find_process(
                &mut connection,
                &self
                    .process_id
                    .with_context(|| "getting a view's process_id")?,
            )
            .await
            .with_context(|| "find_process")?,
        );

        let stream_rows = sqlx::query(
			"SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
             FROM streams
             WHERE process_id = $1
             AND array_position(tags, $2) is not NULL
             ;")
             .bind(self.process_id)
             .bind("log")
			.fetch_all(&mut *connection)
            .await
			.with_context( || "fetching streams")?;
        let convert_ticks = ConvertTicks::new(&process);
        //todo: move to generate_jit_partitions
        let relative_begin_ticks = convert_ticks.to_ticks(begin_query - process.start_time);
        let relative_end_ticks = convert_ticks.to_ticks(end_query - process.start_time);
        let mut all_partitions = vec![];
        for row in stream_rows {
            let stream = Arc::new(stream_from_row(&row).with_context(|| "stream_from_row")?);
            let mut partitions = generate_jit_partitions(
                &mut connection,
                relative_begin_ticks,
                relative_end_ticks,
                stream,
                process.clone(),
            )
            .await
            .with_context(|| "generate_jit_partitions")?;
            all_partitions.append(&mut partitions);
        }
        drop(connection);
        let view_meta = ViewMetadata {
            view_set_name: self.get_view_set_name(),
            view_instance_id: self.get_view_instance_id(),
            file_schema_hash: self.get_file_schema_hash(),
            file_schema: self.get_file_schema(),
        };

        for part in all_partitions {
            if !is_partition_up_to_date(&lake.db_pool, view_meta.clone(), &convert_ticks, &part)
                .await?
            {
                write_partition(lake.clone(), view_meta.clone(), part)
                    .await
                    .with_context(|| "write_partition")?;
            }
        }
        Ok(())
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
                "SELECT * from {full_table_name} WHERE time BETWEEN '{}' AND '{}';",
                begin.to_rfc3339(),
                end.to_rfc3339(),
            ))
            .await?;
        Ok(row_filter.into_view())
    }
}

async fn write_partition(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    source_data: PartitionSourceDataBlocks,
) -> Result<()> {
    if source_data.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    let min_insert_time = source_data.blocks[0].block.insert_time;
    let max_insert_time = source_data.blocks[source_data.blocks.len() - 1]
        .block
        .insert_time;
    let block_spec = BlockPartitionSpec {
        view_metadata,
        begin_insert: min_insert_time,
        end_insert: max_insert_time,
        source_data,
        block_processor: Arc::new(LogBlockProcessor {}),
    };
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    block_spec
        .write(lake, null_response_writer)
        .await
        .with_context(|| "block_spec.write")?;
    Ok(())
}
