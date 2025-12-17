use super::metadata_cache::MetadataCache;
use super::partition_metadata::load_partition_metadata;
use bytes::Bytes;
use datafusion::{
    datasource::{listing::PartitionedFile, physical_plan::ParquetFileReaderFactory},
    parquet::{
        arrow::{
            arrow_reader::ArrowReaderOptions,
            async_reader::{AsyncFileReader, ParquetObjectReader},
        },
        file::metadata::ParquetMetaData,
    },
    physical_plan::metrics::ExecutionPlanMetricsSet,
};
use futures::future::BoxFuture;
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
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    pool: PgPool,
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
    /// Creates a new ReaderFactory with a shared metadata cache.
    pub fn new(
        object_store: Arc<dyn ObjectStore>,
        pool: PgPool,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        Self {
            object_store,
            pool,
            metadata_cache,
        }
    }
}

impl ParquetFileReaderFactory for ReaderFactory {
    fn create_reader(
        &self,
        _partition_index: usize,
        partitioned_file: PartitionedFile,
        metadata_size_hint: Option<usize>,
        _metrics: &ExecutionPlanMetricsSet,
    ) -> datafusion::error::Result<Box<dyn AsyncFileReader + Send>> {
        // todo: don't ignore metrics, report performance of the reader
        let filename = partitioned_file.path().to_string();
        let object_store = Arc::clone(&self.object_store);
        let mut inner = ParquetObjectReader::new(object_store, partitioned_file.path().clone());
        if let Some(hint) = metadata_size_hint {
            inner = inner.with_footer_size_hint(hint)
        };

        Ok(Box::new(ParquetReader {
            filename,
            pool: self.pool.clone(),
            metadata_cache: Arc::clone(&self.metadata_cache),
            inner,
        }))
    }
}

/// A wrapper around a `ParquetObjectReader` that loads metadata on-demand
/// using a shared global cache.
pub struct ParquetReader {
    pub filename: String,
    pub pool: PgPool,
    pub metadata_cache: Arc<MetadataCache>,
    pub inner: ParquetObjectReader,
}

impl AsyncFileReader for ParquetReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        self.inner.get_bytes(range)
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        self.inner.get_byte_ranges(ranges)
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
