use crate::{
    measure::for_each_measure_in_block, metrics_table::MetricsRecordBuilder,
    time::make_time_converter_from_block_meta,
};

use super::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

#[derive(Debug)]
pub struct MetricsBlockProcessor {}

#[async_trait]
impl BlockProcessor for MetricsBlockProcessor {
    #[span_fn]
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let convert_ticks =
            make_time_converter_from_block_meta(&src_block.process, &src_block.block)?;
        let nb_measures = src_block.block.nb_objects;
        let mut record_builder = MetricsRecordBuilder::with_capacity(nb_measures as usize);
        let mut entry_count = 0;

        for_each_measure_in_block(
            blob_storage,
            &convert_ticks,
            src_block.process.clone(),
            &src_block.stream,
            &src_block.block,
            |measure| {
                record_builder.append_entry_only(&measure)?;
                entry_count += 1;
                Ok(true) // continue
            },
        )
        .await
        .with_context(|| "for_each_measure_in_block")?;

        if entry_count > 0 {
            let stream_id_str = format!("{}", src_block.stream.stream_id);
            let block_id_str = format!("{}", src_block.block.block_id);
            let insert_time_nanos = src_block
                .block
                .insert_time
                .timestamp_nanos_opt()
                .with_context(|| "converting insert_time to nanoseconds")?;

            record_builder.fill_constant_columns(
                &src_block.process,
                &stream_id_str,
                &block_id_str,
                insert_time_nanos,
                entry_count,
            )?;
        }

        if let Some(time_range) = record_builder.get_time_range() {
            let record_batch = record_builder.finish()?;
            Ok(Some(PartitionRowSet::new(time_range, record_batch)))
        } else {
            Ok(None)
        }
    }
}
