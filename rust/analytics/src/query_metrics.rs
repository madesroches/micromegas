use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

use crate::{
    measure::for_each_measure_in_block,
    metadata::{find_process, find_stream, find_stream_blocks_in_range},
    metrics_table::MetricsRecordBuilder,
    time::ConvertTicks,
};

pub async fn query_metrics(
    data_lake: &DataLakeConnection,
    limit: i64,
    stream_id: sqlx::types::Uuid,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<RecordBatch> {
    let mut connection = data_lake.db_pool.acquire().await?;
    let stream_info = find_stream(&mut connection, stream_id)
        .await
        .with_context(|| "find_stream")?;
    let process_info = Arc::new(
        find_process(&mut connection, &stream_info.process_id)
            .await
            .with_context(|| "find_process")?,
    );
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

    make_metrics_record_batch(
        &blocks,
        limit,
        begin,
        end,
        data_lake.blob_storage.clone(),
        convert_ticks,
        process_info,
        &stream_info,
    )
    .await
    .with_context(|| "make_metrics_record_batch")
}

#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
#[span_fn]
pub async fn make_metrics_record_batch(
    blocks: &[BlockMetadata],
    limit: i64,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    blob_storage: Arc<BlobStorage>,
    convert_ticks: ConvertTicks,
    process: Arc<ProcessInfo>,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
) -> Result<RecordBatch> {
    let mut record_builder = MetricsRecordBuilder::with_capacity(1024);
    let begin_ns = begin.timestamp_nanos_opt().unwrap_or_default();
    let end_ns = end.timestamp_nanos_opt().unwrap_or_default();
    let mut nb = 0;
    for block in blocks {
        let continue_iterating = for_each_measure_in_block(
            blob_storage.clone(),
            &convert_ticks,
            process.clone(),
            stream,
            block,
            |measure| {
                if measure.time < begin_ns {
                    return Ok(true);
                }
                if measure.time > end_ns || nb >= limit {
                    return Ok(false);
                }
                record_builder.append(&measure)?;
                nb += 1;
                Ok(nb < limit)
            },
        )
        .await
        .with_context(|| "for_each_measure_in_block")?;
        if !continue_iterating {
            break;
        }
    }
    record_builder.finish()
}
