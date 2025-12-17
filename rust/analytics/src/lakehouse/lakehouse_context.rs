use super::metadata_cache::MetadataCache;
use super::reader_factory::ReaderFactory;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Default metadata cache size in MB
const DEFAULT_CACHE_SIZE_MB: u64 = 50;

/// Bundles all runtime resources needed for lakehouse query execution.
///
/// This struct holds the data lake connection, metadata cache, and DataFusion runtime,
/// providing a single context object that can be passed through the query path.
#[derive(Clone)]
pub struct LakehouseContext {
    lake: Arc<DataLakeConnection>,
    metadata_cache: Arc<MetadataCache>,
    runtime: Arc<RuntimeEnv>,
    reader_factory: Arc<ReaderFactory>,
}

impl LakehouseContext {
    /// Creates a new lakehouse context with a default-sized metadata cache.
    pub fn new(lake: Arc<DataLakeConnection>, runtime: Arc<RuntimeEnv>) -> Self {
        let cache_mb = match std::env::var("MICROMEGAS_METADATA_CACHE_MB") {
            Ok(s) => s.parse::<u64>().unwrap_or_else(|_| {
                warn!(
                    "Invalid MICROMEGAS_METADATA_CACHE_MB value '{s}', using default {DEFAULT_CACHE_SIZE_MB} MB"
                );
                DEFAULT_CACHE_SIZE_MB
            }),
            Err(_) => DEFAULT_CACHE_SIZE_MB,
        };
        let metadata_cache = Arc::new(MetadataCache::new(cache_mb * 1024 * 1024));
        let reader_factory = Arc::new(ReaderFactory::new(
            lake.blob_storage.inner(),
            lake.db_pool.clone(),
            metadata_cache.clone(),
        ));
        Self {
            lake,
            metadata_cache,
            runtime,
            reader_factory,
        }
    }

    /// Creates a new lakehouse context with a custom metadata cache.
    pub fn with_cache(
        lake: Arc<DataLakeConnection>,
        runtime: Arc<RuntimeEnv>,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        let reader_factory = Arc::new(ReaderFactory::new(
            lake.blob_storage.inner(),
            lake.db_pool.clone(),
            metadata_cache.clone(),
        ));
        Self {
            lake,
            metadata_cache,
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
            .finish()
    }
}
