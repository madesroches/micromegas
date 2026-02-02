use super::caching_reader::CachingReader;
use super::file_cache::FileCache;
use super::metadata_cache::MetadataCache;
use super::partition_metadata::load_partition_metadata;
use bytes::Bytes;
use datafusion::{
    datasource::{listing::PartitionedFile, physical_plan::ParquetFileReaderFactory},
    parquet::{
        arrow::{arrow_reader::ArrowReaderOptions, async_reader::AsyncFileReader},
        file::metadata::ParquetMetaData,
    },
    physical_plan::metrics::ExecutionPlanMetricsSet,
};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use sqlx::PgPool;
use std::ops::Range;
use std::sync::Arc;

/// A custom [`ParquetFileReaderFactory`] that handles opening parquet files
/// from object storage, and loads metadata on-demand.
///
/// Metadata is cached globally across all readers and queries via a shared
/// `MetadataCache`, significantly reducing database fetches for repeated
/// queries on the same partitions.
///
/// File contents are cached via a shared `FileCache` with thundering herd
/// protection, reducing object storage reads for frequently accessed files.
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    pool: PgPool,
    metadata_cache: Arc<MetadataCache>,
    file_cache: Arc<FileCache>,
}

impl std::fmt::Debug for ReaderFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaderFactory")
            .field("metadata_cache", &self.metadata_cache)
            .field("file_cache", &self.file_cache)
            .finish()
    }
}

impl ReaderFactory {
    /// Creates a new ReaderFactory with shared metadata and file caches.
    pub fn new(
        object_store: Arc<dyn ObjectStore>,
        pool: PgPool,
        metadata_cache: Arc<MetadataCache>,
        file_cache: Arc<FileCache>,
    ) -> Self {
        Self {
            object_store,
            pool,
            metadata_cache,
            file_cache,
        }
    }
}

impl ParquetFileReaderFactory for ReaderFactory {
    fn create_reader(
        &self,
        _partition_index: usize,
        partitioned_file: PartitionedFile,
        _metadata_size_hint: Option<usize>,
        _metrics: &ExecutionPlanMetricsSet,
    ) -> datafusion::error::Result<Box<dyn AsyncFileReader + Send>> {
        // todo: don't ignore metrics, report performance of the reader
        let path = partitioned_file.path().clone();
        let filename = path.to_string();
        let file_size = partitioned_file.object_meta.size;

        // CachingReader handles file content caching with thundering herd protection
        let inner = CachingReader::new(
            Arc::clone(&self.object_store),
            path,
            filename.clone(),
            file_size,
            Arc::clone(&self.file_cache),
        );

        Ok(Box::new(ParquetReader {
            filename,
            file_size,
            pool: self.pool.clone(),
            metadata_cache: Arc::clone(&self.metadata_cache),
            inner,
        }))
    }
}

/// A wrapper around a `CachingReader` that loads metadata on-demand
/// using a shared global cache.
pub struct ParquetReader {
    pub filename: String,
    pub file_size: u64,
    pub pool: PgPool,
    pub metadata_cache: Arc<MetadataCache>,
    pub inner: CachingReader,
}

impl AsyncFileReader for ParquetReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        let filename = self.filename.clone();
        let file_size = self.file_size;
        let bytes_requested = range.end - range.start;
        let inner = &mut self.inner;

        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = inner.get_bytes(range).await;
            let duration_ms = start.elapsed().as_millis();

            debug!(
                "object_storage_read file={filename} file_size={file_size} bytes={bytes_requested} duration_ms={duration_ms}"
            );

            result
        })
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        let filename = self.filename.clone();
        let file_size = self.file_size;
        let num_ranges = ranges.len();
        let total_bytes: u64 = ranges.iter().map(|r| r.end - r.start).sum();
        let inner = &mut self.inner;

        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = inner.get_byte_ranges(ranges).await;
            let duration_ms = start.elapsed().as_millis();

            debug!(
                "object_storage_read file={filename} file_size={file_size} ranges={num_ranges} bytes={total_bytes} duration_ms={duration_ms}"
            );

            result
        })
    }

    fn get_metadata(
        &mut self,
        _options: Option<&ArrowReaderOptions>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        let metadata_cache = Arc::clone(&self.metadata_cache);
        let pool = self.pool.clone();
        let filename = self.filename.clone();

        Box::pin(async move {
            // Load metadata using the shared cache (handles cache hit/miss internally)
            load_partition_metadata(&pool, &filename, Some(&metadata_cache))
                .await
                .map_err(|e| datafusion::parquet::errors::ParquetError::External(e.into()))
        })
    }
}
