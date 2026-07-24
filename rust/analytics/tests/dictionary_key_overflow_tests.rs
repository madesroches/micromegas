//! Regression tests for the dictionary key overflow bug (issue #1341).
//!
//! `StringDictionaryBuilder<Int16Type>` panics once a single `RecordBatch` accumulates
//! more than 32,767 distinct values in one dictionary column (Arrow's
//! `GenericByteDictionaryBuilder::append_value()` calls `.expect("dictionary key overflow")`
//! internally). Widening the affected builders to `Int32Type` and switching their calls
//! from `append_value`/`append_values` to the fallible `append`/`append_n` fixes this.
//!
//! Each test below drives one of the affected builders past the old 32,767-value Int16
//! ceiling and asserts `finish()` succeeds instead of panicking — the exact scenario that
//! used to crash the background query task (see `thread_block_processor::parse_thread_block`
//! in production).

use micromegas_analytics::async_events_table::{AsyncEventRecord, AsyncEventRecordBuilder};
use micromegas_analytics::images_table::ImagesRecordBuilder;
use micromegas_analytics::lakehouse::block_partition_spec::BlockProcessor;
use micromegas_analytics::lakehouse::otel::logs_block_processor::OtelLogsBlockProcessor;
use micromegas_analytics::lakehouse::otel::metrics_block_processor::OtelMetricsBlockProcessor;
use micromegas_analytics::lakehouse::partition_source_data::PartitionSourceBlock;
use micromegas_analytics::log_entries_table::LogEntriesRecordBuilder;
use micromegas_analytics::log_entry::LogEntry;
use micromegas_analytics::measure::Measure;
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::metrics_table::MetricsRecordBuilder;
use micromegas_analytics::net_spans_table::{NetSpanRecord, NetSpanRecordBuilder};
use micromegas_analytics::properties::property_set::PropertySet;
use micromegas_analytics::span_table::{SpanRecordBuilder, SpanRow};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_telemetry::types::block::BlockMetadata;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;

mod test_helpers;
use test_helpers::make_process_metadata;

/// Comfortably above the old `Int16Type` dictionary-key ceiling (32,767).
const OVERFLOW_COUNT: usize = 33_000;

#[test]
fn span_record_builder_overflow() {
    let mut builder = SpanRecordBuilder::with_capacity(OVERFLOW_COUNT);
    for i in 0..OVERFLOW_COUNT {
        builder
            .append(SpanRow {
                id: i as i64,
                parent: -1,
                depth: 0,
                begin: i as i64,
                end: i as i64 + 1,
                hash: i as u32,
                name: Arc::new(format!("name-{i}")),
                target: Arc::new(format!("target-{i}")),
                filename: Arc::new(format!("filename-{i}")),
                line: 1,
            })
            .expect("append must not panic past the old Int16 dictionary cap");
    }
    let batch = builder
        .finish()
        .expect("finish must not panic past the old Int16 dictionary cap");
    assert_eq!(batch.num_rows(), OVERFLOW_COUNT);
}

#[test]
fn async_event_record_builder_overflow() {
    let mut builder = AsyncEventRecordBuilder::with_capacity(OVERFLOW_COUNT);
    let stream_id = Arc::new(String::from("stream-1"));
    let block_id = Arc::new(String::from("block-1"));
    for i in 0..OVERFLOW_COUNT {
        let name = format!("name-{i}");
        let filename = format!("filename-{i}");
        let target = format!("target-{i}");
        builder
            .append(&AsyncEventRecord {
                stream_id: stream_id.clone(),
                block_id: block_id.clone(),
                time: i as i64,
                event_type: "begin",
                span_id: i as i64,
                parent_span_id: -1,
                depth: 0,
                hash: i as u32,
                name: &name,
                filename: &filename,
                target: &target,
                line: 1,
            })
            .expect("append must not panic past the old Int16 dictionary cap");
    }
    let batch = builder
        .finish()
        .expect("finish must not panic past the old Int16 dictionary cap");
    assert_eq!(batch.num_rows(), OVERFLOW_COUNT);
}

#[test]
fn net_span_record_builder_overflow() {
    let mut builder = NetSpanRecordBuilder::with_capacity(OVERFLOW_COUNT);
    let process_id = Arc::new(String::from("proc-1"));
    let stream_id = Arc::new(String::from("stream-1"));
    for i in 0..OVERFLOW_COUNT {
        builder
            .append(&NetSpanRecord {
                process_id: process_id.clone(),
                stream_id: stream_id.clone(),
                span_id: i as i64,
                parent_span_id: -1,
                depth: 0,
                kind: Arc::new(String::from("connection")),
                name: Arc::new(format!("name-{i}")),
                connection_name: Arc::new(format!("connection-{i}")),
                is_outgoing: false,
                begin_bits: 0,
                end_bits: 0,
                bit_size: 0,
                begin_time: i as i64,
                end_time: i as i64 + 1,
            })
            .expect("append must not panic past the old Int16 dictionary cap");
    }
    let batch = builder
        .finish()
        .expect("finish must not panic past the old Int16 dictionary cap");
    assert_eq!(batch.num_rows(), OVERFLOW_COUNT);
}

