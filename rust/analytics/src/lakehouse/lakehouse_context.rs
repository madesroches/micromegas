use super::audience_guard::{
    AudienceIndex, DEFAULT_AUDIENCE_CACHE_ENTRIES, DEFAULT_AUDIENCE_CACHE_TTL,
};
use super::metadata_cache::MetadataCache;
use super::migration::migrate_lakehouse;
use super::query_deny_list::QueryDenyList;
use super::reader_factory::ReaderFactory;
use super::runtime::make_runtime_env;
use anyhow::Context;
use anyhow::Result;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_config::DataLakeConfig;
use micromegas_ingestion::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// Default metadata cache size in MB
const DEFAULT_METADATA_CACHE_SIZE_MB: u64 = 50;

/// Bundles all runtime resources needed for lakehouse query execution.
///
/// This struct holds the data lake connection, metadata cache, and DataFusion runtime,
/// providing a single context object that can be passed through the query path. Parquet
/// byte-range caching is handled by the in-process L1 cache wrapped around the reader
/// factory's object store (see `object_cache::l1_wrap`), not by this struct.
#[derive(Clone)]
pub struct LakehouseContext {
    lake: Arc<DataLakeConnection>,
    metadata_cache: Arc<MetadataCache>,
    runtime: Arc<RuntimeEnv>,
    reader_factory: Arc<ReaderFactory>,
    /// Query Enforcement Prong B (#1371, AbAC Stage 3) -- resolves *any telemetry id -> its
    /// owning process's audience* from Postgres, size- and TTL-bounded. Fixed shape, no
    /// operational knob (see [`DEFAULT_AUDIENCE_CACHE_ENTRIES`]'s doc comment for why).
    audience_index: Arc<AudienceIndex>,
    /// Admin-managed query deny list (`tasks/query_deny_list_plan.md`). The refresh task that
    /// keeps its snapshot warm is spawned only by the FlightSQL server builder -- every other
    /// holder of a `LakehouseContext` (maintenance daemon, tests) keeps an empty snapshot and
    /// `check` never denies anything.
    query_denials: Arc<QueryDenyList>,
}

impl LakehouseContext {
    /// Builds a lakehouse context from an already-connected `DataLakeConnection`.
    ///
    /// Runs `migrate_lakehouse` on the supplied connection (idempotent) and
    /// creates the DataFusion runtime.  The caller is responsible for running
    /// `migrate_db` (ingestion schema) before this if both migrations are needed
    /// — the monolith does this via `connect_to_remote_data_lake`.
    pub async fn from_connection(lake: Arc<DataLakeConnection>) -> Result<Arc<Self>> {
        migrate_lakehouse(lake.db_pool.clone())
            .await
            .with_context(|| "migrate_lakehouse")?;
        let runtime = Arc::new(make_runtime_env()?);
        Ok(Arc::new(Self::new(lake, runtime)))
    }

    /// Reads MICROMEGAS_SQL_CONNECTION_STRING and MICROMEGAS_OBJECT_STORE_URI,
    /// connects to the data lake, runs lakehouse migrations, and creates the
    /// runtime environment.
    pub async fn from_env() -> Result<Arc<Self>> {
        let cfg = DataLakeConfig::from_env()?;
        let data_lake = Arc::new(
            connect_to_data_lake(&cfg.sql_connection_string, &cfg.object_store_uri).await?,
        );
        migrate_lakehouse(data_lake.db_pool.clone())
            .await
            .with_context(|| "migrate_lakehouse")?;
        let runtime = Arc::new(make_runtime_env()?);
        Ok(Arc::new(Self::new(data_lake, runtime)))
    }

    /// Creates a new lakehouse context with a default-sized metadata cache.
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

        let metadata_cache = Arc::new(MetadataCache::new(metadata_cache_mb * 1024 * 1024));

        let reader_factory = Arc::new(ReaderFactory::new(
            micromegas_object_cache::l1_wrap(lake.blob_storage.inner(), "lakehouse"),
            metadata_cache.clone(),
        ));
        let audience_index = Arc::new(AudienceIndex::new(
            lake.db_pool.clone(),
            DEFAULT_AUDIENCE_CACHE_ENTRIES,
            DEFAULT_AUDIENCE_CACHE_TTL,
        ));
        let query_denials = Arc::new(QueryDenyList::new(lake.db_pool.clone()));
        Self {
            lake,
            metadata_cache,
            runtime,
            reader_factory,
            audience_index,
            query_denials,
        }
    }

    /// Creates a new lakehouse context with a custom metadata cache.
    pub fn with_caches(
        lake: Arc<DataLakeConnection>,
        runtime: Arc<RuntimeEnv>,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        let reader_factory = Arc::new(ReaderFactory::new(
            micromegas_object_cache::l1_wrap(lake.blob_storage.inner(), "lakehouse"),
            metadata_cache.clone(),
        ));
        let audience_index = Arc::new(AudienceIndex::new(
            lake.db_pool.clone(),
            DEFAULT_AUDIENCE_CACHE_ENTRIES,
            DEFAULT_AUDIENCE_CACHE_TTL,
        ));
        let query_denials = Arc::new(QueryDenyList::new(lake.db_pool.clone()));
        Self {
            lake,
            metadata_cache,
            runtime,
            reader_factory,
            audience_index,
            query_denials,
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

    /// Clones this context with `runtime` swapped, sharing the metadata cache and reader
    /// factory. Deliberately a struct-update clone rather than a call to `new()`/
    /// `with_caches()` — those rebuild the `MetadataCache` and `ReaderFactory`, which would
    /// throw away the shared metadata cache per query.
    pub fn with_runtime(&self, runtime: Arc<RuntimeEnv>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            ..self.clone()
        })
    }

    /// Returns the shared `ReaderFactory`.
    pub fn reader_factory(&self) -> &Arc<ReaderFactory> {
        &self.reader_factory
    }

    /// Returns the shared Prong B audience index (#1371, AbAC Stage 3).
    pub fn audience_index(&self) -> &Arc<AudienceIndex> {
        &self.audience_index
    }

    /// Returns the shared query deny list (`tasks/query_deny_list_plan.md`).
    pub fn query_denials(&self) -> &Arc<QueryDenyList> {
        &self.query_denials
    }
}

impl std::fmt::Debug for LakehouseContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakehouseContext")
            .field("metadata_cache", &self.metadata_cache)
            .finish()
    }
}
