use crate::{
    measure::for_each_measure_in_block, metrics_table::MetricsRecordBuilder, time::ConvertTicks,
};

use super::{
    block_partition_spec::{BlockProcessor, PartitionRowSet},
    partition_source_data::PartitionSourceBlock,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

pub struct MetricsBlockProcessor {}

#[async_trait]
impl BlockProcessor for MetricsBlockProcessor {
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
        let nb_measures = src_block.block.nb_objects;
        let mut record_builder = MetricsRecordBuilder::with_capacity(nb_measures as usize);

        for_each_measure_in_block(
            blob_storage,
            &convert_ticks,
            src_block.process.clone(),
            &src_block.stream,
            &src_block.block,
            |measure| {
                record_builder.append(&measure)?;
                Ok(true) // continue
            },
        )
        .await
        .with_context(|| "for_each_measure_in_block")?;

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
