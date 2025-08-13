use super::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::{
    async_events_table::{AsyncEventRecord, AsyncEventRecordBuilder},
    payload::fetch_block_payload,
    scope::ScopeDesc,
    thread_block_processor::{AsyncBlockProcessor, parse_async_block_payload},
    time::make_time_converter_from_block_meta,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::process_info::ProcessInfo;
use std::sync::Arc;

/// A `BlockProcessor` implementation for processing async event blocks.
#[derive(Debug)]
pub struct AsyncEventsBlockProcessor {}

/// Helper struct to collect async events during processing.
struct AsyncEventCollector {
    record_builder: AsyncEventRecordBuilder,
    process: Arc<ProcessInfo>,
    stream_id: Arc<String>,
    block_id: Arc<String>,
    insert_time: i64,
}

impl AsyncEventCollector {
    fn new(
        capacity: usize,
        process: Arc<ProcessInfo>,
        stream_id: Arc<String>,
        block_id: Arc<String>,
        insert_time: i64,
    ) -> Self {
        Self {
            record_builder: AsyncEventRecordBuilder::with_capacity(capacity),
            process,
            stream_id,
            block_id,
            insert_time,
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
        let record = AsyncEventRecord {
            process_id: self.process.process_id,
            stream_id: self.stream_id.clone(),
            block_id: self.block_id.clone(),
            insert_time: self.insert_time,
            exe: self.process.exe.clone().into(),
            username: self.process.username.clone().into(),
            computer: self.process.computer.clone().into(),
            time: ts,
            event_type: Arc::new("begin".to_string()),
            span_id,
            parent_span_id,
            name: scope.name,
            filename: scope.filename,
            target: scope.target,
            line: scope.line,
            process_properties: self.process.properties.clone(),
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
        let record = AsyncEventRecord {
            process_id: self.process.process_id,
            stream_id: self.stream_id.clone(),
            block_id: self.block_id.clone(),
            insert_time: self.insert_time,
            exe: self.process.exe.clone().into(),
            username: self.process.username.clone().into(),
            computer: self.process.computer.clone().into(),
            time: ts,
            event_type: Arc::new("end".to_string()),
            span_id,
            parent_span_id,
            name: scope.name,
            filename: scope.filename,
            target: scope.target,
            line: scope.line,
            process_properties: self.process.properties.clone(),
        };
        self.record_builder.append(&record)?;
        Ok(true)
    }
}

#[async_trait]
impl BlockProcessor for AsyncEventsBlockProcessor {
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let _convert_ticks =
            make_time_converter_from_block_meta(&src_block.process, &src_block.block)?;
        let nb_async_events = src_block.block.nb_objects;

        let mut collector = AsyncEventCollector::new(
            nb_async_events as usize,
            src_block.process.clone(),
            Arc::new(format!("{}", src_block.stream.stream_id)),
            Arc::new(format!("{}", src_block.block.block_id)),
            src_block
                .block
                .insert_time
                .timestamp_nanos_opt()
                .unwrap_or_default(),
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
