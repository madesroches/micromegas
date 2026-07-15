use super::metadata_cache::MetadataCache;
use super::partition_metadata::load_partition_metadata;
use bytes::Bytes;
use datafusion::{
    datasource::{
        listing::PartitionedFile,
        physical_plan::{ParquetFileMetrics, ParquetFileReaderFactory},
    },
    parquet::{
        arrow::{arrow_reader::ArrowReaderOptions, async_reader::AsyncFileReader},
        file::metadata::ParquetMetaData,
    },
    physical_plan::metrics::{Count, ExecutionPlanMetricsSet},
};
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use object_store::{ObjectStore, ObjectStoreExt, path::Path};
use std::ops::Range;
use std::sync::Arc;

/// A custom [`ParquetFileReaderFactory`] that handles opening parquet files
/// from object storage, and loads metadata on-demand.
///
/// Parsed metadata is cached globally across all readers and queries via a shared
/// `MetadataCache`, significantly reducing repeated object-storage footer reads for
/// repeated queries on the same partitions.
///
/// File content caching is the responsibility of `object_store` itself: this
/// factory is typically constructed with an object store already wrapped by the
/// in-process L1 cache (see `object_cache::l1_wrap`), so it just reads bytes
/// through it directly.
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    metadata_cache: Arc<MetadataCache>,
}

impl std::fmt::Debug for ReaderFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaderFactory")
            .field("metadata_cache", &self.metadata_cache)
            .finish()
    }
}

impl ReaderFactory {
    /// Creates a new ReaderFactory with a shared metadata cache, reading file
    /// contents through `object_store`.
    pub fn new(object_store: Arc<dyn ObjectStore>, metadata_cache: Arc<MetadataCache>) -> Self {
        Self {
            object_store,
            metadata_cache,
        }
    }
}

impl ParquetFileReaderFactory for ReaderFactory {
    fn create_reader(
        &self,
        partition_index: usize,
        partitioned_file: PartitionedFile,
        _metadata_size_hint: Option<usize>,
        metrics: &ExecutionPlanMetricsSet,
    ) -> datafusion::error::Result<Box<dyn AsyncFileReader + Send>> {
        let path = partitioned_file.path().clone();
        let filename = path.to_string();
        let file_size = partitioned_file.object_meta.size;
        let file_metrics = ParquetFileMetrics::new(partition_index, path.as_ref(), metrics);

        Ok(Box::new(ParquetReader {
            filename,
            file_size,
            metadata_cache: Arc::clone(&self.metadata_cache),
            object_store: Arc::clone(&self.object_store),
            path,
            bytes_scanned: file_metrics.bytes_scanned,
        }))
    }
}

/// Reads a parquet file's bytes and metadata directly from its `object_store`
/// (which may itself be L1-cache-backed, see `object_cache::l1_wrap`) and a
/// shared `MetadataCache`.
pub struct ParquetReader {
    pub filename: String,
    pub file_size: u64,
    pub metadata_cache: Arc<MetadataCache>,
    pub object_store: Arc<dyn ObjectStore>,
    pub path: Path,
    pub bytes_scanned: Count,
}

impl AsyncFileReader for ParquetReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        let filename = self.filename.clone();
        let file_size = self.file_size;
        let bytes_requested = range.end - range.start;
        let object_store = Arc::clone(&self.object_store);
        let path = self.path.clone();
        let bytes_scanned = self.bytes_scanned.clone();

        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = object_store
                .get_range(&path, range)
                .await
                .map_err(|e| datafusion::parquet::errors::ParquetError::External(Box::new(e)));
            let duration_ms = start.elapsed().as_millis();

            debug!(
                "parquet_read file={filename} file_size={file_size} bytes={bytes_requested} duration_ms={duration_ms}"
            );
            bytes_scanned.add(bytes_requested as usize);

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
        let object_store = Arc::clone(&self.object_store);
        let path = self.path.clone();
        let bytes_scanned = self.bytes_scanned.clone();

        Box::pin(async move {
            let start = std::time::Instant::now();
            let result = object_store
                .get_ranges(&path, &ranges)
                .await
                .map_err(|e| datafusion::parquet::errors::ParquetError::External(Box::new(e)));
            let duration_ms = start.elapsed().as_millis();

            debug!(
                "parquet_read file={filename} file_size={file_size} ranges={num_ranges} bytes={total_bytes} duration_ms={duration_ms}"
            );
            bytes_scanned.add(total_bytes as usize);

            result
        })
    }

    fn get_metadata(
        &mut self,
        _options: Option<&ArrowReaderOptions>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        let metadata_cache = Arc::clone(&self.metadata_cache);
        let object_store = Arc::clone(&self.object_store);
        let path = self.path.clone();
        let file_size = self.file_size;
        Box::pin(async move {
            load_partition_metadata(&object_store, &path, file_size, Some(&metadata_cache)).await
        })
    }
}
