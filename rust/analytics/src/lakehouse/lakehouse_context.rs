use super::file_cache::FileCache;
use super::metadata_cache::MetadataCache;
use super::reader_factory::ReaderFactory;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Default metadata cache size in MB
const DEFAULT_METADATA_CACHE_SIZE_MB: u64 = 50;

/// Default file cache size in MB
const DEFAULT_FILE_CACHE_SIZE_MB: u64 = 200;

/// Default max file size to cache in MB
const DEFAULT_FILE_CACHE_MAX_FILE_MB: u64 = 10;

/// Bundles all runtime resources needed for lakehouse query execution.
///
/// This struct holds the data lake connection, metadata cache, file cache, and DataFusion runtime,
/// providing a single context object that can be passed through the query path.
#[derive(Clone)]
pub struct LakehouseContext {
    lake: Arc<DataLakeConnection>,
    metadata_cache: Arc<MetadataCache>,
    file_cache: Arc<FileCache>,
    runtime: Arc<RuntimeEnv>,
    reader_factory: Arc<ReaderFactory>,
}

impl LakehouseContext {
    /// Creates a new lakehouse context with default-sized metadata and file caches.
    pub fn new(lake: Arc<DataLakeConnection>, runtime: Arc<RuntimeEnv>) -> Self {
        let metadata_cache_mb = match std::env::var("MICROMEGAS_METADATA_CACHE_MB") {
            Ok(s) => s.parse::<u64>().unwrap_or_else(|_| {
                warn!(
                    "Invalid MICROMEGAS_METADATA_CACHE_MB value '{s}', using default {DEFAULT_METADATA_CACHE_SIZE_MB} MB"
                );
                DEFAULT_METADATA_CACHE_SIZE_MB
            }),
            Err(_) => DEFAULT_METADATA_CACHE_SIZE_MB,
        };

        let file_cache_mb = match std::env::var("MICROMEGAS_FILE_CACHE_MB") {
            Ok(s) => s.parse::<u64>().unwrap_or_else(|_| {
                warn!(
                    "Invalid MICROMEGAS_FILE_CACHE_MB value '{s}', using default {DEFAULT_FILE_CACHE_SIZE_MB} MB"
                );
                DEFAULT_FILE_CACHE_SIZE_MB
            }),
            Err(_) => DEFAULT_FILE_CACHE_SIZE_MB,
        };

        let file_cache_max_file_mb = match std::env::var("MICROMEGAS_FILE_CACHE_MAX_FILE_MB") {
            Ok(s) => s.parse::<u64>().unwrap_or_else(|_| {
                warn!(
                    "Invalid MICROMEGAS_FILE_CACHE_MAX_FILE_MB value '{s}', using default {DEFAULT_FILE_CACHE_MAX_FILE_MB} MB"
                );
                DEFAULT_FILE_CACHE_MAX_FILE_MB
            }),
            Err(_) => DEFAULT_FILE_CACHE_MAX_FILE_MB,
        };

        let metadata_cache = Arc::new(MetadataCache::new(metadata_cache_mb * 1024 * 1024));
        let file_cache = Arc::new(FileCache::new(
            file_cache_mb * 1024 * 1024,
            file_cache_max_file_mb * 1024 * 1024,
        ));

        let reader_factory = Arc::new(ReaderFactory::new(
            lake.blob_storage.inner(),
            lake.db_pool.clone(),
            metadata_cache.clone(),
            file_cache.clone(),
        ));
        Self {
            lake,
            metadata_cache,
            file_cache,
            runtime,
            reader_factory,
        }
    }

    /// Creates a new lakehouse context with custom metadata and file caches.
    pub fn with_caches(
        lake: Arc<DataLakeConnection>,
        runtime: Arc<RuntimeEnv>,
        metadata_cache: Arc<MetadataCache>,
        file_cache: Arc<FileCache>,
    ) -> Self {
        let reader_factory = Arc::new(ReaderFactory::new(
            lake.blob_storage.inner(),
            lake.db_pool.clone(),
            metadata_cache.clone(),
            file_cache.clone(),
        ));
        Self {
            lake,
            metadata_cache,
            file_cache,
            runtime,
            reader_factory,
        }
    }

    /// Returns the data lake connection.
    pub fn lake(&self) -> &Arc<DataLakeConnection> {
        &self.lake
    }

    /// Returns the metadata cache.
    pub fn metadata_cache(&self) -> &Arc<MetadataCache> {
        &self.metadata_cache
    }

    /// Returns the file cache.
    pub fn file_cache(&self) -> &Arc<FileCache> {
        &self.file_cache
    }

    /// Returns the DataFusion runtime environment.
    pub fn runtime(&self) -> &Arc<RuntimeEnv> {
        &self.runtime
    }

    /// Returns the shared `ReaderFactory`.
    pub fn reader_factory(&self) -> &Arc<ReaderFactory> {
        &self.reader_factory
    }
}

impl std::fmt::Debug for LakehouseContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakehouseContext")
            .field("metadata_cache", &self.metadata_cache)
            .field("file_cache", &self.file_cache)
            .finish()
    }
}
