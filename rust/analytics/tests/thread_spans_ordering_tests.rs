//! Offline (no live DB) regression tests for the perfetto-export sort-elimination plan
//! (tasks/1297_perfetto_redundant_sort_plan.md):
//! - the §3 file-group sort + non-overlap loud-failure guard in `make_partitioned_execution_plan`
//! - the resulting plan shape: a declared ordering lets `EnforceSorting` elide a redundant `Sort`
//!   for a multi-partition file group, while an undeclared ordering keeps it (negative control)
//! - the §4 runtime `begin`-monotonicity guard in `write_thread_spans`

use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::{RecordBatch, StringArray, TimestampNanosecondArray, UInt32Array};
use datafusion::arrow::compute::SortOptions;
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_expr::{LexOrdering, PhysicalSortExpr, expressions::Column};
use datafusion::physical_optimizer::PhysicalOptimizerRule;
use datafusion::physical_optimizer::enforce_sorting::EnforceSorting;
use datafusion::physical_plan::sorts::sort::SortExec;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{ExecutionPlan, displayable};
use datafusion::prelude::SessionContext;
use futures::stream;
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partitioned_execution_plan::make_partitioned_execution_plan;
use micromegas_analytics::lakehouse::perfetto_trace_execution_plan::write_thread_spans;
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::view::{ScanSortColumn, ViewMetadata};
use micromegas_analytics::span_table::get_spans_schema;
use micromegas_analytics::time::TimeRange;
use micromegas_perfetto::chunk_sender::ChunkSender;
use micromegas_perfetto::streaming_writer::PerfettoWriter;
use std::sync::Arc;

fn make_partition(file_path: &str, min_time: DateTime<Utc>, max_time: DateTime<Utc>) -> Partition {
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("thread_spans".to_owned()),
            view_instance_id: Arc::new("test-stream".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(min_time, max_time),
        event_time_range: Some(TimeRange::new(min_time, max_time)),
        updated: Utc::now(),
        file_path: Some(file_path.to_owned()),
        file_size: 1024,
        source_data_hash: vec![0],
        num_rows: 10,
    }
}

fn make_reader_factory() -> Arc<ReaderFactory> {
    Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        Arc::new(MetadataCache::new(1024 * 1024)),
    ))
}

fn begin_ascending() -> Vec<ScanSortColumn> {
    vec![ScanSortColumn {
        column: Arc::new("begin".to_owned()),
        descending: false,
    }]
}

#[tokio::test]
async fn overlapping_partitions_are_rejected() {
    let schema = Arc::new(get_spans_schema());
    let t0 = Utc::now();
    let part_a = make_partition("a.parquet", t0, t0 + TimeDelta::seconds(10));
    // part_b's min_event_time (t0+5s) is before part_a's max_event_time (t0+10s): overlap.
    let part_b = make_partition(
        "b.parquet",
        t0 + TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(20),
    );
    let ctx = SessionContext::new();
    let state = ctx.state();
    let result = make_partitioned_execution_plan(
        schema,
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(vec![part_a, part_b]),
        &begin_ascending(),
    );
    assert!(
        result.is_err(),
        "overlapping partitions must be rejected instead of silently mis-declaring the ordering"
    );
}

#[tokio::test]
async fn non_overlapping_partitions_are_accepted() {
    let schema = Arc::new(get_spans_schema());
    let t0 = Utc::now();
    let part_a = make_partition("a.parquet", t0, t0 + TimeDelta::seconds(10));
    let part_b = make_partition(
        "b.parquet",
        t0 + TimeDelta::seconds(10),
        t0 + TimeDelta::seconds(20),
    );
    let ctx = SessionContext::new();
    let state = ctx.state();
    let result = make_partitioned_execution_plan(
        schema,
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(vec![part_a, part_b]),
        &begin_ascending(),
    );
    assert!(
        result.is_ok(),
        "non-overlapping partitions should be accepted: {result:?}"
    );
}

fn begin_sort_expr(schema: &Schema) -> LexOrdering {
    let sort_expr = PhysicalSortExpr::new(
        Arc::new(Column::new_with_schema("begin", schema).expect("begin column")),
        SortOptions {
            descending: false,
            nulls_first: false,
        },
    );
    LexOrdering::new(vec![sort_expr]).expect("non-empty ordering")
}

