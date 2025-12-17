use super::metadata_cache::MetadataCache;
use super::reader_factory::ReaderFactory;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

/// Default metadata cache size in MB
const DEFAULT_CACHE_SIZE_MB: u64 = 50;

/// Bundles all runtime resources needed for lakehouse query execution.
///
/// This struct holds the data lake connection, metadata cache, and DataFusion runtime,
/// providing a single context object that can be passed through the query path.
#[derive(Clone)]
pub struct LakehouseContext {
    pub lake: Arc<DataLakeConnection>,
    pub metadata_cache: Arc<MetadataCache>,
    pub runtime: Arc<RuntimeEnv>,
}

impl LakehouseContext {
    /// Creates a new lakehouse context with a default-sized metadata cache.
    pub fn new(lake: Arc<DataLakeConnection>, runtime: Arc<RuntimeEnv>) -> Self {
        let cache_mb = std::env::var("MICROMEGAS_METADATA_CACHE_MB")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_CACHE_SIZE_MB);
        let metadata_cache = Arc::new(MetadataCache::new(cache_mb * 1024 * 1024));
        Self {
            lake,
            metadata_cache,
            runtime,
        }
    }

    /// Creates a new lakehouse context with a custom metadata cache.
    pub fn with_cache(
        lake: Arc<DataLakeConnection>,
        runtime: Arc<RuntimeEnv>,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        Self {
            lake,
            metadata_cache,
            runtime,
        }
    }

    /// Creates a `ReaderFactory` using the shared metadata cache.
    pub fn make_reader_factory(&self) -> Arc<ReaderFactory> {
        Arc::new(ReaderFactory::new(
            self.lake.blob_storage.inner(),
            self.lake.db_pool.clone(),
            self.metadata_cache.clone(),
        ))
    }
}

impl std::fmt::Debug for LakehouseContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakehouseContext")
            .field("metadata_cache", &self.metadata_cache)
            .finish()
    }
}
