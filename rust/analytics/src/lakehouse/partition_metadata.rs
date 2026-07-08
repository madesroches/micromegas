use anyhow::{Context, Result};
use bytes::Bytes;
use datafusion::parquet::arrow::async_reader::MetadataFetch;
use datafusion::parquet::errors::ParquetError;
use datafusion::parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use futures::FutureExt;
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use object_store::{ObjectStore, ObjectStoreExt, path::Path};
use std::ops::Range;
use std::sync::Arc;

use super::metadata_cache::MetadataCache;

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
    // Extract FileMetaData portion: the parquet footer is laid out as
    // [Page Indexes][FileMetaData][Length][PAR1]
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

/// Adapts `ObjectStore::get_range` to the `MetadataFetch` interface expected by
/// `ParquetMetaDataReader`, so the footer read benefits from the same
/// object-store-backed byte caching as the rest of the file (the object store
/// itself may be L1-cache-backed, see `object_cache::l1_wrap`).
///
/// Also tallies the number of footer bytes fetched via `bytes_read`, so callers can use it
/// as a cheap proxy for the parsed metadata's weight in `MetadataCache` (see
/// `load_partition_metadata`).
struct ObjectStoreFetch<'a> {
    object_store: &'a Arc<dyn ObjectStore>,
    path: &'a Path,
    bytes_read: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl MetadataFetch for ObjectStoreFetch<'_> {
    fn fetch(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        self.bytes_read.fetch_add(
            range.end - range.start,
            std::sync::atomic::Ordering::Relaxed,
        );
        let object_store = self.object_store;
        let path = self.path;
        async move {
            object_store
                .get_range(path, range)
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))
        }
        .boxed()
    }
}

/// Read and parse a partition's parquet footer directly from `object_store` — the sole
/// partition-metadata read path. Checks and backfills the shared `MetadataCache`
/// parsed-lookaside: on a cache hit this returns immediately, on a miss it reads the
/// footer from `object_store` (which may itself be L1-cache-backed) and stores the parsed
/// result before returning it.
#[span_fn]
pub async fn load_partition_metadata(
    object_store: &Arc<dyn ObjectStore>,
    path: &Path,
    file_size: u64,
    cache: Option<&MetadataCache>,
) -> datafusion::parquet::errors::Result<Arc<ParquetMetaData>> {
    let file_path = path.as_ref();

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
            ObjectStoreFetch {
                object_store,
                path,
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

    // Store in cache. There's no pre-serialized footer blob to measure the size of here, so
    // use the number of footer bytes actually read from object storage as the weight: it's a
    // natural, cheap proxy for the parsed metadata's size, without paying for an extra
    // re-serialization pass just to compute a weight.
    if let Some(cache) = cache {
        let weight = bytes_read.load(std::sync::atomic::Ordering::Relaxed);
        let weight = u32::try_from(weight).unwrap_or(u32::MAX);
        cache
            .insert(file_path.to_string(), result.clone(), weight)
            .await;
    }

    Ok(result)
}
