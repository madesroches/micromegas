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

fn make_arrow_writer() -> AsyncArrowWriter<AsyncParquetWriter> {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/output.parquet");
    let byte_counter = Arc::new(AtomicI64::new(0));
    let buf_writer = BufWriter::new(store, path);
    let parquet_writer = AsyncParquetWriter::new(buf_writer, byte_counter);
    AsyncArrowWriter::try_new(parquet_writer, one_column_schema(), None)
        .expect("AsyncArrowWriter::try_new")
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
    let result = write_rows_and_track_times(&mut rx, &mut arrow_writer, &logger, "test").await;

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
    let result = write_rows_and_track_times(&mut rx, &mut arrow_writer, &logger, "test")
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
    let result = write_rows_and_track_times(&mut rx, &mut arrow_writer, &logger, "test")
        .await
        .expect("write_rows_and_track_times should succeed");

    assert_eq!(
        result.max_sort_key_time, None,
        "a single None row set must poison the partition-level value to None, even though every \
         other row set carried Some"
    );
}
