use super::partition_metadata::load_partition_metadata;
use anyhow::{Context, Result};
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
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use sqlx::PgPool;
use std::ops::Range;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A custom [`ParquetFileReaderFactory`] that handles opening parquet files
/// from object storage, and loads metadata on-demand.
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    pool: PgPool,
}

impl std::fmt::Debug for ReaderFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaderFactory").finish()
    }
}

impl ReaderFactory {
    pub fn new(object_store: Arc<dyn ObjectStore>, pool: PgPool) -> Self {
        Self { object_store, pool }
    }
}

async fn load_parquet_metadata(filename: &str, pool: &PgPool) -> Result<Arc<ParquetMetaData>> {
    // Load metadata on-demand using the dedicated metadata table
    load_partition_metadata(pool, filename)
        .await
        .with_context(|| format!("[reader_factory] loading metadata for {filename}"))
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
            metadata: Arc::new(Mutex::new(None)), // Load on-demand in get_metadata()
            inner,
        }))
    }
}

/// A wrapper around a `ParquetObjectReader` that loads metadata on-demand.
pub struct ParquetReader {
    pub filename: String,
    pub pool: PgPool,
    pub metadata: Arc<Mutex<Option<Arc<ParquetMetaData>>>>, // Thread-safe cached metadata
    pub inner: ParquetObjectReader,
}

impl AsyncFileReader for ParquetReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        // debug!("ParquetReader::get_bytes {}", &self.filename);
        self.inner.get_bytes(range)
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        // debug!("ParquetReader::get_byte_ranges {}", &self.filename);
        self.inner.get_byte_ranges(ranges)
    }

    fn get_metadata(
        &mut self,
        options: Option<&ArrowReaderOptions>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        let metadata_cache = self.metadata.clone();
        let pool = self.pool.clone();
        let filename = self.filename.clone();
        // Use options if provided, otherwise create default with page index disabled
        let _options = options.cloned().unwrap_or_else(|| {
            ArrowReaderOptions::new().with_page_index(false) // Disable for backward compatibility with legacy Parquet files
        });

        Box::pin(async move {
            // Check if we already have metadata cached
            {
                let lock = metadata_cache.lock().await;
                if let Some(metadata) = &*lock {
                    debug!("reusing cached metadata");
                    return Ok(metadata.clone());
                }
            }

            // Load metadata from database, with options applied
            let metadata = load_parquet_metadata(&filename, &pool)
                .await
                .map_err(|e| datafusion::parquet::errors::ParquetError::External(e.into()))?;

            // Note: Page index reading is disabled via ArrowReaderOptions above
            // for backward compatibility with legacy Parquet files that may have
            // incomplete ColumnIndex metadata (missing null_pages field)

            // Cache the metadata for future calls
            {
                let mut lock = metadata_cache.lock().await;
                *lock = Some(metadata.clone());
            }

            Ok(metadata)
        })
    }
}
