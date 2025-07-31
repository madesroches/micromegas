use bytes::Bytes;
use datafusion::parquet::{self, errors::ParquetError};
use futures::future::BoxFuture;
use object_store::buffered::BufWriter;
use parquet::arrow::async_writer::AsyncFileWriter;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use tokio::io::AsyncWriteExt;

/// A Parquet writer that writes to an `object_store::buffered::BufWriter` and counts bytes written.
#[derive(Debug)]
pub struct AsyncParquetWriter {
    w: BufWriter,
    counter: Arc<AtomicI64>,
}

impl AsyncParquetWriter {
    pub fn new(w: BufWriter, counter: Arc<AtomicI64>) -> Self {
        Self { w, counter }
    }
}

impl AsyncFileWriter for AsyncParquetWriter {
    fn write(&mut self, bs: Bytes) -> BoxFuture<'_, parquet::errors::Result<()>> {
        self.counter.fetch_add(bs.len() as i64, Ordering::Relaxed);
        Box::pin(async {
            self.w
                .put(bs)
                .await
                .map_err(|err| ParquetError::External(Box::new(err)))
        })
    }

    fn complete(&mut self) -> BoxFuture<'_, parquet::errors::Result<()>> {
        Box::pin(async {
            self.w
                .shutdown()
                .await
                .map_err(|err| ParquetError::External(Box::new(err)))
        })
    }
}
