use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::{Int32Array, RecordBatch};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::parquet::arrow::AsyncArrowWriter;
use micromegas_analytics::lakehouse::async_parquet_writer::AsyncParquetWriter;
use micromegas_analytics::lakehouse::write_partition::{
    PartitionRowSet, write_rows_and_track_times,
};
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_analytics::time::TimeRange;
use object_store::buffered::BufWriter;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::{Arc, atomic::AtomicI64};

fn one_column_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]))
}

/// Same single `x: Int32` column, but declared nullable -- used by the nullability-guard tests
/// below to build a batch `RecordBatch::try_new` itself would accept (a null under a *declared*
/// non-nullable field is what the guard added in `write_partition.rs` exists to catch instead).
fn nullable_one_column_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, true)]))
}

fn make_arrow_writer() -> AsyncArrowWriter<AsyncParquetWriter> {
    make_arrow_writer_with_schema(one_column_schema())
}

fn make_arrow_writer_with_schema(schema: Arc<Schema>) -> AsyncArrowWriter<AsyncParquetWriter> {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/output.parquet");
    let byte_counter = Arc::new(AtomicI64::new(0));
    let buf_writer = BufWriter::new(store, path);
    let parquet_writer = AsyncParquetWriter::new(buf_writer, byte_counter);
    AsyncArrowWriter::try_new(parquet_writer, schema, None).expect("AsyncArrowWriter::try_new")
}

/// A one-row, one-column (`x: Int32`) batch matching `make_arrow_writer`'s schema.
fn make_row_set(
    rows_time_range: TimeRange,
    max_sort_key_time: Option<DateTime<Utc>>,
) -> PartitionRowSet {
    let rows = RecordBatch::try_new(
        one_column_schema(),
        vec![Arc::new(Int32Array::from(vec![1]))],
    )
    .expect("building one-column batch");
    PartitionRowSet::new(rows_time_range, rows, max_sort_key_time)
}

/// A one-row, one-column (`x: Int32`) batch carrying a `NULL` -- built under
/// `nullable_one_column_schema()` so `RecordBatch::try_new` itself accepts it (see that
/// function's doc comment).
fn make_row_set_with_null(rows_time_range: TimeRange) -> PartitionRowSet {
    let rows = RecordBatch::try_new(
        nullable_one_column_schema(),
        vec![Arc::new(Int32Array::from(vec![None::<i32>]))],
    )
    .expect("building one-column nullable batch with a null");
    PartitionRowSet::new(rows_time_range, rows, None)
}

#[tokio::test]
async fn test_write_rows_propagates_err_from_channel() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>(1);
    tx.send(Err(anyhow::anyhow!("injected error")))
        .await
        .expect("send");
    drop(tx);

    let logger: Arc<dyn micromegas_analytics::response_writer::Logger> =
        Arc::new(ResponseWriter::new(None));
    let mut arrow_writer = make_arrow_writer();
    let result = write_rows_and_track_times(
        &mut rx,
        &mut arrow_writer,
        &one_column_schema(),
        &logger,
        "test",
    )
    .await;

    assert!(result.is_err(), "expected Err from channel poison");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("injected error"),
        "error should contain original message; got: {msg}"
    );
}

/// `write_rows_and_track_times`'s `max_sort_key_time` fold must be a running `max`, not "last row
/// set wins" -- `BlockPartitionSpec` streams row sets *out of order* via `buffer_unordered`, so
/// "last wins" would be actively wrong there. Sends three `Some` row sets out of increasing order
/// through a channel wide enough to avoid the `channel(1)` deadlock the other test in this file
/// sidesteps by sending before the consumer runs.
#[tokio::test]
async fn write_rows_folds_a_running_max_not_last_row_set_wins() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>(8);
    let t0 = Utc::now();
    // Sent out of order and not matching arrival order: the highest value (t0+30s) is sent
    // second, not last, so "last row set wins" would wrongly report t0+10s.
    let rows = vec![
        make_row_set(
            TimeRange::new(t0, t0 + TimeDelta::seconds(1)),
            Some(t0 + TimeDelta::seconds(20)),
        ),
        make_row_set(
            TimeRange::new(t0, t0 + TimeDelta::seconds(1)),
            Some(t0 + TimeDelta::seconds(30)),
        ),
        make_row_set(
            TimeRange::new(t0, t0 + TimeDelta::seconds(1)),
            Some(t0 + TimeDelta::seconds(10)),
        ),
    ];
    for row_set in rows {
        tx.send(Ok(row_set)).await.expect("send");
    }
    drop(tx);

    let logger: Arc<dyn micromegas_analytics::response_writer::Logger> =
        Arc::new(ResponseWriter::new(None));
    let mut arrow_writer = make_arrow_writer();
    let result = write_rows_and_track_times(
        &mut rx,
        &mut arrow_writer,
        &one_column_schema(),
        &logger,
        "test",
    )
    .await
    .expect("write_rows_and_track_times should succeed");

    assert_eq!(
        result.max_sort_key_time,
        Some(t0 + TimeDelta::seconds(30)),
        "the folded value must be the running max across all row sets, not the last one sent"
    );
}

