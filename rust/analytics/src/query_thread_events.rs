use crate::{
    metadata::{find_process, find_stream, find_stream_blocks_in_range},
    thread_block_processor::parse_thread_block,
    thread_events_table::ThreadEventsRecordBuilder,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use sqlx::types::chrono::{DateTime, Utc};
use std::{cmp::max, sync::Arc};

pub async fn query_thread_events(
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
    let mut relative_end_ticks = convert_ticks.to_ticks(end - process_info.start_time);
    let blocks = find_stream_blocks_in_range(
        &mut connection,
        stream_id,
        relative_begin_ticks,
        relative_end_ticks,
    )
    .await
    .with_context(|| "find_stream_blocks_in_range")?;
    drop(connection);

    if let Some(b) = blocks.last().as_ref() {
        relative_end_ticks = relative_end_ticks.min(b.end_ticks);
    }

    make_thread_events_record_batch(
        &blocks,
        limit,
        convert_ticks.ticks_to_nanoseconds(relative_begin_ticks + process_info.start_ticks),
        convert_ticks.ticks_to_nanoseconds(relative_end_ticks + process_info.start_ticks),
        data_lake.blob_storage.clone(),
        convert_ticks,
        &stream_info,
    )
    .await
    .with_context(|| "make_thread_events_record_batch")
}

#[span_fn]
pub async fn make_thread_events_record_batch(
    blocks: &[BlockMetadata],
    limit: i64,
    begin_query_ns: i64,
    end_query_ns: i64,
    blob_storage: Arc<BlobStorage>,
    convert_ticks: ConvertTicks,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
) -> Result<RecordBatch> {
    let mut record_builder = ThreadEventsRecordBuilder::new(
        begin_query_ns,
        end_query_ns,
        limit, //todo: handle limit
        convert_ticks,
        1024 * 1024,
    ); // should we use limit as capacity, we would then always allocate the worst case
    for block in blocks {
        parse_thread_block(
            blob_storage.clone(),
            stream,
            block.block_id,
            block.object_offset,
            &mut record_builder,
        )
        .await?;
    }

    record_builder.finish()
}
