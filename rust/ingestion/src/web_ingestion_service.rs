use crate::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use crate::remote_data_lake::migrate_db;
use anyhow::Context;
use bytes::Buf;
use micromegas_telemetry::block_wire_format;
use micromegas_telemetry::property::Property;
use micromegas_telemetry::property::make_properties;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

/// Sentinel value for `dependencies_metadata` / `objects_metadata` on streams that
/// do not use the transit/POD wire format (e.g. OTLP).
///
/// The single byte 0x80 is the CBOR encoding of an empty array. Every existing
/// codepath that reads these BYTEA columns runs them through
/// `ciborium::from_reader::<Vec<UserDefinedType>>(...)` and iterates the result;
/// decoding 0x80 yields an empty Vec, so those iterations become no-ops without
/// touching any of the consumer code.
pub const EMPTY_TRANSIT_METADATA_CBOR: &[u8] = &[0x80];

/// Format string for native streams (transit-encoded payload, CBOR envelope).
pub const FORMAT_TRANSIT: &str = "micromegas-transit";

/// Stream `format` value for OTel logs (one `ResourceLogs` proto per block payload).
pub const FORMAT_OTLP_LOGS: &str = "otlp/v1/logs";

/// Stream `format` value for OTel metrics (one `ResourceMetrics` proto per block payload).
pub const FORMAT_OTLP_METRICS: &str = "otlp/v1/metrics";

/// Stream `format` value for OTel traces (one `ResourceSpans` proto per block payload).
pub const FORMAT_OTLP_TRACES: &str = "otlp/v1/traces";

/// Error type for ingestion service operations.
/// Categorizes errors to enable proper HTTP status code mapping.
#[derive(Error, Debug)]
pub enum IngestionServiceError {
    /// Client-side errors (malformed input) - maps to 400 Bad Request
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Database errors - maps to 500 Internal Server Error
    #[error("Database error: {0}")]
    DatabaseError(String),

    /// Object storage errors - maps to 500 Internal Server Error
    #[error("Storage error: {0}")]
    StorageError(String),
}

#[derive(Clone)]
pub struct WebIngestionService {
    lake: DataLakeConnection,
}

impl WebIngestionService {
    pub fn new(lake: DataLakeConnection) -> Self {
        Self { lake }
    }

    /// Reads MICROMEGAS_SQL_CONNECTION_STRING and MICROMEGAS_OBJECT_STORE_URI,
    /// connects to the data lake, runs ingestion migrations, and returns
    /// a ready-to-use service.
    pub async fn from_env() -> anyhow::Result<Arc<Self>> {
        let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
            .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
        let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
            .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
        let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
        migrate_db(lake.db_pool.clone())
            .await
            .with_context(|| "migrate_db")?;
        Ok(Arc::new(Self::new(lake)))
    }

    #[span_fn]
    pub async fn insert_block(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing block: {e}")))?;
        self.insert_block_typed(block).await
    }

    /// Inserts a block whose payload is already typed (no envelope round-trip on the caller side).
    ///
    /// The caller hands us a fully-built `Block`; we CBOR-encode the payload envelope once,
    /// write it to object storage, and INSERT the row. Used by the OTLP adapter where
    /// constructing the CBOR `Block` envelope just so `insert_block` could decode it
    /// would be wasted work.
    #[span_fn]
    pub async fn insert_block_typed(
        &self,
        block: block_wire_format::Block,
    ) -> Result<(), IngestionServiceError> {
        let encoded_payload = encode_cbor(&block.payload)
            .map_err(|e| IngestionServiceError::ParseError(format!("encoding payload: {e}")))?;
        let payload_size = encoded_payload.len();

        let process_id = &block.process_id;
        let stream_id = &block.stream_id;
        let block_id = &block.block_id;
        let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        debug!("writing {obj_path}");

        use sqlx::types::chrono::{DateTime, FixedOffset};
        let begin_time = DateTime::<FixedOffset>::parse_from_rfc3339(&block.begin_time)
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing begin_time: {e}")))?;
        let end_time = DateTime::<FixedOffset>::parse_from_rfc3339(&block.end_time)
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing end_time: {e}")))?;
        {
            let begin_put = now();
            self.lake
                .blob_storage
                .put(&obj_path, encoded_payload.into())
                .await
                .map_err(|e| {
                    IngestionServiceError::StorageError(format!(
                        "writing block to blob storage: {e}"
                    ))
                })?;
            imetric!("put_duration", "ticks", (now() - begin_put) as u64);
        }

        debug!("recording block_id={block_id} stream_id={stream_id} process_id={process_id}");
        let begin_insert = now();
        let insert_time = sqlx::types::chrono::Utc::now();
        let result = sqlx::query(
            "INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (block_id) DO NOTHING;",
        )
        .bind(block_id)
        .bind(stream_id)
        .bind(process_id)
        .bind(begin_time)
        .bind(block.begin_ticks)
        .bind(end_time)
        .bind(block.end_ticks)
        .bind(block.nb_objects)
        .bind(block.object_offset)
        .bind(payload_size as i64)
        .bind(insert_time)
        .execute(&self.lake.db_pool)
        .await
        .map_err(|e| IngestionServiceError::DatabaseError(format!("inserting into blocks: {e}")))?;
        imetric!("insert_duration", "ticks", (now() - begin_insert) as u64);

        if result.rows_affected() == 0 {
            debug!("duplicate block_id={block_id} skipped (already exists)");
        }
        // this measure does not benefit from a dynamic property - I just want to make sure the feature works well
        // the cost in this context should be reasonnable
        imetric!(
            "payload_size_inserted",
            "bytes",
            property_set::PropertySet::find_or_create(vec![property_set::Property::new(
                "target",
                "micromegas::ingestion"
            ),]),
            payload_size as u64
        );
        debug!("recorded block_id={block_id} stream_id={stream_id} process_id={process_id}");

        Ok(())
    }

