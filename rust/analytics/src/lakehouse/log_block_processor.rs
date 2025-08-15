use super::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::{
    log_entries_table::LogEntriesRecordBuilder, log_entry::for_each_log_entry_in_block,
    time::make_time_converter_from_block_meta,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A `BlockProcessor` implementation for processing log blocks.
#[derive(Debug)]
pub struct LogBlockProcessor {}

#[async_trait]
impl BlockProcessor for LogBlockProcessor {
    #[span_fn]
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let convert_ticks =
            make_time_converter_from_block_meta(&src_block.process, &src_block.block)?;
        let nb_log_entries = src_block.block.nb_objects;
        let mut record_builder = LogEntriesRecordBuilder::with_capacity(nb_log_entries as usize);

        for_each_log_entry_in_block(
            blob_storage,
            &convert_ticks,
            src_block.process.clone(),
            &src_block.stream,
            &src_block.block,
            |log_entry| {
                record_builder.append(&log_entry)?;
                Ok(true) // continue
            },
        )
        .await
        .with_context(|| "for_each_log_entry_in_block")?;

        if let Some(time_range) = record_builder.get_time_range() {
            let record_batch = record_builder.finish()?;
            Ok(Some(PartitionRowSet {
                rows_time_range: time_range,
                rows: record_batch,
            }))
        } else {
            Ok(None)
        }
    }
}
