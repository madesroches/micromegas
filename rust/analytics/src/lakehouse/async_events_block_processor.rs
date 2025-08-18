use super::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::{
    async_block_processing::{AsyncBlockProcessor, parse_async_block_payload},
    async_events_table::{AsyncEventRecord, AsyncEventRecordBuilder},
    payload::fetch_block_payload,
    scope::ScopeDesc,
    time::{ConvertTicks, make_time_converter_from_block_meta},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

lazy_static::lazy_static! {
    static ref BEGIN_EVENT_TYPE: Arc<String> = Arc::new("begin".to_string());
    static ref END_EVENT_TYPE: Arc<String> = Arc::new("end".to_string());
}

/// A `BlockProcessor` implementation for processing async event blocks.
#[derive(Debug)]
pub struct AsyncEventsBlockProcessor {}

/// Helper struct to collect async events during processing.
struct AsyncEventCollector {
    record_builder: AsyncEventRecordBuilder,
    stream_id: Arc<String>,
    block_id: Arc<String>,
    convert_ticks: Arc<ConvertTicks>,
}

impl AsyncEventCollector {
    fn new(
        capacity: usize,
        stream_id: Arc<String>,
        block_id: Arc<String>,
        convert_ticks: Arc<ConvertTicks>,
    ) -> Self {
        Self {
            record_builder: AsyncEventRecordBuilder::with_capacity(capacity),
            stream_id,
            block_id,
            convert_ticks,
        }
    }
}

impl AsyncBlockProcessor for AsyncEventCollector {
    fn on_begin_async_scope(
        &mut self,
        _block_id: &str,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
    ) -> Result<bool> {
        let time_ns = self.convert_ticks.ticks_to_nanoseconds(ts);
        let record = AsyncEventRecord {
            stream_id: self.stream_id.clone(),
            block_id: self.block_id.clone(),
            time: time_ns,
            event_type: BEGIN_EVENT_TYPE.clone(),
            span_id,
            parent_span_id,
            name: scope.name,
            filename: scope.filename,
            target: scope.target,
            line: scope.line,
        };
        self.record_builder.append(&record)?;
        Ok(true)
    }

    fn on_end_async_scope(
        &mut self,
        _block_id: &str,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
    ) -> Result<bool> {
        let time_ns = self.convert_ticks.ticks_to_nanoseconds(ts);
        let record = AsyncEventRecord {
            stream_id: self.stream_id.clone(),
            block_id: self.block_id.clone(),
            time: time_ns,
            event_type: END_EVENT_TYPE.clone(),
            span_id,
            parent_span_id,
            name: scope.name,
            filename: scope.filename,
            target: scope.target,
            line: scope.line,
        };
        self.record_builder.append(&record)?;
        Ok(true)
    }
}

#[async_trait]
impl BlockProcessor for AsyncEventsBlockProcessor {
    // #[span_fn]  // Temporarily disabled to test
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let convert_ticks =
            make_time_converter_from_block_meta(&src_block.process, &src_block.block)?;
        // Use nb_objects as initial capacity estimate (may contain non-async events)
        let estimated_capacity = src_block.block.nb_objects;
        let mut collector = AsyncEventCollector::new(
            estimated_capacity as usize,
            Arc::new(format!("{}", src_block.stream.stream_id)),
            Arc::new(format!("{}", src_block.block.block_id)),
            Arc::new(convert_ticks),
        );
        let payload = fetch_block_payload(
            blob_storage,
            src_block.process.process_id,
            src_block.stream.stream_id,
            src_block.block.block_id,
        )
        .await
        .with_context(|| "fetch_block_payload")?;
        let block_id_str = src_block
            .block
            .block_id
            .hyphenated()
            .encode_lower(&mut sqlx::types::uuid::Uuid::encode_buffer())
            .to_owned();
        parse_async_block_payload(
            &block_id_str,
            0,
            &payload,
            &src_block.stream,
            &mut collector,
        )
        .with_context(|| "parse_async_block_payload")?;
        if let Some(time_range) = collector.record_builder.get_time_range() {
            let record_batch = collector.record_builder.finish()?;
            Ok(Some(PartitionRowSet {
                rows_time_range: time_range,
                rows: record_batch,
            }))
        } else {
            Ok(None)
        }
    }
}