#[test]
fn metrics_record_builder_overflow() {
    let process = Arc::new(make_process_metadata(
        uuid::Uuid::new_v4(),
        None,
        HashMap::new(),
    ));
    let mut builder = MetricsRecordBuilder::with_capacity(OVERFLOW_COUNT);
    for i in 0..OVERFLOW_COUNT {
        let name = format!("metric-{i}");
        let target = format!("target-{i}");
        let measure = Measure {
            process: process.clone(),
            stream_id: Arc::new(String::from("unused-stream")),
            block_id: Arc::new(String::from("unused-block")),
            insert_time: 0,
            time: i as i64,
            target: &target,
            name: &name,
            unit: "unit",
            value: i as f64,
            properties: PropertySet::empty(),
        };
        builder
            .append_entry_only(&measure)
            .expect("append_entry_only must not panic past the old Int16 dictionary cap");
        // Distinct block_id/stream_id per call: drives the exact columns the issue calls
        // out (many distinct blocks/streams merged into a single batch).
        let stream_id_str = format!("stream-{i}");
        let block_id_str = format!("block-{i}");
        builder
            .fill_constant_columns(&process, &stream_id_str, &block_id_str, 0, 1)
            .expect("fill_constant_columns must not panic past the old Int16 dictionary cap");
    }
    let batch = builder
        .finish()
        .expect("finish must not panic past the old Int16 dictionary cap");
    assert_eq!(batch.num_rows(), OVERFLOW_COUNT);
}

#[test]
fn log_entries_record_builder_overflow() {
    let process = Arc::new(make_process_metadata(
        uuid::Uuid::new_v4(),
        None,
        HashMap::new(),
    ));
    let mut builder = LogEntriesRecordBuilder::with_capacity(OVERFLOW_COUNT);
    for i in 0..OVERFLOW_COUNT {
        let target = format!("target-{i}");
        let msg = format!("msg-{i}");
        let entry = LogEntry {
            process: process.clone(),
            stream_id: Arc::new(String::from("unused-stream")),
            block_id: Arc::new(String::from("unused-block")),
            insert_time: 0,
            time: i as i64,
            level: 4,
            target: &target,
            msg: &msg,
            properties: PropertySet::empty(),
        };
        builder
            .append_entry_only(&entry)
            .expect("append_entry_only must not panic past the old Int16 dictionary cap");
        // Distinct block_id/stream_id per call: drives the exact columns the issue calls
        // out (many distinct blocks/streams merged into a single batch).
        let stream_id_str = format!("stream-{i}");
        let block_id_str = format!("block-{i}");
        builder
            .fill_constant_columns(&process, &stream_id_str, &block_id_str, 0, 1)
            .expect("fill_constant_columns must not panic past the old Int16 dictionary cap");
    }
    let batch = builder
        .finish()
        .expect("finish must not panic past the old Int16 dictionary cap");
    assert_eq!(batch.num_rows(), OVERFLOW_COUNT);
}

#[test]
fn images_record_builder_overflow() {
    let process = Arc::new(make_process_metadata(
        uuid::Uuid::new_v4(),
        None,
        HashMap::new(),
    ));
    let mut builder = ImagesRecordBuilder::new();
    for i in 0..OVERFLOW_COUNT {
        let block_id_str = format!("block-{i}");
        let format_str = format!("format-{i}");
        builder
            .append(
                &process,
                "process-1",
                "stream-1",
                &block_id_str,
                0,
                i as i64,
                "image.png",
                &format_str,
                0,
                &[],
            )
            .expect("append must not panic past the old Int16 dictionary cap");
    }
    let batch = builder
        .finish()
        .expect("finish must not panic past the old Int16 dictionary cap");
    assert_eq!(batch.num_rows(), OVERFLOW_COUNT);
}

/// Builds an in-memory `BlobStorage` with no path prefix, so `blobs/{process_id}/...`
/// paths written directly onto the store are exactly what `fetch_block_payload` reads.
fn make_in_memory_blob_storage() -> Arc<BlobStorage> {
    Arc::new(BlobStorage::new(
        Arc::new(object_store::memory::InMemory::new()),
        object_store::path::Path::from(""),
    ))
}

