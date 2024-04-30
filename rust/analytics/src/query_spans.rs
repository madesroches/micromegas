use std::cmp::max;

use crate::{
    arrow_utils::make_empty_record_batch,
    call_tree::make_call_tree,
    metadata::{find_process, find_stream, find_stream_blocks_in_range},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::types::chrono::{DateTime, Utc};

pub async fn query_spans(
    data_lake: &DataLakeConnection,
    stream_id: &str,
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

    let _call_tree = make_call_tree(
        &blocks,
        relative_begin_ticks + process_info.start_ticks,
        relative_end_ticks + process_info.start_ticks,
        data_lake.blob_storage.clone(),
        convert_ticks,
        &stream_info,
    )
    .await
    .with_context(|| "make_call_tree")?;

    Ok(make_empty_record_batch())
}