    #[span_fn]
    pub async fn insert_stream(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let stream_info: StreamInfo = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing StreamInfo: {e}")))?;
        info!(
            "new stream {} {:?} {:?}",
            stream_info.stream_id, &stream_info.tags, &stream_info.properties
        );
        let dependencies_metadata =
            encode_cbor(&stream_info.dependencies_metadata).map_err(|e| {
                IngestionServiceError::ParseError(format!("encoding dependencies_metadata: {e}"))
            })?;
        let objects_metadata = encode_cbor(&stream_info.objects_metadata).map_err(|e| {
            IngestionServiceError::ParseError(format!("encoding objects_metadata: {e}"))
        })?;
        let result = sqlx::query(
            "INSERT INTO streams (stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties, insert_time, format)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (stream_id) DO NOTHING;",
        )
        .bind(stream_info.stream_id)
        .bind(stream_info.process_id)
        .bind(dependencies_metadata)
        .bind(objects_metadata)
        .bind(&stream_info.tags)
        .bind(make_properties(&stream_info.properties))
        .bind(sqlx::types::chrono::Utc::now())
        .bind(FORMAT_TRANSIT)
        .execute(&self.lake.db_pool)
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting into streams: {e}"))
        })?;

        if result.rows_affected() == 0 {
            debug!(
                "duplicate stream_id={} skipped (already exists)",
                stream_info.stream_id
            );
        }
        Ok(())
    }

    /// Registers a stream produced by an OTLP ingestion path.
    ///
    /// `dependencies_metadata` and `objects_metadata` are filled with the CBOR sentinel
    /// for an empty `Vec<UserDefinedType>` so legacy decode sites continue to work.
    /// `format` distinguishes per-block dispatch downstream (e.g. `"otlp/v1/logs"`).
    #[span_fn]
    pub async fn register_otel_stream(
        &self,
        stream_id: Uuid,
        process_id: Uuid,
        tags: Vec<String>,
        properties: Vec<Property>,
        format: &str,
    ) -> Result<(), IngestionServiceError> {
        let result = sqlx::query(
            "INSERT INTO streams (stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties, insert_time, format)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (stream_id) DO NOTHING;",
        )
        .bind(stream_id)
        .bind(process_id)
        .bind(EMPTY_TRANSIT_METADATA_CBOR)
        .bind(EMPTY_TRANSIT_METADATA_CBOR)
        .bind(tags)
        .bind(properties)
        .bind(sqlx::types::chrono::Utc::now())
        .bind(format)
        .execute(&self.lake.db_pool)
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting otel stream: {e}"))
        })?;

        if result.rows_affected() == 0 {
            debug!("duplicate otel stream_id={stream_id} skipped (already exists)");
        }
        Ok(())
    }

    #[span_fn]
    pub async fn insert_process(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let process_info: ProcessInfo = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing ProcessInfo: {e}")))?;

        let insert_time = sqlx::types::chrono::Utc::now();
        let result = sqlx::query(
            "INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT (process_id) DO NOTHING;",
        )
        .bind(process_info.process_id)
        .bind(process_info.exe)
        .bind(process_info.username)
        .bind(process_info.realname)
        .bind(process_info.computer)
        .bind(process_info.distro)
        .bind(process_info.cpu_brand)
        .bind(process_info.tsc_frequency)
        .bind(process_info.start_time)
        .bind(process_info.start_ticks)
        .bind(insert_time)
        .bind(process_info.parent_process_id)
        .bind(make_properties(&process_info.properties))
        .execute(&self.lake.db_pool)
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting into processes: {e}"))
        })?;

        if result.rows_affected() == 0 {
            debug!(
                "duplicate process_id={} skipped (already exists)",
                process_info.process_id
            );
        }
        Ok(())
    }

    /// Registers a process originating from OTLP. Idempotent via `ON CONFLICT DO NOTHING`.
    ///
    /// `realname` is set equal to `username` (OTel has no separate "real name" concept).
    /// `parent_process_id` is always NULL — OTel has no parent-process model.
    /// `insert_time` is the server wall clock, matching the existing `insert_process` path.
    #[span_fn]
    #[expect(clippy::too_many_arguments, reason = "OTel process identity fields")]
    pub async fn register_otel_process(
        &self,
        process_id: Uuid,
        exe: String,
        username: String,
        computer: String,
        distro: String,
        cpu_brand: String,
        tsc_frequency: i64,
        start_time: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
        start_ticks: i64,
        properties: Vec<Property>,
    ) -> Result<(), IngestionServiceError> {
        let insert_time = sqlx::types::chrono::Utc::now();
        let result = sqlx::query(
            "INSERT INTO processes
             (process_id, exe, username, realname, computer, distro, cpu_brand,
              tsc_frequency, start_time, start_ticks, insert_time, parent_process_id, properties)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,NULL,$12)
             ON CONFLICT (process_id) DO NOTHING;",
        )
        .bind(process_id)
        .bind(exe)
        .bind(&username)
        .bind(&username)
        .bind(computer)
        .bind(distro)
        .bind(cpu_brand)
        .bind(tsc_frequency)
        .bind(start_time)
        .bind(start_ticks)
        .bind(insert_time)
        .bind(properties)
        .execute(&self.lake.db_pool)
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting otel process: {e}"))
        })?;

        if result.rows_affected() == 0 {
            debug!("duplicate otel process_id={process_id} skipped (already exists)");
        }
        Ok(())
    }
}
