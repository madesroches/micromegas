use anyhow::{Context, Result};
use bytes::Buf;
use bytes::BufMut;
use datafusion::parquet::basic::Compression;
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::parquet::file::properties::WriterVersion;
use datafusion::{arrow::record_batch::RecordBatch, parquet::arrow::ArrowWriter};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use serde::Deserialize;
use sqlx::types::chrono::{DateTime, FixedOffset};

use crate::sql_arrow_bridge::rows_to_record_batch;

#[derive(Debug, Clone)]
pub struct AnalyticsService {
    data_lake: DataLakeConnection,
}

#[derive(Debug, Deserialize)]
pub struct QueryProcessesRequest {
    pub limit: i64,
    pub begin: String,
    pub end: String,
}

#[derive(Debug, Deserialize)]
pub struct QueryStreamsRequest {
    pub limit: i64,
    pub begin: String,
    pub end: String,
    pub tag_filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryBlocksRequest {
    pub stream_id: String,
}

#[derive(Debug, Deserialize)]
pub struct QuerySpansRequest {
    pub limit: i64,
    pub begin: String,
    pub end: String,
    pub stream_id: String,
}

impl AnalyticsService {
    pub fn new(data_lake: DataLakeConnection) -> Self {
        Self { data_lake }
    }

    pub async fn query_processes(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryProcessesRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing QueryProcessesRequest")?;

        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;

        let mut connection = self.data_lake.db_pool.acquire().await?;
        let rows = sqlx::query(
            "SELECT process_id,
                    exe,
                    username,
                    realname,
                    computer,
                    distro,
                    cpu_brand,
                    tsc_frequency,
                    start_time,
                    start_ticks,
                    insert_time,
                    parent_process_id,
                    properties
             FROM processes
             WHERE start_time >= $1
             AND start_time < $2
             ORDER BY start_time
             LIMIT $3",
        )
        .bind(begin)
        .bind(end)
        .bind(request.limit)
        .fetch_all(&mut *connection)
        .await?;
        serialize_record_batch(
            &rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?,
        )
    }

    pub async fn query_streams(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryStreamsRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QueryStreamsRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let mut connection = self.data_lake.db_pool.acquire().await?;
        let mut tag_condition = "";
        if request.tag_filter.is_some() {
            tag_condition = "AND array_position(tags, $4) is not NULL";
        }
        let sql = format!(
            "SELECT stream_id,
                    process_id,
                    tags,
                    properties
             FROM streams
             WHERE insert_time >= $1
             AND insert_time < $2
             {tag_condition}
             ORDER BY insert_time
             LIMIT $3"
        );
        let mut query = sqlx::query(&sql).bind(begin).bind(end).bind(request.limit);
        if let Some(tag) = &request.tag_filter {
            query = query.bind(tag);
        }
        let rows = query.fetch_all(&mut *connection).await?;
        serialize_record_batch(
            &rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?,
        )
    }

    pub async fn query_blocks(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryBlocksRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QueryBlocksRequest")?;
        let mut connection = self.data_lake.db_pool.acquire().await?;
        let sql = "SELECT block_id,
                    stream_id,
                    process_id,
                    begin_time,
                    begin_ticks,
                    end_time,
                    end_ticks,
                    nb_objects,
                    payload_size
             FROM blocks
             WHERE stream_id = $1
             ORDER BY begin_time;";
        let rows = sqlx::query(sql)
            .bind(request.stream_id)
            .fetch_all(&mut *connection)
            .await?;
        serialize_record_batch(
            &rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?,
        )
    }

    pub async fn query_spans(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QuerySpansRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QuerySpansRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        serialize_record_batch(
            &crate::query_spans::query_spans(
                &self.data_lake,
                &request.stream_id,
                begin.into(),
                end.into(),
            )
            .await
            .with_context(|| "query_spans")?,
        )
    }
}

fn serialize_record_batch(record_batch: &RecordBatch) -> Result<bytes::Bytes> {
    let mut buffer_writer = bytes::BytesMut::with_capacity(1024).writer();
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        .build();
    let mut arrow_writer =
        ArrowWriter::try_new(&mut buffer_writer, record_batch.schema(), Some(props))?;
    arrow_writer.write(record_batch)?;
    arrow_writer.close()?;
    Ok(buffer_writer.into_inner().into())
}
