use crate::data_lake_connection::DataLakeConnection;
use bytes::Buf;
use micromegas_telemetry::block_wire_format;
use micromegas_telemetry::property::make_properties;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set;
use thiserror::Error;

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

    #[span_fn]
    pub async fn insert_block(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing block: {e}")))?;
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
            "INSERT INTO blocks
             SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11
             WHERE NOT EXISTS (SELECT 1 FROM blocks WHERE block_id = $1);",
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
            "INSERT INTO streams
             SELECT $1,$2,$3,$4,$5,$6,$7
             WHERE NOT EXISTS (SELECT 1 FROM streams WHERE stream_id = $1);",
        )
        .bind(stream_info.stream_id)
        .bind(stream_info.process_id)
        .bind(dependencies_metadata)
        .bind(objects_metadata)
        .bind(&stream_info.tags)
        .bind(make_properties(&stream_info.properties))
        .bind(sqlx::types::chrono::Utc::now())
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

    #[span_fn]
    pub async fn insert_process(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let process_info: ProcessInfo = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing ProcessInfo: {e}")))?;

        let insert_time = sqlx::types::chrono::Utc::now();
        let result = sqlx::query(
            "INSERT INTO processes
             SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13
             WHERE NOT EXISTS (SELECT 1 FROM processes WHERE process_id = $1);",
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
}
