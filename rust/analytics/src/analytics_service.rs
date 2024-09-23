use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Buf;
use bytes::BufMut;
use chrono::TimeDelta;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::basic::Compression;
use datafusion::parquet::file::properties::WriterProperties;
use datafusion::parquet::file::properties::WriterVersion;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use serde::Deserialize;
use sqlx::types::chrono::{DateTime, FixedOffset};
use uuid::Uuid;

use crate::lakehouse::answer::Answer;
use crate::lakehouse::batch_update::materialize_partition_range;
use crate::lakehouse::partition_cache::LivePartitionProvider;
use crate::lakehouse::partition_cache::PartitionCache;
use crate::lakehouse::view_factory::ViewFactory;
use crate::lakehouse::write_partition::retire_partitions;
use crate::response_writer::ResponseWriter;
use crate::sql_arrow_bridge::rows_to_record_batch;
use crate::time::TimeRange;

#[derive(Clone)]
pub struct AnalyticsService {
    data_lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
}

#[derive(Debug, Deserialize)]
pub struct FindProcessRequest {
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string")]
    pub process_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct FindStreamRequest {
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string")]
    pub stream_id: Uuid,
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
    pub begin: Option<String>,
    pub end: Option<String>,
    pub tag_filter: Option<String>,
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::opt_uuid_from_string")]
    pub process_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct QueryBlocksRequest {
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string")]
    pub stream_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct QuerySpansRequest {
    pub limit: i64,
    pub begin: String,
    pub end: String,
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string")]
    pub stream_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct QueryThreadEventsRequest {
    pub limit: i64,
    pub begin: String,
    pub end: String,
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::uuid_from_string")]
    pub stream_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct QueryLogEntriesRequest {
    pub limit: Option<i64>,
    pub begin: String,
    pub end: String,
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::opt_uuid_from_string")]
    pub stream_id: Option<Uuid>,
    pub sql: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryMetricsRequest {
    pub limit: Option<i64>,
    pub begin: String,
    pub end: String,
    #[serde(deserialize_with = "micromegas_transit::uuid_utils::opt_uuid_from_string")]
    pub stream_id: Option<Uuid>,
    pub sql: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryViewRequest {
    pub view_set_name: String,
    pub view_instance_id: String,
    pub begin: String,
    pub end: String,
    pub sql: String,
}

#[derive(Debug, Deserialize)]
pub struct MetarializePartitionsRequest {
    pub view_set_name: String,
    pub view_instance_id: String,
    pub begin: String,
    pub end: String,
    pub partition_delta_seconds: i64,
}

#[derive(Debug, Deserialize)]
pub struct MergePartitionsRequest {
    pub view_set_name: String,
    pub view_instance_id: String,
    pub begin: String,
    pub end: String,
    pub partition_delta_seconds: i64,
}

#[derive(Debug, Deserialize)]
pub struct RetirePartitionsRequest {
    pub view_set_name: String,
    pub view_instance_id: String,
    pub begin: String,
    pub end: String,
}

impl AnalyticsService {
    pub fn new(data_lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Self {
        Self {
            data_lake,
            view_factory,
        }
    }

    pub async fn find_process(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: FindProcessRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing FindProcessRequest")?;
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
             WHERE process_id = $1",
        )
        .bind(request.process_id)
        .fetch_all(&self.data_lake.db_pool)
        .await?;
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn find_stream(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: FindStreamRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing FindStreamRequest")?;
        let rows = sqlx::query(
            "SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
             FROM streams
             WHERE stream_id = $1",
        )
        .bind(request.stream_id)
        .fetch_all(&self.data_lake.db_pool)
        .await?;
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn query_processes(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryProcessesRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing QueryProcessesRequest")?;

        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
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
        .fetch_all(&self.data_lake.db_pool)
        .await?;
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn query_streams(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryStreamsRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QueryStreamsRequest")?;
        if (request.begin.is_none() || request.end.is_none()) && request.process_id.is_none() {
            anyhow::bail!("Time range or process_id have to be provided");
        }
        let mut conditions = vec![];
        let mut begin_time = None;
        if let Some(time_str) = &request.begin {
            begin_time = Some(
                DateTime::<FixedOffset>::parse_from_rfc3339(time_str)
                    .with_context(|| "parsing begin time range")?,
            );
            conditions.push(format!(
                "(insert_time >= {})",
                format_postgres_placeholder(conditions.len())
            ));
        }
        let mut end_time = None;
        if let Some(time_str) = &request.end {
            end_time = Some(
                DateTime::<FixedOffset>::parse_from_rfc3339(time_str)
                    .with_context(|| "parsing end time range")?,
            );
            conditions.push(format!(
                "(insert_time < {})",
                format_postgres_placeholder(conditions.len())
            ));
        }
        if request.process_id.is_some() {
            conditions.push(format!(
                "(process_id = {})",
                format_postgres_placeholder(conditions.len())
            ));
        }
        if request.tag_filter.is_some() {
            conditions.push(format!(
                "(array_position(tags, {}) is not NULL)",
                format_postgres_placeholder(conditions.len())
            ));
        }
        let limit_placeholder = format_postgres_placeholder(conditions.len());
        let joined_conditions = conditions.join(" AND ");
        let sql = format!(
            "SELECT stream_id,
                    process_id,
                    tags,
                    properties
             FROM streams
             WHERE {joined_conditions}
             ORDER BY insert_time
             LIMIT {limit_placeholder}"
        );

        let mut query = sqlx::query(&sql);
        if let Some(begin) = begin_time {
            query = query.bind(begin);
        }
        if let Some(end) = end_time {
            query = query.bind(end);
        }
        if request.process_id.is_some() {
            query = query.bind(request.process_id);
        }
        if request.tag_filter.is_some() {
            query = query.bind(request.tag_filter);
        }
        query = query.bind(request.limit);
        let rows = query.fetch_all(&self.data_lake.db_pool).await?;
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn query_blocks(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryBlocksRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QueryBlocksRequest")?;
        let sql = "SELECT block_id,
                    stream_id,
                    process_id,
                    begin_time,
                    begin_ticks,
                    end_time,
                    end_ticks,
                    nb_objects,
                    object_offset,
                    payload_size
             FROM blocks
             WHERE stream_id = $1
             ORDER BY begin_time;";
        let rows = sqlx::query(sql)
            .bind(request.stream_id)
            .fetch_all(&self.data_lake.db_pool)
            .await?;
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn query_spans(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QuerySpansRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QuerySpansRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let record_batch = crate::query_spans::query_spans(
            &self.data_lake,
            request.limit,
            request.stream_id,
            begin.into(),
            end.into(),
        )
        .await
        .with_context(|| "query_spans")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn query_thread_events(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryThreadEventsRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing QueryThreadEventsRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let record_batch = crate::query_thread_events::query_thread_events(
            &self.data_lake,
            request.limit,
            request.stream_id,
            begin.into(),
            end.into(),
        )
        .await
        .with_context(|| "query_thread_events")?;
        let answer = Answer::from_record_batch(record_batch);
        serialize_record_batches(&answer)
    }

    pub async fn query_log_entries(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryLogEntriesRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing QueryLogEntriesRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let answer = if let Some(stream_id) = request.stream_id {
            if request.limit.is_none() {
                anyhow::bail!("limit is required for stream-specific queries");
            }
            let record_batch = crate::query_log_entries::query_log_entries(
                &self.data_lake,
                stream_id,
                begin.into(),
                end.into(),
                request.limit.unwrap(),
            )
            .await
            .with_context(|| "query_log_entries")?;
            Answer::from_record_batch(record_batch)
        } else {
            if request.sql.is_none() {
                anyhow::bail!("sql is required for lakehouse queries");
            }
            if request.limit.is_some() {
                anyhow::bail!("limit must be included in the sql statement for lakehouse queries");
            }
            let view = self.view_factory.make_view("log_entries", "global")?;
            crate::lakehouse::query::query_single_view(
                self.data_lake.clone(),
                Arc::new(LivePartitionProvider::new(self.data_lake.db_pool.clone())),
                TimeRange::new(begin.into(), end.into()),
                &request.sql.unwrap(),
                view,
            )
            .await
            .with_context(|| "lakehouse::query::query")?
        };
        serialize_record_batches(&answer)
    }

    pub async fn query_metrics(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryMetricsRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QueryMetricsRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let answer = if let Some(stream_id) = request.stream_id {
            if request.limit.is_none() {
                anyhow::bail!("limit is required for stream-specific queries");
            }
            let record_batch = crate::query_metrics::query_metrics(
                &self.data_lake,
                request.limit.unwrap(),
                stream_id,
                begin.into(),
                end.into(),
            )
            .await
            .with_context(|| "query_metrics")?;
            Answer::from_record_batch(record_batch)
        } else {
            if request.sql.is_none() {
                anyhow::bail!("sql is required for lakehouse queries");
            }
            if request.limit.is_some() {
                anyhow::bail!("limit must be included in the sql statement for lakehouse queries");
            }
            let view = self.view_factory.make_view("measures", "global")?;
            crate::lakehouse::query::query_single_view(
                self.data_lake.clone(),
                Arc::new(LivePartitionProvider::new(self.data_lake.db_pool.clone())),
                TimeRange::new(begin.into(), end.into()),
                &request.sql.unwrap(),
                view,
            )
            .await
            .with_context(|| "lakehouse::query::query")?
        };
        serialize_record_batches(&answer)
    }

    pub async fn query_view(&self, body: bytes::Bytes) -> Result<bytes::Bytes> {
        let request: QueryViewRequest =
            ciborium::from_reader(body.reader()).with_context(|| "parsing QueryViewRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let view = self
            .view_factory
            .make_view(&request.view_set_name, &request.view_instance_id)
            .with_context(|| "making view")?;
        let answer = crate::lakehouse::query::query_single_view(
            self.data_lake.clone(),
            Arc::new(LivePartitionProvider::new(self.data_lake.db_pool.clone())),
            TimeRange::new(begin.into(), end.into()),
            &request.sql,
            view,
        )
        .await
        .with_context(|| "lakehouse::query::query")?;
        serialize_record_batches(&answer)
    }

    pub async fn query_partitions(&self) -> Result<bytes::Bytes> {
        // if partitions are merged on a daily basis, there should not be that many
        let rows = sqlx::query(
            "SELECT *
             FROM lakehouse_partitions
             WHERE file_metadata IS NOT NULL
             ;",
        )
        .fetch_all(&self.data_lake.db_pool)
        .await?;
        let record_batch =
            rows_to_record_batch(&rows).with_context(|| "converting rows to record batch")?;
        serialize_record_batches(&Answer::from_record_batch(record_batch))
    }

    pub async fn materialize_partition_range(
        &self,
        body: bytes::Bytes,
        writer: Arc<ResponseWriter>,
    ) -> Result<()> {
        let request: MetarializePartitionsRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing MetarializePartitionsRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let view = self
            .view_factory
            .make_view(&request.view_set_name, &request.view_instance_id)
            .with_context(|| "making view")?;
        let delta = TimeDelta::try_seconds(request.partition_delta_seconds)
            .with_context(|| "making time delta")?;
        let existing_partitions = Arc::new(
            PartitionCache::fetch_overlapping_insert_range(
                &self.data_lake.db_pool,
                begin.into(),
                end.into(),
            )
            .await?,
        );
        materialize_partition_range(
            existing_partitions,
            self.data_lake.clone(),
            view,
            begin.into(),
            end.into(),
            delta,
            writer,
        )
        .await?;
        Ok(())
    }

    pub async fn retire_partitions(
        &self,
        body: bytes::Bytes,
        writer: Arc<ResponseWriter>,
    ) -> Result<()> {
        let request: RetirePartitionsRequest = ciborium::from_reader(body.reader())
            .with_context(|| "parsing RetirePartitionsRequest")?;
        let begin = DateTime::<FixedOffset>::parse_from_rfc3339(&request.begin)
            .with_context(|| "parsing begin time range")?;
        let end = DateTime::<FixedOffset>::parse_from_rfc3339(&request.end)
            .with_context(|| "parsing end time range")?;
        let mut tr = self.data_lake.db_pool.begin().await?;
        retire_partitions(
            &mut tr,
            &request.view_set_name,
            &request.view_instance_id,
            begin.into(),
            end.into(),
            writer,
        )
        .await?;
        tr.commit().await.with_context(|| "commit")?;
        Ok(())
    }
}

fn format_postgres_placeholder(index: usize) -> String {
    format!("${}", index + 1)
}

fn serialize_record_batches(answer: &Answer) -> Result<bytes::Bytes> {
    let mut buffer_writer = bytes::BytesMut::with_capacity(1024).writer();
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::LZ4_RAW)
        .build();
    let mut arrow_writer =
        ArrowWriter::try_new(&mut buffer_writer, answer.schema.clone(), Some(props))?;
    for batch in &answer.record_batches {
        arrow_writer.write(batch)?;
    }
    arrow_writer.close()?;
    Ok(buffer_writer.into_inner().into())
}
