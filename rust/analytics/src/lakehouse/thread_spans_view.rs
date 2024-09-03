use super::{
    jit_partitions::{generate_jit_partitions, is_jit_partition_up_to_date},
    partition::{write_partition_from_rows, PartitionRowSet},
    partition_source_data::{hash_to_object_count, PartitionSourceDataBlocks},
    view::{PartitionSpec, View, ViewMetadata},
    view_factory::ViewMaker,
};
use crate::{
    call_tree::make_call_tree,
    metadata::{find_process, find_stream},
    response_writer::ResponseWriter,
    span_table::{get_spans_schema, SpanRecordBuilder},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use datafusion::{
    arrow::datatypes::Schema, catalog::TableProvider, execution::context::SessionContext,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use uuid::Uuid;

const VIEW_SET_NAME: &str = "thread_spans";

pub struct ThreadSpansViewMaker {}

impl ViewMaker for ThreadSpansViewMaker {
    fn make_view(&self, stream_id: &str) -> Result<Arc<dyn View>> {
        Ok(Arc::new(ThreadSpansView::new(stream_id)?))
    }
}

pub struct ThreadSpansView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    stream_id: sqlx::types::Uuid,
}

impl ThreadSpansView {
    pub fn new(view_instance_id: &str) -> Result<Self> {
        if view_instance_id == "global" {
            anyhow::bail!("the global view is not implemented for thread spans");
        }

        Ok(Self {
            view_set_name: Arc::new(String::from(VIEW_SET_NAME)),
            view_instance_id: Arc::new(String::from(view_instance_id)),
            stream_id: Uuid::parse_str(view_instance_id).with_context(|| "Uuid::parse_str")?,
        })
    }
}

async fn append_call_tree(
    record_builder: &mut SpanRecordBuilder,
    convert_ticks: &ConvertTicks,
    blocks: &[BlockMetadata],
    blob_storage: Arc<BlobStorage>,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
) -> Result<()> {
    let call_tree = make_call_tree(
        blocks,
        convert_ticks.delta_ticks_to_ns(blocks[0].begin_ticks),
        convert_ticks.delta_ticks_to_ns(blocks[blocks.len() - 1].end_ticks),
        None,
        blob_storage,
        convert_ticks.clone(),
        stream,
    )
    .await
    .with_context(|| "make_call_tree")?;
    record_builder
        .append_call_tree(&call_tree)
        .with_context(|| "adding call tree to span record builder")?;
    Ok(())
}

async fn write_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    convert_ticks: &ConvertTicks,
    spec: &PartitionSourceDataBlocks,
) -> Result<()> {
    let nb_events = hash_to_object_count(&spec.block_ids_hash)? as usize;
    info!("nb_events: {nb_events}");
    let mut record_builder = SpanRecordBuilder::with_capacity(nb_events / 2);
    let mut blocks_to_process = vec![];
    let mut last_end = None;
    if spec.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    // for jit partitions, we assume that the blocks were registered in order
    // since they are built based on begin_ticks, not insert_time
    let min_insert_time = spec.blocks[0].block.insert_time;
    let max_insert_time = spec.blocks[spec.blocks.len() - 1].block.insert_time;
    dbg!((min_insert_time, max_insert_time));

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    let join_handle = tokio::spawn(write_partition_from_rows(
        lake.clone(),
        view_meta,
        min_insert_time,
        max_insert_time,
        spec.block_ids_hash.clone(),
        rx,
        1024 * 1024,
        null_response_writer,
    ));

    for block in &spec.blocks {
        if block.block.begin_ticks == last_end.unwrap_or(block.block.begin_ticks) {
            last_end = Some(block.block.end_ticks);
            blocks_to_process.push(block.block.clone());
        } else {
            append_call_tree(
                &mut record_builder,
                convert_ticks,
                &blocks_to_process,
                lake.blob_storage.clone(),
                &block.stream,
            )
            .await?;
            blocks_to_process = vec![];
            last_end = None;
        }
    }
    if !blocks_to_process.is_empty() {
        append_call_tree(
            &mut record_builder,
            convert_ticks,
            &blocks_to_process,
            lake.blob_storage.clone(),
            &spec.blocks[0].stream,
        )
        .await?;
    }
    let min_time_row = convert_ticks.delta_ticks_to_time(spec.blocks[0].block.begin_ticks);
    let max_time_row =
        convert_ticks.delta_ticks_to_time(spec.blocks[spec.blocks.len() - 1].block.end_ticks);
    let rows = record_builder
        .finish()
        .with_context(|| "record_builder.finish()")?;
    info!("writing {} rows", rows.num_rows());
    tx.send(PartitionRowSet {
        min_time_row,
        max_time_row,
        rows,
    })
    .await?;
    drop(tx);
    join_handle.await??;
    Ok(())
}
/// rebuild the partition if it's missing or out of date
async fn update_partition(
    lake: Arc<DataLakeConnection>,
    view_meta: ViewMetadata,
    convert_ticks: &ConvertTicks,
    spec: &PartitionSourceDataBlocks,
) -> Result<()> {
    if is_jit_partition_up_to_date(&lake.db_pool, view_meta.clone(), convert_ticks, spec).await? {
        return Ok(());
    }
    write_partition(lake, view_meta, convert_ticks, spec)
        .await
        .with_context(|| "write_partition")?;

    Ok(())
}

#[async_trait]
impl View for ThreadSpansView {
    fn get_view_set_name(&self) -> Arc<String> {
        self.view_set_name.clone()
    }

    fn get_view_instance_id(&self) -> Arc<String> {
        self.view_instance_id.clone()
    }

    async fn make_batch_partition_spec(
        &self,
        _pool: &sqlx::PgPool,
        _begin_insert: DateTime<Utc>,
        _end_insert: DateTime<Utc>,
    ) -> Result<Arc<dyn PartitionSpec>> {
        anyhow::bail!("not implemented")
    }

    fn get_file_schema_hash(&self) -> Vec<u8> {
        vec![0]
    }

    fn get_file_schema(&self) -> Arc<Schema> {
        Arc::new(get_spans_schema())
    }

    async fn jit_update(
        &self,
        lake: Arc<DataLakeConnection>,
        begin_query: DateTime<Utc>,
        end_query: DateTime<Utc>,
    ) -> Result<()> {
        let mut connection = lake.db_pool.acquire().await?;
        let stream = Arc::new(
            find_stream(&mut connection, self.stream_id)
                .await
                .with_context(|| "find_stream")?,
        );
        let process = Arc::new(
            find_process(&mut connection, &stream.process_id)
                .await
                .with_context(|| "find_process")?,
        );
        let partitions = generate_jit_partitions(
            &mut connection,
            begin_query,
            end_query,
            stream.clone(),
            process.clone(),
        )
        .await
        .with_context(|| "generate_jit_partitions")?;
        drop(connection);
        let convert_ticks = ConvertTicks::new(&process);
        for part in &partitions {
            update_partition(
                lake.clone(),
                ViewMetadata {
                    view_set_name: self.get_view_set_name(),
                    view_instance_id: self.get_view_instance_id(),
                    file_schema_hash: self.get_file_schema_hash(),
                    file_schema: self.get_file_schema(),
                },
                &convert_ticks,
                part,
            )
            .await
            .with_context(|| "update_partition")?;
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
                "SELECT * from {full_table_name} WHERE begin <= '{}' AND end >= '{}';",
                end.to_rfc3339(),
                begin.to_rfc3339()
            ))
            .await?;
        Ok(row_filter.into_view())
    }
}
