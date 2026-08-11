//! Regression test for the read-side half of #1465: after ingestion stopped backfilling
//! `observed_time_unix_nano` (see `rust/otel-ingestion/src/block.rs`), an OTel log record
//! with neither `time_unix_nano` nor `observed_time_unix_nano` set must still materialize a
//! row — at the block's `begin_time` — instead of being dropped. See the `time_nanos`
//! fallback in `OtelLogsBlockProcessor::process` (`logs_block_processor.rs`).

use datafusion::arrow::array::TimestampNanosecondArray;
use micromegas_analytics::lakehouse::block_partition_spec::BlockProcessor;
use micromegas_analytics::lakehouse::otel::logs_block_processor::OtelLogsBlockProcessor;
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use prost::Message;

mod test_helpers;
use test_helpers::{make_in_memory_blob_storage, make_source_block};

#[tokio::test]
async fn zero_timestamp_log_record_materializes_at_block_begin_time() {
    let resource_logs = ResourceLogs {
        resource: None,
        scope_logs: vec![ScopeLogs {
            scope: Some(InstrumentationScope {
                name: "scope-1".to_string(),
                version: String::new(),
                attributes: vec![],
                dropped_attributes_count: 0,
            }),
            log_records: vec![LogRecord {
                time_unix_nano: 0,
                observed_time_unix_nano: 0,
                severity_number: 9,
                severity_text: String::new(),
                body: None,
                attributes: vec![],
                dropped_attributes_count: 0,
                flags: 0,
                trace_id: vec![],
                span_id: vec![],
                event_name: String::new(),
            }],
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    };
    let payload_bytes = resource_logs.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(&blob_storage, payload_bytes, 1, "otlp/v1/logs")
        .await
        .expect("make_source_block");
    let expected_time_nanos = src_block
        .block
        .begin_time
        .timestamp_nanos_opt()
        .expect("begin_time -> nanos");

    let processor = OtelLogsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not error on a zero-timestamp record");
    let row_set = result.expect("a zero-timestamp record must materialize a row, not be dropped");
    assert_eq!(row_set.rows.num_rows(), 1);

    let times = row_set
        .rows
        .column_by_name("time")
        .expect("time column")
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .expect("time column is TimestampNanosecondArray");
    assert_eq!(times.value(0), expected_time_nanos);
}
