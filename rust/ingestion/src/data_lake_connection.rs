use anyhow::{Context, Result};
use micromegas_object_cache::CacheClientStore;
use micromegas_object_cache::prefetch::{ObjectPrefetch, PrefetchItem, PrefixPrefetch};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// A connection to the data lake, including a database pool and a blob storage client.
#[derive(Debug, Clone)]
pub struct DataLakeConnection {
    pub db_pool: PgPool,
    pub blob_storage: Arc<BlobStorage>,
    /// `Some` when the object cache is configured for this connection; used to
    /// fire-and-forget warm freshly-written partitions (`warm_partition`).
    /// `None` when the cache is not configured.
    prefetch: Option<Arc<dyn ObjectPrefetch>>,
}

impl DataLakeConnection {
    pub fn new(db_pool: PgPool, blob_storage: Arc<BlobStorage>) -> Self {
        Self {
            db_pool,
            blob_storage,
            prefetch: None,
        }
    }

    /// Like `new`, but also wires the object cache's prefetch face for
    /// write-time warming (see `warm_partition`).
    pub fn new_with_prefetch(
        db_pool: PgPool,
        blob_storage: Arc<BlobStorage>,
        prefetch: Option<Arc<dyn ObjectPrefetch>>,
    ) -> Self {
        Self {
            db_pool,
            blob_storage,
            prefetch,
        }
    }

    /// Warm a freshly-written partition in the object cache. Fire-and-forget at
    /// prefetch priority: spawns a detached task and returns immediately, so the
    /// write/materialization path is never delayed or failed by a warm. No-op when
    /// the cache is not configured or the partition is empty. Returns the spawned
    /// task handle (or None) purely so tests can await completion deterministically;
    /// production callers ignore it.
    pub fn warm_partition(&self, file_path: &str, file_size: i64) -> Option<JoinHandle<()>> {
        let prefetch = self.prefetch.as_ref()?.clone();
        if file_size <= 0 {
            return None; // empty partitions have no object to warm
        }
        let key = file_path.to_string(); // owned copy: the spawned future must be 'static
        let item = PrefetchItem {
            key: key.clone(),
            size: file_size as u64,
            ranges: None,
        };
        imetric!("write_time_partition_warm_requested", "count", 1_u64);
        Some(spawn_with_context(async move {
            match prefetch.prefetch(vec![item]).await {
                Ok(resp) => debug!(
                    "write-time warm enqueued accepted={} rejected={} dropped={}",
                    resp.accepted, resp.rejected, resp.dropped
                ),
                // CacheClientStore::prefetch already bumps range_cache_client_prefetch_error;
                // keep this at debug — a failed warm just means the first read is a cold miss.
                Err(e) => debug!("write-time warm failed for {key}: {e}"),
            }
        }))
    }
}

/// Wrap `direct` with the object cache when configured, returning the store
/// layer and — when enabled — the same client's `ObjectPrefetch` face for
/// write-time warming.
pub(crate) fn make_cache(
    direct: Arc<dyn ObjectStore>,
) -> (Arc<dyn ObjectStore>, Option<Arc<dyn ObjectPrefetch>>) {
    let cache_url = std::env::var("MICROMEGAS_OBJECT_CACHE_URL").ok();
    let api_key = std::env::var("MICROMEGAS_OBJECT_CACHE_API_KEY").ok();
    match cache_url {
        Some(url) if api_key.is_some() => {
            let client = Arc::new(CacheClientStore::new(url, api_key, direct));
            (
                client.clone() as Arc<dyn ObjectStore>,
                Some(client as Arc<dyn ObjectPrefetch>),
            )
        }
        Some(url) => {
            // URL without key: disabled, warn (preserve current behavior)
            warn!(
                "MICROMEGAS_OBJECT_CACHE_URL is set ({url}) but MICROMEGAS_OBJECT_CACHE_API_KEY is missing: the object cache is disabled and requests will go directly to the store"
            );
            (direct, None)
        }
        None => (direct, None),
    }
}

/// Connects to the data lake.
pub async fn connect_to_data_lake(
    db_uri: &str,
    object_store_url: &str,
) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let (raw_store, root) = BlobStorage::parse_url_opts(object_store_url)
        .with_context(|| "connecting to blob storage")?;
    let (layered, prefetch_client) = make_cache(raw_store);
    let blob_storage = Arc::new(BlobStorage::new(layered, root.clone()));
    let prefetch =
        prefetch_client.map(|p| Arc::new(PrefixPrefetch::new(p, root)) as Arc<dyn ObjectPrefetch>);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    Ok(DataLakeConnection::new_with_prefetch(
        pool,
        blob_storage,
        prefetch,
    ))
}
