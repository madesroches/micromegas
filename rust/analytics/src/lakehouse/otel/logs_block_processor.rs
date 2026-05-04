//! `BlockProcessor` for OTLP `ResourceLogs` payloads → `log_entries` rows.
//!
//! The block payload carries a single `ResourceLogs` message (one resource per block —
//! see Plan §"One block per Resource"). We prost-decode it, walk each scope and log
//! record, emit one row per `LogRecord`.

use super::attrs::{any_value_to_string, attrs_to_jsonb, scope_extras, severity_number_to_level};
use crate::lakehouse::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::payload::fetch_block_payload;
use crate::time::TimeRange;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::arrow::array::{
    BinaryDictionaryBuilder, PrimitiveBuilder, StringBuilder, StringDictionaryBuilder,
};
use datafusion::arrow::datatypes::{Int16Type, Int32Type, TimestampNanosecondType};
use datafusion::arrow::record_batch::RecordBatch;
use jsonb::Value as JsonbValue;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use opentelemetry_proto::tonic::logs::v1::ResourceLogs;
use prost::Message;
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Debug)]
pub struct OtelLogsBlockProcessor {}

#[async_trait]
impl BlockProcessor for OtelLogsBlockProcessor {
    #[span_fn]
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let payload = fetch_block_payload(
            blob_storage,
            sqlx::types::Uuid::from_bytes(*src_block.block.process_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*src_block.block.stream_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*src_block.block.block_id.as_bytes()),
        )
        .await
        .with_context(|| "fetch_block_payload")?;

        // Block payload format: BlockPayload { dependencies: [], objects: <ResourceLogs proto> }.
        // (`fetch_block_payload` already CBOR-decodes the envelope; we get the proto bytes.)
        let resource_logs = ResourceLogs::decode(payload.objects.as_slice())
            .with_context(|| "decoding ResourceLogs proto")?;