async fn build_plan_wrapped_in_sort(output_ordering: &[ScanSortColumn]) -> Arc<dyn ExecutionPlan> {
    let schema = Arc::new(get_spans_schema());
    let t0 = Utc::now();
    // Two non-overlapping partitions handed in reverse (non-min_event_time) order, to exercise
    // the file-group sort as well as the statistics/ordering declaration -- this is the
    // multi-partition-file-group case the declared ordering exists to optimize.
    let part_later = make_partition(
        "later.parquet",
        t0 + TimeDelta::hours(2),
        t0 + TimeDelta::hours(2) + TimeDelta::seconds(10),
    );
    let part_earlier = make_partition("earlier.parquet", t0, t0 + TimeDelta::seconds(10));

    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = make_partitioned_execution_plan(
        schema.clone(),
        make_reader_factory(),
        &state,
        None,
        &[],
        None,
        Arc::new(vec![part_later, part_earlier]),
        output_ordering,
    )
    .expect("plan should build");

    Arc::new(SortExec::new(begin_sort_expr(&schema), plan))
}

#[tokio::test]
async fn declared_ordering_elides_redundant_sort_for_multi_partition_group() {
    let sorted_plan = build_plan_wrapped_in_sort(&begin_ascending()).await;
    let optimized = EnforceSorting::new()
        .optimize(sorted_plan, &Default::default())
        .expect("EnforceSorting should not fail");
    let plan_str = displayable(optimized.as_ref()).indent(false).to_string();
    assert!(
        !plan_str.contains("SortExec"),
        "expected the redundant Sort to be elided once the ordering is declared and file \
         statistics are attached, got:\n{plan_str}"
    );
}

#[tokio::test]
async fn undeclared_ordering_keeps_sort_negative_control() {
    let sorted_plan = build_plan_wrapped_in_sort(&[]).await;
    let optimized = EnforceSorting::new()
        .optimize(sorted_plan, &Default::default())
        .expect("EnforceSorting should not fail");
    let plan_str = displayable(optimized.as_ref()).indent(false).to_string();
    assert!(
        plan_str.contains("SortExec"),
        "a view that does not declare an ordering must still sort, got:\n{plan_str}"
    );
}

fn thread_span_batch_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(
            "begin",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "end",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("name", DataType::Utf8, false),
        Field::new("filename", DataType::Utf8, false),
        Field::new("target", DataType::Utf8, false),
        Field::new("line", DataType::UInt32, false),
    ]))
}

fn make_thread_span_stream(begin_values: Vec<i64>) -> SendableRecordBatchStream {
    let schema = thread_span_batch_schema();
    let n = begin_values.len();
    let end_values: Vec<i64> = begin_values.iter().map(|b| b + 1).collect();
    let tz: Arc<str> = Arc::from("+00:00");
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(TimestampNanosecondArray::from(begin_values).with_timezone(tz.clone())),
            Arc::new(TimestampNanosecondArray::from(end_values).with_timezone(tz)),
            Arc::new(StringArray::from(vec!["span"; n])),
            Arc::new(StringArray::from(vec!["file.rs"; n])),
            Arc::new(StringArray::from(vec!["target"; n])),
            Arc::new(UInt32Array::from(vec![1u32; n])),
        ],
    )
    .expect("valid batch");
    Box::pin(RecordBatchStreamAdapter::new(
        schema,
        stream::iter(vec![Ok(batch)]),
    ))
}

fn make_discarding_writer() -> PerfettoWriter {
    // Channel is large enough that the single flush a small test emits never blocks; the
    // receiver is kept alive by the caller (held in the same scope) and its contents unused.
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let chunk_sender = ChunkSender::new(tx, 8 * 1024);
    PerfettoWriter::new(Box::new(chunk_sender), "test-process")
}

#[tokio::test]
async fn monotonic_begin_is_accepted() {
    let mut writer = make_discarding_writer();
    let data_stream = make_thread_span_stream(vec![100, 200, 300]);
    let result = write_thread_spans(&mut writer, "test-stream", data_stream).await;
    assert!(
        result.is_ok(),
        "non-decreasing begin should be accepted: {result:?}"
    );
}

#[tokio::test]
async fn out_of_order_begin_is_rejected() {
    let mut writer = make_discarding_writer();
    // 200 follows 300: an out-of-order row like this must never be silently emitted, since
    // ThreadSpansView's declared scan ordering means DataFusion no longer re-sorts these rows.
    let data_stream = make_thread_span_stream(vec![100, 300, 200]);
    let result = write_thread_spans(&mut writer, "test-stream", data_stream).await;
    assert!(
        result.is_err(),
        "a begin regression within a single thread's stream must be a hard error"
    );
}
