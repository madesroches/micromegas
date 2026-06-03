use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::parquet::arrow::AsyncArrowWriter;
use micromegas_analytics::lakehouse::async_parquet_writer::AsyncParquetWriter;
use micromegas_analytics::lakehouse::write_partition::{
    PartitionRowSet, write_rows_and_track_times,
};
use micromegas_analytics::response_writer::ResponseWriter;
use object_store::buffered::BufWriter;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::{Arc, atomic::AtomicI64};

fn make_arrow_writer() -> AsyncArrowWriter<AsyncParquetWriter> {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/output.parquet");
    let byte_counter = Arc::new(AtomicI64::new(0));
    let buf_writer = BufWriter::new(store, path);
    let parquet_writer = AsyncParquetWriter::new(buf_writer, byte_counter);
    let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]));
    AsyncArrowWriter::try_new(parquet_writer, schema, None).expect("AsyncArrowWriter::try_new")
}

#[tokio::test]
async fn test_write_rows_propagates_err_from_channel() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>(1);
    let _ = tx
        .send(Err(anyhow::anyhow!("injected error")))
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
