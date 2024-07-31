use std::sync::Arc;

use crate::{
    log_entries_table::LogEntriesRecordBuilder,
    log_entry::for_each_log_entry_in_block,
    metadata::{find_process, find_stream, find_stream_blocks_in_range},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use sqlx::types::chrono::{DateTime, Utc};

pub async fn query_log_entries(
    data_lake: &DataLakeConnection,
    stream_id: sqlx::types::Uuid,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: i64,
) -> Result<RecordBatch> {
    let mut connection = data_lake.db_pool.acquire().await?;
    let stream_info = find_stream(&mut connection, stream_id)
        .await
        .with_context(|| "find_stream")?;
    let process_info = find_process(&mut connection, &stream_info.process_id)
        .await
        .with_context(|| "find_process")?;
    let convert_ticks = ConvertTicks::new(&process_info);
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

    make_log_entries_record_batch(
        &blocks,
        begin,
        end,
        limit,
        data_lake.blob_storage.clone(),
        convert_ticks,
        Arc::new(process_info),
        &stream_info,
    )
    .await
    .with_context(|| "make_log_entries_record_batch")
}

#[allow(clippy::cast_precision_loss)]
#[span_fn]
pub async fn make_log_entries_record_batch(
    blocks: &[BlockMetadata],
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: i64,
    blob_storage: Arc<BlobStorage>,
    convert_ticks: ConvertTicks,
    process: Arc<ProcessInfo>,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
) -> Result<RecordBatch> {
    let mut record_builder = LogEntriesRecordBuilder::with_capacity(1024);
    let begin_ns = begin.timestamp_nanos_opt().unwrap_or_default();
    let end_ns = end.timestamp_nanos_opt().unwrap_or_default();
    for block in blocks {
        for_each_log_entry_in_block(
            blob_storage.clone(),
            &convert_ticks,
            process.clone(),
            stream,
            block,
            |log_entry| {
                if log_entry.time >= begin_ns
                    && log_entry.time <= end_ns
                    && record_builder.len() < limit
                {
                    record_builder.append(&log_entry)?;
                }
                Ok(log_entry.time <= end_ns && record_builder.len() < limit)
            },
        )
        .await
        .with_context(|| "for_each_log_entry_in_block")?;
    }
    record_builder.finish()
}
