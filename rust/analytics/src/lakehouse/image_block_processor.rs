use super::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::{
    images_table::ImagesRecordBuilder,
    payload::{fetch_block_payload, parse_block},
    time::make_time_converter_from_block_meta,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use micromegas_transit::value::Value;
use std::sync::Arc;

#[derive(Debug)]
pub struct ImageBlockProcessor {}

#[async_trait]
impl BlockProcessor for ImageBlockProcessor {
    #[span_fn]
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let convert_ticks =
            make_time_converter_from_block_meta(&src_block.process, &src_block.block)?;
        let payload = fetch_block_payload(
            blob_storage,
            src_block.process.process_id,
            src_block.stream.stream_id,
            src_block.block.block_id,
        )
        .await
        .with_context(|| "fetch_block_payload")?;

        let process_id_str = format!("{}", src_block.process.process_id);
        let stream_id_str = format!("{}", src_block.stream.stream_id);
        let block_id_str = format!("{}", src_block.block.block_id);
        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "converting insert_time to nanoseconds")?;

        let mut record_builder = ImagesRecordBuilder::new();

        parse_block(&src_block.stream, &payload, |val| {
            if let Value::Object(obj) = &val
                && obj.type_name.as_str() == "ImageEvent"
            {
                let ticks = obj.get::<i64>("time").with_context(|| "reading time")?;
                let name = obj
                    .get::<Arc<String>>("name")
                    .with_context(|| "reading name")?;
                let format = obj
                    .get::<Arc<String>>("format")
                    .with_context(|| "reading format")?;
                let image_data = obj
                    .get::<Arc<Vec<u8>>>("data")
                    .with_context(|| "reading data")?;
                let time_ns = convert_ticks.ticks_to_nanoseconds(ticks);
                let payload_size = image_data.len() as i64;
                record_builder.append(
                    &src_block.process,
                    &process_id_str,
                    &stream_id_str,
                    &block_id_str,
                    insert_time_nanos,
                    time_ns,
                    &name,
                    &format,
                    payload_size,
                    &image_data,
                )?;
            }
            Ok(true)
        })
        .with_context(|| "parse_block")?;

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
