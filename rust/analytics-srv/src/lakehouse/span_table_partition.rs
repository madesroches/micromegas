use anyhow::{Context, Result};
use lgn_analytics::time::ConvertTicks;
use lgn_telemetry_proto::telemetry::BlockMetadata;
use lgn_telemetry_proto::telemetry::Stream;
use lgn_tracing::prelude::*;
use std::collections::HashMap;
use std::path::Path;

use super::span_table::{make_rows_from_tree, write_spans_parquet, SpanRowGroup};
use crate::call_tree::CallTreeBuilder;
use crate::thread_block_processor::parse_thread_block_payload;

#[allow(clippy::cast_possible_wrap)]
#[span_fn]
pub async fn write_local_partition(
    payload: &lgn_telemetry_proto::telemetry::BlockPayload,
    stream: &Stream,
    block: &BlockMetadata,
    convert_ticks: ConvertTicks,
    next_id: &mut i64,
    relative_file_name: String,
    parquet_full_path: &Path,
) -> Result<Option<deltalake::action::Action>> {
    //todo: do not allow overwriting - it could break id generation
    info!("processing block {}", &block.block_id);
    if let Some(parent) = parquet_full_path.parent() {
        tokio::fs::create_dir_all(&parent)
            .await
            .with_context(|| format!("creating directory for {}", parquet_full_path.display()))?;
    }
    let mut builder = CallTreeBuilder::new(block.begin_ticks, block.end_ticks, convert_ticks);
    parse_thread_block_payload(payload, stream, &mut builder)
        .with_context(|| "parsing thread block payload")?;
    let processed_block = builder.finish();
    if let Some(root) = processed_block.call_tree_root {
        let mut rows = SpanRowGroup::new();
        make_rows_from_tree(&root, next_id, &mut rows);
        write_spans_parquet(&rows, parquet_full_path).await?;

        let attr = tokio::fs::metadata(&parquet_full_path).await?; //that's not cool, we should already know how big the file is
        Ok(Some(deltalake::action::Action::add(
            deltalake::action::Add {
                path: relative_file_name,
                size: attr.len() as i64,
                partition_values: HashMap::from([
                    ("block_id".to_owned(), Some(block.block_id.clone())),
                    ("thread_id".to_owned(), Some(stream.stream_id.clone())),
                ]),
                partition_values_parsed: None,
                modification_time: 0,
                data_change: false,
                stats: None,
                stats_parsed: None,
                tags: None,
            },
        )))
    } else {
        Ok(None)
    }
}
