use std::cmp::{max, min};

use crate::{
    call_tree::make_call_tree,
    metadata::{find_process, find_stream, find_stream_blocks_in_range},
    span_table::SpanRecordBuilder,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
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
            let begin_call_tree = blocks_to_process[0].begin_ticks;
            let end_call_tree = min(
                relative_end_ticks,
                blocks_to_process[blocks_to_process.len() - 1].end_ticks,
            );
            let call_tree = make_call_tree(
                &blocks_to_process,
                begin_call_tree + process_info.start_ticks,
                end_call_tree + process_info.start_ticks,
                limit, //todo
                data_lake.blob_storage.clone(),
                convert_ticks.clone(),
                &stream_info,
            )
            .await
            .with_context(|| "make_call_tree")?;
            blocks_to_process = vec![];
			last_end = None;
            record_builder
                .append_call_tree(&call_tree)
                .with_context(|| "adding call tree to span record builder")?;
        }
    }

    if !blocks_to_process.is_empty() {
        //todo factorize
        let begin_call_tree = blocks_to_process[0].begin_ticks;
        let end_call_tree = min(
            relative_end_ticks,
            blocks_to_process[blocks_to_process.len() - 1].end_ticks,
        );
        let call_tree = make_call_tree(
            &blocks_to_process,
            begin_call_tree + process_info.start_ticks,
            end_call_tree + process_info.start_ticks,
            limit, //todo
            data_lake.blob_storage.clone(),
            convert_ticks.clone(),
            &stream_info,
        )
        .await
        .with_context(|| "make_call_tree")?;
        drop(blocks_to_process);
        record_builder
            .append_call_tree(&call_tree)
            .with_context(|| "adding call tree to span record builder")?;
    }

    record_builder
        .finish()
        .with_context(|| "finalizing span record builder")
}
