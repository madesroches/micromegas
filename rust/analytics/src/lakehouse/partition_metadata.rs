use anyhow::{Context, Result};
use bytes::Bytes;
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::sync::Arc;

use crate::arrow_utils::parse_parquet_metadata;
use crate::lakehouse::metadata_compat;
use datafusion::parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};

/// Strips column index information from Parquet metadata
///
/// This removes column_index_offset and column_index_length from ColumnChunk metadata
/// to prevent DataFusion from trying to read legacy ColumnIndex structures that may
/// have incomplete or malformed null_pages fields (required in Arrow 57.0+).
///
/// The approach: serialize metadata to thrift, modify it, then re-parse.
#[allow(deprecated)]
fn strip_column_index_info(metadata: ParquetMetaData) -> Result<ParquetMetaData> {
    use datafusion::parquet::file::metadata::ParquetMetaDataWriter;
    use parquet::format::FileMetaData as ThriftFileMetaData;
    use parquet::thrift::TSerializable;
    use thrift::protocol::{TCompactInputProtocol, TCompactOutputProtocol, TOutputProtocol};
    // Serialize metadata using ParquetMetaDataWriter
    let mut buffer = Vec::new();
    let writer = ParquetMetaDataWriter::new(&mut buffer, &metadata);
    writer.finish()?;
    // Extract FileMetaData portion (similar to serialize_parquet_metadata)
    // Format: [Page Indexes][FileMetaData][Length][PAR1]
    let metadata_len = u32::from_le_bytes([
        buffer[buffer.len() - 8],
        buffer[buffer.len() - 7],
        buffer[buffer.len() - 6],
        buffer[buffer.len() - 5],
    ]) as usize;
    let file_metadata_start = buffer.len() - 8 - metadata_len;
    let file_metadata_bytes = &buffer[file_metadata_start..buffer.len() - 8];
    // Parse FileMetaData with thrift
    let mut transport =
        thrift::transport::TBufferChannel::with_capacity(file_metadata_bytes.len(), 0);
    transport.set_readable_bytes(file_metadata_bytes);
    let mut protocol = TCompactInputProtocol::new(transport);
    let mut thrift_meta = ThriftFileMetaData::read_from_in_protocol(&mut protocol)
        .context("parsing thrift metadata to strip column index")?;
    // Remove column index information from all row groups and columns
    for rg in thrift_meta.row_groups.iter_mut() {
        for col in rg.columns.iter_mut() {
            col.column_index_offset = None;
            col.column_index_length = None;
            // Also remove offset index for consistency
            col.offset_index_offset = None;
            col.offset_index_length = None;
        }
    }
    // Re-serialize
    let mut out_transport = thrift::transport::TBufferChannel::with_capacity(0, 8192);
    let mut out_protocol = TCompactOutputProtocol::new(&mut out_transport);
    thrift_meta
        .write_to_out_protocol(&mut out_protocol)
        .context("serializing modified thrift metadata")?;
    out_protocol.flush()?;
    let modified_bytes = out_transport.write_bytes();
    // Parse back to ParquetMetaData
    ParquetMetaDataReader::decode_metadata(&Bytes::copy_from_slice(&modified_bytes))
        .context("re-parsing metadata after stripping column index")
}

/// Load partition metadata by file path from the dedicated metadata table
///
/// Dispatches to appropriate parser based on partition_format_version:
/// - Version 1: Arrow 56.0 format, uses legacy parser with num_rows injection (requires additional query)
/// - Version 2: Arrow 57.0 format, uses standard parser (fast path, no join)
#[span_fn]
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
) -> Result<Arc<ParquetMetaData>> {
    // Fast path: query only partition_metadata table (no join)
    let row = sqlx::query(
        "SELECT metadata, partition_format_version
         FROM partition_metadata
         WHERE file_path = $1",
    )
    .bind(file_path)
    .fetch_one(pool)
    .await
    .with_context(|| format!("loading metadata for file: {}", file_path))?;
    let metadata_bytes: Vec<u8> = row.try_get("metadata")?;
    let partition_format_version: i32 = row.try_get("partition_format_version")?;
    // Dispatch based on format version
    let mut metadata = match partition_format_version {
        1 => {
            // Arrow 56.0 format - need num_rows from lakehouse_partitions for legacy parser
            let num_rows_row =
                sqlx::query("SELECT num_rows FROM lakehouse_partitions WHERE file_path = $1")
                    .bind(file_path)
                    .fetch_one(pool)
                    .await
                    .with_context(|| format!("loading num_rows for v1 partition: {}", file_path))?;
            let num_rows: i64 = num_rows_row.try_get("num_rows")?;
            metadata_compat::parse_legacy_and_upgrade(&metadata_bytes, num_rows)
                .with_context(|| format!("parsing v1 metadata for file: {}", file_path))?
        }
        2 => {
            // Arrow 57.0 format - use standard parser (no additional query needed)
            parse_parquet_metadata(&metadata_bytes.into())
                .with_context(|| format!("parsing v2 metadata for file: {}", file_path))?
        }
        _ => {
            return Err(anyhow::anyhow!(
                "unsupported partition_format_version {} for file: {}",
                partition_format_version,
                file_path
            ));
        }
    };
    // Remove column index information to prevent DataFusion from trying to read
    // legacy ColumnIndex structures that may have incomplete null_pages fields
    metadata = strip_column_index_info(metadata)?;
    Ok(Arc::new(metadata))
}

/// Delete multiple partition metadata entries in a single transaction
/// Uses PostgreSQL's ANY() with array to avoid placeholder limits
#[span_fn]
pub async fn delete_partition_metadata_batch(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_paths: &[String],
) -> Result<()> {
    if file_paths.is_empty() {
        return Ok(());
    }
    // Use PostgreSQL's ANY() with array - no placeholder limits
    let result = sqlx::query("DELETE FROM partition_metadata WHERE file_path = ANY($1)")
        .bind(file_paths)
        .execute(&mut **tr)
        .await
        .with_context(|| format!("deleting {} metadata entries", file_paths.len()))?;
    debug!("deleted {} metadata entries", result.rows_affected());
    Ok(())
}