/// Builds a `PartitionSourceBlock` for a single block carrying `payload_bytes` under a
/// fresh random process/stream/block id, and writes the CBOR-wrapped `BlockPayload` to
/// `blob_storage` at the path `fetch_block_payload` expects.
async fn make_source_block(
    blob_storage: &BlobStorage,
    payload_bytes: Vec<u8>,
    nb_objects: usize,
    format: &str,
) -> anyhow::Result<Arc<PartitionSourceBlock>> {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let block_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();

    let block_payload = BlockPayload {
        dependencies: vec![],
        objects: payload_bytes,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&block_payload, &mut buf)?;
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
    blob_storage.put(&obj_path, buf.into()).await?;

    let block = BlockMetadata {
        block_id,
        stream_id,
        process_id,
        begin_time: now,
        end_time: now,
        begin_ticks: 0,
        end_ticks: 0,
        nb_objects: nb_objects as i32,
        payload_size: block_payload.objects.len() as i64,
        object_offset: 0,
        insert_time: now,
    };
    let stream = Arc::new(StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(make_process_metadata(process_id, None, HashMap::new()));
    Ok(Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: format.to_string(),
    }))
}

/// The "mandatory companions" fix (plan §"Companion OTLP processors"): OTel processors
/// build their own local `Int16Type` dictionary builders instead of reusing
/// `LogEntriesRecordBuilder`/`MetricsRecordBuilder`, so they need the exact same widening.
/// Drives one distinct scope (→ `target`) per log record, past the old Int16 cap.
#[tokio::test]
async fn otel_logs_block_processor_survives_target_dictionary_overflow() {
    let mut scope_logs = Vec::with_capacity(OVERFLOW_COUNT);
    for i in 0..OVERFLOW_COUNT {
        scope_logs.push(ScopeLogs {
            scope: Some(InstrumentationScope {
                name: format!("scope-{i}"),
                version: String::new(),
                attributes: vec![],
                dropped_attributes_count: 0,
            }),
            log_records: vec![LogRecord {
                time_unix_nano: i as u64 + 1,
                observed_time_unix_nano: 0,
                severity_number: 9,
                severity_text: String::new(),
                body: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(format!("msg-{i}"))),
                }),
                attributes: vec![],
                dropped_attributes_count: 0,
                flags: 0,
                trace_id: vec![],
                span_id: vec![],
                event_name: String::new(),
            }],
            schema_url: String::new(),
        });
    }
    let resource_logs = ResourceLogs {
        resource: None,
        scope_logs,
        schema_url: String::new(),
    };
    let payload_bytes = resource_logs.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(&blob_storage, payload_bytes, OVERFLOW_COUNT, "otlp/v1/logs")
        .await
        .expect("make_source_block");

    let processor = OtelLogsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not panic past the old Int16 dictionary cap");
    let row_set = result.expect("expected Some(row_set) for a non-empty block");
    assert_eq!(row_set.rows.num_rows(), OVERFLOW_COUNT);
}

/// Same as `otel_logs_block_processor_survives_target_dictionary_overflow`, but for
/// `OtelMetricsBlockProcessor`/`MeasuresRowBuilder` (Gauge data points), proving the
/// `MeasuresRowBuilder::append` signature change (`()` → `Result<()>`) actually propagates
/// the overflow instead of panicking.
#[tokio::test]
async fn otel_metrics_block_processor_survives_target_dictionary_overflow() {
    let mut scope_metrics = Vec::with_capacity(OVERFLOW_COUNT);
    for i in 0..OVERFLOW_COUNT {
        scope_metrics.push(ScopeMetrics {
            scope: Some(InstrumentationScope {
                name: format!("scope-{i}"),
                version: String::new(),
                attributes: vec![],
                dropped_attributes_count: 0,
            }),
            metrics: vec![Metric {
                name: format!("metric-{i}"),
                description: String::new(),
                unit: "unit".to_string(),
                metadata: vec![],
                data: Some(metric::Data::Gauge(Gauge {
                    data_points: vec![NumberDataPoint {
                        attributes: vec![],
                        start_time_unix_nano: 0,
                        time_unix_nano: i as u64 + 1,
                        exemplars: vec![],
                        flags: 0,
                        value: Some(number_data_point::Value::AsDouble(i as f64)),
                    }],
                })),
            }],
            schema_url: String::new(),
        });
    }
    let resource_metrics = ResourceMetrics {
        resource: None,
        scope_metrics,
        schema_url: String::new(),
    };
    let payload_bytes = resource_metrics.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(
        &blob_storage,
        payload_bytes,
        OVERFLOW_COUNT,
        "otlp/v1/metrics",
    )
    .await
    .expect("make_source_block");

    let processor = OtelMetricsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not panic past the old Int16 dictionary cap");
    let row_set = result.expect("expected Some(row_set) for a non-empty block");
    assert_eq!(row_set.rows.num_rows(), OVERFLOW_COUNT);
}
