//! Pure, no-DB unit tests for `insert_time_range` and `ensure_begin_non_decreasing`
//! (tasks/1429_jit_event_time_block_ordering_plan.md, Testing Strategy #9-10). Both targets are
//! `pub` so this integration-test crate can call them directly.

use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::{RecordBatch, TimestampNanosecondArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use micromegas_analytics::lakehouse::jit_partitions::insert_time_range;
use micromegas_analytics::lakehouse::partition_source_data::PartitionSourceBlock;
use micromegas_analytics::lakehouse::thread_spans_view::ensure_begin_non_decreasing;
use micromegas_analytics::metadata::{ProcessMetadata, StreamMetadata};
use micromegas_telemetry::types::block::BlockMetadata;
use std::sync::Arc;

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .to_utc()
}

fn make_block(insert_time: DateTime<Utc>) -> Arc<PartitionSourceBlock> {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let block = BlockMetadata {
        block_id: uuid::Uuid::new_v4(),
        stream_id,
        process_id,
        begin_time: insert_time,
        end_time: insert_time,
        begin_ticks: 0,
        end_ticks: 100,
        nb_objects: 1,
        payload_size: 0,
        object_offset: 0,
        insert_time,
    };
    let stream = Arc::new(StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(ProcessMetadata {
        process_id,
        exe: "test".to_owned(),
        username: "test".to_owned(),
        realname: "test".to_owned(),
        computer: "test".to_owned(),
        distro: "test".to_owned(),
        cpu_brand: "test".to_owned(),
        tsc_frequency: 1_000_000_000,
        start_time: insert_time,
        start_ticks: 0,
        parent_process_id: None,
        properties: Arc::new(vec![]),
    });
    Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: "test".to_owned(),
    })
}

/// Test #9: `insert_time_range` over a permuted list returns the true min/max, and equals the
/// endpoints for an already-sorted list.
#[test]
fn insert_time_range_returns_true_min_max() {
    let t0 = base_time();
    // Permuted: neither the first nor the last element is the true min/max.
    let blocks = vec![
        make_block(t0 + TimeDelta::seconds(5)),
        make_block(t0 + TimeDelta::seconds(1)),
        make_block(t0 + TimeDelta::seconds(9)),
        make_block(t0 + TimeDelta::seconds(3)),
    ];
    let range = insert_time_range(&blocks).expect("non-empty");
    assert_eq!(range.begin, t0 + TimeDelta::seconds(1));
    assert_eq!(range.end, t0 + TimeDelta::seconds(9));

    // Already sorted: min/max equal the endpoints.
    let sorted_blocks = vec![
        make_block(t0),
        make_block(t0 + TimeDelta::seconds(1)),
        make_block(t0 + TimeDelta::seconds(2)),
    ];
    let range = insert_time_range(&sorted_blocks).expect("non-empty");
    assert_eq!(range.begin, sorted_blocks[0].block.insert_time);
    assert_eq!(
        range.end,
        sorted_blocks[sorted_blocks.len() - 1].block.insert_time
    );
}

#[test]
fn insert_time_range_rejects_empty_list() {
    assert!(insert_time_range(&[]).is_err());
}

fn begin_batch(begin_values: Vec<i64>) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "begin",
        DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
        false,
    )]));
    let tz: Arc<str> = Arc::from("+00:00");
    RecordBatch::try_new(
        schema,
        vec![Arc::new(
            TimestampNanosecondArray::from(begin_values).with_timezone(tz),
        )],
    )
    .expect("valid batch")
}

/// Test #10: `ensure_begin_non_decreasing` rejects a hand-built batch with a regressing `begin`
/// and passes a monotone one. The returned error names the stream and the offending row, so the
/// same detail is guaranteed present in the `error!` log line.
#[test]
fn ensure_begin_non_decreasing_accepts_monotone_batch() {
    let batch = begin_batch(vec![100, 200, 200, 300]);
    let result = ensure_begin_non_decreasing("stream-a", &batch);
    assert!(
        result.is_ok(),
        "non-decreasing begin should be accepted: {result:?}"
    );
}

#[test]
fn ensure_begin_non_decreasing_rejects_regressing_batch() {
    let batch = begin_batch(vec![100, 300, 200]);
    let result = ensure_begin_non_decreasing("stream-a", &batch);
    let err = result.expect_err("a begin regression must be a hard error");
    let message = format!("{err:#}");
    assert!(
        message.contains("stream-a"),
        "error should name the stream: {message}"
    );
    assert!(
        message.contains("row 2"),
        "error should name the offending row index: {message}"
    );
    assert!(
        message.contains("300") && message.contains("200"),
        "error should name the two begin values: {message}"
    );
}
