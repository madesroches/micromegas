use anyhow::{Context, Result};
use bytes::Buf;
use bytes::BufMut;
use datafusion::parquet::basic::Compression;
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::{arrow::record_batch::RecordBatch, parquet::arrow::ArrowWriter};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use serde::Deserialize;

use crate::sql_arrow_bridge::rows_to_record_batch;

#[derive(Debug, Clone)]
pub struct AnalyticsService {
    data_lake: DataLakeConnection,
}

#[derive(Debug, Deserialize)]
pub struct QueryProcessesRequest {
    pub limit: i64,
}

impl AnalyticsService {
    pub fn new(data_lake: DataLakeConnection) -> Self {
        Self { data_lake }
    }

    pub async fn query_processes(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryProcessesRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing QueryProcessesRequest")?;
        let mut connection = self.data_lake.db_pool.acquire().await?;
        let rows = sqlx::query(
            "SELECT process_id, tsc_frequency
             FROM processes
             LIMIT $1",
        )
        .bind(request.limit)
        .fetch_all(&mut *connection)
        .await?;
        serialize_record_batch(
            &rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?,
        )
    }
}

fn serialize_record_batch(record_batch: &RecordBatch) -> Result<bytes::Bytes> {
    let mut buffer_writer = bytes::BytesMut::with_capacity(1024).writer();
    let props = WriterProperties::builder()
        .set_compression(Compression::LZ4_RAW)
        .build();
    let mut arrow_writer =
        ArrowWriter::try_new(&mut buffer_writer, record_batch.schema(), Some(props))?;
    arrow_writer.write(record_batch)?;
    arrow_writer.close()?;
    Ok(buffer_writer.into_inner().into())
}