        let process = &src_block.process;
        let stream_id_str = src_block.block.stream_id.to_string();
        let block_id_str = src_block.block.block_id.to_string();
        let process_id_str = process.process_id.to_string();
        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "block.insert_time → nanos")?;

        // Pre-count rows so dictionary builders can size their backing storage.
        let row_count: usize = resource_logs
            .scope_logs
            .iter()
            .map(|s| s.log_records.len())
            .sum();
        if row_count == 0 {
            return Ok(None);
        }

        let mut process_ids = StringDictionaryBuilder::<Int16Type>::new();
        let mut stream_ids = StringDictionaryBuilder::<Int16Type>::new();
        let mut block_ids = StringDictionaryBuilder::<Int16Type>::new();
        let mut insert_times =
            PrimitiveBuilder::<TimestampNanosecondType>::with_capacity(row_count);
        let mut exes = StringDictionaryBuilder::<Int16Type>::new();
        let mut usernames = StringDictionaryBuilder::<Int16Type>::new();
        let mut computers = StringDictionaryBuilder::<Int16Type>::new();
        let mut times = PrimitiveBuilder::<TimestampNanosecondType>::with_capacity(row_count);
        let mut targets = StringDictionaryBuilder::<Int16Type>::new();
        let mut levels = PrimitiveBuilder::<Int32Type>::with_capacity(row_count);
        let mut msgs = StringBuilder::new();
        let mut properties = BinaryDictionaryBuilder::<Int32Type>::new();
        let mut process_properties = BinaryDictionaryBuilder::<Int32Type>::new();

        let mut min_time = i64::MAX;
        let mut max_time = i64::MIN;
        let mut nb_appended = 0usize;
        let mut nb_dropped_no_timestamp = 0usize;
        let mut nb_severity_out_of_range = 0usize;

        for scope_logs in &resource_logs.scope_logs {
            let scope = scope_logs.scope.as_ref();
            // `target` mirrors the existing native semantics (logger name).
            let scope_name = scope.map(|s| s.name.clone()).unwrap_or_default();

            for record in &scope_logs.log_records {
                // time_unix_nano is optional — fall back to observed_time per OTLP spec.
                let time_nanos = if record.time_unix_nano != 0 {
                    record.time_unix_nano as i64
                } else if record.observed_time_unix_nano != 0 {
                    record.observed_time_unix_nano as i64
                } else {
                    // No timestamp at all — skip so it doesn't anchor the partition
                    // at 1970-01-01. Aggregated below to one log line per block.
                    nb_dropped_no_timestamp += 1;
                    continue;
                };
                min_time = min_time.min(time_nanos);
                max_time = max_time.max(time_nanos);

                let level = severity_number_to_level(record.severity_number);
                if !(0..=24).contains(&record.severity_number) {
                    nb_severity_out_of_range += 1;
                }

                // Body → msg. String body lands directly; structured body gets stringified
                // (deferred parsing — see the plan's "Logs → log_entries" mapping table).
                let msg = record
                    .body
                    .as_ref()
                    .map(any_value_to_string)
                    .unwrap_or_default();

                // Properties: log record attributes + scope info (otel.scope.*) +
                // optional trace correlation + raw severity_text. Built per-row because
                // OTel attributes vary record-to-record; dictionary dedup happens inside
                // BinaryDictionaryBuilder by content hash.
                let mut extras = scope_extras(scope, &scope_logs.schema_url);
                // W3C Trace Context: trace_id is 16 bytes, span_id is 8 bytes.
                // `otel_spans` enforces these lengths and skips bad rows; we mirror
                // that here so a buggy SDK can't write half-size hex strings that
                // silently fail correlation joins against the spans view.
                if !record.trace_id.is_empty() {
                    if record.trace_id.len() == 16 {
                        let hex = hex::encode(&record.trace_id);
                        extras.push((
                            "otel.trace_id".to_string(),
                            JsonbValue::String(Cow::Owned(hex)),
                        ));
                    } else {
                        debug!(
                            "OTel log record with bad trace_id ({}b), dropping property",
                            record.trace_id.len()
                        );
                    }
                }
                if !record.span_id.is_empty() {
                    if record.span_id.len() == 8 {
                        let hex = hex::encode(&record.span_id);
                        extras.push((
                            "otel.span_id".to_string(),
                            JsonbValue::String(Cow::Owned(hex)),
                        ));
                    } else {
                        debug!(
                            "OTel log record with bad span_id ({}b), dropping property",
                            record.span_id.len()
                        );
                    }
                }
                if !record.severity_text.is_empty() {
                    extras.push((
                        "otel.severity_text".to_string(),
                        JsonbValue::String(Cow::Owned(record.severity_text.clone())),
                    ));
                }
                if !record.event_name.is_empty() {
                    extras.push((
                        "otel.event_name".to_string(),
                        JsonbValue::String(Cow::Owned(record.event_name.clone())),
                    ));
                }

                let props_jsonb = attrs_to_jsonb(&record.attributes, &extras);

                process_ids.append_value(&process_id_str);
                stream_ids.append_value(&stream_id_str);
                block_ids.append_value(&block_id_str);
                insert_times.append_value(insert_time_nanos);
                exes.append_value(&process.exe);
                usernames.append_value(&process.username);
                computers.append_value(&process.computer);
                times.append_value(time_nanos);
                targets.append_value(&scope_name);
                levels.append_value(level);
                msgs.append_value(&msg);
                properties.append_value(&props_jsonb);
                process_properties.append_value(&**process.properties);

                nb_appended += 1;
            }
        }

        if nb_dropped_no_timestamp > 0 {
            debug!(
                "OTel log records without timestamp dropped (block_id={block_id_str}, count={nb_dropped_no_timestamp})"
            );
        }
        if nb_severity_out_of_range > 0 {
            debug!(
                "OTel log records with out-of-range severity_number treated as Info (block_id={block_id_str}, count={nb_severity_out_of_range})"
            );
        }

        if nb_appended == 0 {
            return Ok(None);
        }

        let schema = Arc::new(crate::log_entries_table::log_table_schema());
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(process_ids.finish()),
                Arc::new(stream_ids.finish()),
                Arc::new(block_ids.finish()),
                Arc::new(insert_times.finish().with_timezone_utc()),
                Arc::new(exes.finish()),
                Arc::new(usernames.finish()),
                Arc::new(computers.finish()),
                Arc::new(times.finish().with_timezone_utc()),
                Arc::new(targets.finish()),
                Arc::new(levels.finish()),
                Arc::new(msgs.finish()),
                Arc::new(properties.finish()),
                Arc::new(process_properties.finish()),
            ],
        )
        .with_context(|| "building OTel log_entries batch")?;

        Ok(Some(PartitionRowSet::new(
            TimeRange::new(
                DateTime::from_timestamp_nanos(min_time),
                DateTime::from_timestamp_nanos(max_time),
            ),
            batch,
        )))
    }
}