/// A single `None` among several `Some` row sets must poison the partition-level
/// `max_sort_key_time` to `None` -- the soundness rule that lets a scan check trust the recorded
/// bound only when *every* row set actually carried one.
#[tokio::test]
async fn write_rows_none_row_set_poisons_max_sort_key_time() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>(8);
    let t0 = Utc::now();
    let rows = vec![
        make_row_set(
            TimeRange::new(t0, t0 + TimeDelta::seconds(1)),
            Some(t0 + TimeDelta::seconds(20)),
        ),
        make_row_set(TimeRange::new(t0, t0 + TimeDelta::seconds(1)), None),
        make_row_set(
            TimeRange::new(t0, t0 + TimeDelta::seconds(1)),
            Some(t0 + TimeDelta::seconds(30)),
        ),
    ];
    for row_set in rows {
        tx.send(Ok(row_set)).await.expect("send");
    }
    drop(tx);

    let logger: Arc<dyn micromegas_analytics::response_writer::Logger> =
        Arc::new(ResponseWriter::new(None));
    let mut arrow_writer = make_arrow_writer();
    let result = write_rows_and_track_times(
        &mut rx,
        &mut arrow_writer,
        &one_column_schema(),
        &logger,
        "test",
    )
    .await
    .expect("write_rows_and_track_times should succeed");

    assert_eq!(
        result.max_sort_key_time, None,
        "a single None row set must poison the partition-level value to None, even though every \
         other row set carried Some"
    );
}

/// The nullability guard (#1482 §1) is what turns a `NULL` slipping under a declared
/// non-nullable column into a loud write failure instead of a silently `""`-labelled row. The
/// batch itself is built under a *nullable* schema (so `RecordBatch::try_new` doesn't reject the
/// null first, per this file's `make_row_set_with_null` doc comment), while the `file_schema`
/// handed to `write_rows_and_track_times` declares the same column NOT NULL -- exactly the
/// mismatch a bug in an extraction site's `COALESCE` (#1482), or any future producer that
/// bypasses it, would produce against a view's declared schema.
#[tokio::test]
async fn write_rows_rejects_null_under_declared_non_nullable_column() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>(1);
    let t0 = Utc::now();
    tx.send(Ok(make_row_set_with_null(TimeRange::new(
        t0,
        t0 + TimeDelta::seconds(1),
    ))))
    .await
    .expect("send");
    drop(tx);

    let logger: Arc<dyn micromegas_analytics::response_writer::Logger> =
        Arc::new(ResponseWriter::new(None));
    let mut arrow_writer = make_arrow_writer();
    let result = write_rows_and_track_times(
        &mut rx,
        &mut arrow_writer,
        &one_column_schema(),
        &logger,
        "test",
    )
    .await;

    let err = result.expect_err("a NULL under a declared NOT NULL column must fail the write");
    let msg = format!("{err:#}");
    assert!(
        msg.contains('x'),
        "the error must name the offending column; got: {msg}"
    );
}

/// The mirror image of the test above: the same batch (a `NULL` under `x`) succeeds when the
/// declared `file_schema` also marks the column nullable -- the guard only ever rejects a
/// mismatch, never nullability itself.
#[tokio::test]
async fn write_rows_accepts_null_when_field_declared_nullable() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>(1);
    let t0 = Utc::now();
    tx.send(Ok(make_row_set_with_null(TimeRange::new(
        t0,
        t0 + TimeDelta::seconds(1),
    ))))
    .await
    .expect("send");
    drop(tx);

    let logger: Arc<dyn micromegas_analytics::response_writer::Logger> =
        Arc::new(ResponseWriter::new(None));
    let mut arrow_writer = make_arrow_writer_with_schema(nullable_one_column_schema());
    write_rows_and_track_times(
        &mut rx,
        &mut arrow_writer,
        &nullable_one_column_schema(),
        &logger,
        "test",
    )
    .await
    .expect("a NULL under a declared nullable column must succeed");
}
