use super::{
    block_partition_spec::{BlockProcessor, PartitionRowSet},
    partition_source_data::PartitionSourceBlock,
};
use crate::{
    log_entries_table::LogEntriesRecordBuilder, log_entry::for_each_log_entry_in_block,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

pub struct LogBlockProcessor {}

#[async_trait]
impl BlockProcessor for LogBlockProcessor {
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let convert_ticks = ConvertTicks::from_meta_data(
            src_block.process.start_ticks,
            src_block
                .process
                .start_time
                .timestamp_nanos_opt()
                .unwrap_or_default(),
            src_block.process.tsc_frequency,
        );
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
                min_time_row: time_range.0,
                max_time_row: time_range.1,
                rows: record_batch,
            }))
        } else {
            Ok(None)
        }
    }
}
