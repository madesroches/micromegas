use anyhow::{Context, Result};
use bytes::Bytes;
use datafusion::parquet::arrow::async_reader::MetadataFetch;
use datafusion::parquet::errors::ParquetError;
use datafusion::parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use futures::FutureExt;
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::ops::Range;
use std::sync::Arc;

use super::caching_reader::CachingReader;
use super::metadata_cache::MetadataCache;
use crate::arrow_utils::parse_parquet_metadata;
use crate::lakehouse::metadata_compat;

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
    // Re-serialize - use Vec<u8> which auto-grows as needed
    let mut modified_bytes: Vec<u8> = Vec::with_capacity(file_metadata_bytes.len() * 2);
    let mut out_protocol = TCompactOutputProtocol::new(&mut modified_bytes);
    thrift_meta
        .write_to_out_protocol(&mut out_protocol)
        .context("serializing modified thrift metadata")?;
    out_protocol.flush()?;
    // Parse back to ParquetMetaData
    ParquetMetaDataReader::decode_metadata(&Bytes::copy_from_slice(&modified_bytes))
        .context("re-parsing metadata after stripping column index")
}

/// Load partition metadata by file path from the dedicated metadata table
///
/// Dispatches to appropriate parser based on partition_format_version:
/// - Version 1: Arrow 56.0 format, uses legacy parser with num_rows injection (requires additional query)
/// - Version 2: Arrow 57.0 format, uses standard parser (fast path, no join)
///
/// If a cache is provided, checks it first and stores results after loading.
#[span_fn]
pub async fn load_partition_metadata(
    pool: &PgPool,
    file_path: &str,
    cache: Option<&MetadataCache>,
) -> Result<Arc<ParquetMetaData>> {
    // Check cache first
    if let Some(cache) = cache
        && let Some(metadata) = cache.get(file_path).await
    {
        debug!("cache hit for partition metadata path={file_path}");
        return Ok(metadata);
    }

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
    let serialized_size = metadata_bytes.len() as u32;

    debug!("fetched partition metadata path={file_path} size={serialized_size}");
    // Dispatch based on format version
    let metadata = match partition_format_version {
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
    let stripped = strip_column_index_info(metadata)
        .with_context(|| format!("stripping column index for file: {}", file_path))?;
    let result = Arc::new(stripped);

    // Store in cache
    if let Some(cache) = cache {
        cache
            .insert(file_path.to_string(), result.clone(), serialized_size)
            .await;
    }

    Ok(result)
}

/// Adapts `CachingReader::get_bytes` to the `MetadataFetch` interface expected by
/// `ParquetMetaDataReader`, so the footer read benefits from the same object-cache-backed
/// byte caching as the rest of the file.
///
/// Also tallies the number of footer bytes fetched via `bytes_read`, so callers can use it
/// as a cheap proxy for the parsed metadata's weight in `MetadataCache` (see
/// `load_partition_metadata_from_footer`).
struct CachingReaderFetch<'a> {
    reader: &'a mut CachingReader,
    bytes_read: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl MetadataFetch for CachingReaderFetch<'_> {
    fn fetch(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        self.bytes_read.fetch_add(
            range.end - range.start,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.reader.get_bytes(range).boxed()
    }
}

/// Read and parse a partition's parquet footer directly from object storage via the
/// object-cache-backed reader, bypassing the postgres partition_metadata table.
/// Still checks and backfills the shared `MetadataCache` parsed-lookaside, so on a cache hit
/// this path is identical to `load_partition_metadata`; the two read paths differ only in the
/// miss-backfill source (object-cache footer read vs. postgres round-trip) and are otherwise
/// behaviorally interchangeable and A/B comparable.
#[span_fn]
pub async fn load_partition_metadata_from_footer(
    reader: &mut CachingReader,
    file_path: &str,
    file_size: u64,
    cache: Option<&MetadataCache>,
) -> datafusion::parquet::errors::Result<Arc<ParquetMetaData>> {
    // Check cache first
    if let Some(cache) = cache
        && let Some(metadata) = cache.get(file_path).await
    {
        debug!("cache hit for partition metadata path={file_path}");
        return Ok(metadata);
    }

    let start = std::time::Instant::now();
    let bytes_read = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let raw = ParquetMetaDataReader::new()
        .load_and_finish(
            CachingReaderFetch {
                reader,
                bytes_read: bytes_read.clone(),
            },
            file_size,
        )
        .await?;
    let stripped = strip_column_index_info(raw).map_err(|e| ParquetError::External(e.into()))?;
    let duration_ms = start.elapsed().as_millis();
    debug!(
        "partition_metadata_footer_read file={file_path} file_size={file_size} duration_ms={duration_ms}"
    );
    let result = Arc::new(stripped);

    // Store in cache. There's no serialized footer bytes on this path (unlike the postgres
    // path, which stores a pre-serialized blob), so use the number of footer bytes actually
    // read from object storage as the weight: it's a natural, cheap proxy for the parsed
    // metadata's size, comparable to the postgres path's stored serialized size, without
    // paying for an extra re-serialization pass just to compute a weight.
    if let Some(cache) = cache {
        let weight = bytes_read.load(std::sync::atomic::Ordering::Relaxed);
        let weight = u32::try_from(weight).unwrap_or(u32::MAX);
        cache
            .insert(file_path.to_string(), result.clone(), weight)
            .await;
    }

    Ok(result)
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
