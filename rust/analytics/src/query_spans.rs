use std::{
    cmp::{max, min},
    sync::Arc,
};

use crate::{
    call_tree::make_call_tree,
    metadata::{find_process, find_stream, find_stream_blocks_in_range},
    span_table::SpanRecordBuilder,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::process_info::ProcessInfo;
use sqlx::types::chrono::{DateTime, Utc};

pub async fn query_spans(
    data_lake: &DataLakeConnection,
    limit: i64,
    stream_id: sqlx::types::Uuid,
    mut begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<RecordBatch> {
    let mut connection = data_lake.db_pool.acquire().await?;
    let stream_info = find_stream(&mut connection, stream_id)
        .await
        .with_context(|| "find_stream")?;
    let process_info = find_process(&mut connection, &stream_info.process_id)
        .await
        .with_context(|| "find_process")?;
    let convert_ticks = ConvertTicks::new(&process_info);
    begin = max(begin, process_info.start_time);
    let relative_begin_ticks = convert_ticks.to_ticks(begin - process_info.start_time);
    let relative_end_ticks = convert_ticks.to_ticks(end - process_info.start_time);
    let blocks = find_stream_blocks_in_range(
        &mut connection,
        stream_id,
        relative_begin_ticks,
        relative_end_ticks,
    )
    .await
    .with_context(|| "find_stream_blocks_in_range")?;
    drop(connection);

    let mut record_builder = SpanRecordBuilder::with_capacity(1024); //todo: replace with number of nodes

    let mut blocks_to_process = vec![];
    let mut last_end = None;
    for block in blocks {
        if block.begin_ticks == last_end.unwrap_or(block.begin_ticks) {
            last_end = Some(block.end_ticks);
            blocks_to_process.push(block);
        } else {
            append_call_tree(
                &mut record_builder,
                &process_info,
                &blocks_to_process,
                relative_begin_ticks,
                relative_end_ticks,
                limit,
                data_lake.blob_storage.clone(),
                &stream_info,
            )
            .await?;
            blocks_to_process = vec![];
            last_end = None;
        }
    }

    if !blocks_to_process.is_empty() {
        append_call_tree(
            &mut record_builder,
            &process_info,
            &blocks_to_process,
            relative_begin_ticks,
            relative_end_ticks,
            limit,
            data_lake.blob_storage.clone(),
            &stream_info,
        )
        .await?;
        drop(blocks_to_process);
    }

    record_builder
        .finish()
        .with_context(|| "finalizing span record builder")
}

#[allow(clippy::too_many_arguments)]
async fn append_call_tree(
    record_builder: &mut SpanRecordBuilder,
    process_info: &ProcessInfo,
    blocks: &[BlockMetadata],
    relative_begin_ticks: i64,
    relative_end_ticks: i64,
    limit: i64,
    blob_storage: Arc<BlobStorage>,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
) -> Result<()> {
    let begin_call_tree = max(relative_begin_ticks, blocks[0].begin_ticks);
    let end_call_tree = min(relative_end_ticks, blocks[blocks.len() - 1].end_ticks);
    let convert_ticks = ConvertTicks::new(process_info);
    let call_tree = make_call_tree(
        blocks,
        begin_call_tree + process_info.start_ticks,
        end_call_tree + process_info.start_ticks,
        limit, //todo
        blob_storage,
        convert_ticks,
        stream,
    )
    .await
    .with_context(|| "make_call_tree")?;
    record_builder
        .append_call_tree(&call_tree)
        .with_context(|| "adding call tree to span record builder")?;
    Ok(())
}
