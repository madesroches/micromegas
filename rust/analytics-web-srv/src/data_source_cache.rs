use crate::app_db::DataSourceConfig;
use anyhow::Result;
use moka::future::Cache;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

/// Cached value: `Some` for an existing data source, `None` for a confirmed miss.
type CachedEntry = Option<Arc<DataSourceConfig>>;

/// In-memory cache mapping data source names to their configs.
///
/// - Lazy loading: entries are loaded from PG on first access
/// - TTL expiry: entries expire after a configurable duration so updates from
///   other processes are picked up
/// - CRUD on this process: invalidate the cache entry so the next resolve
///   fetches fresh from PG
#[derive(Clone)]
pub struct DataSourceCache {
    cache: Cache<String, CachedEntry>,
    pool: PgPool,
}

impl DataSourceCache {
    pub fn new(pool: PgPool, ttl: Duration) -> Self {
        let cache = Cache::builder()
            .time_to_live(ttl)
            .max_capacity(1000)
            .build();
        Self { cache, pool }
    }

    /// Resolve a data source name to its config.
    /// Returns `None` if the data source does not exist.
    pub async fn resolve(&self, name: &str) -> Result<Option<DataSourceConfig>> {
        let pool = self.pool.clone();
        let name_owned = name.to_string();

        let result = self
            .cache
            .try_get_with::<_, anyhow::Error>(name_owned.clone(), async {
                let row = sqlx::query_scalar::<_, serde_json::Value>(
                    "SELECT config FROM data_sources WHERE name = $1",
                )
                .bind(&name_owned)
                .fetch_optional(&pool)
                .await
                .map_err(|e| anyhow::anyhow!("database error: {e}"))?;

                match row {
                    Some(config_json) => {
                        let config: DataSourceConfig = serde_json::from_value(config_json)
                            .map_err(|e| anyhow::anyhow!("invalid config: {e}"))?;
                        Ok(Some(Arc::new(config)))
                    }
                    None => Ok(None),
                }
            })
            .await;

        match result {
            Ok(Some(config)) => Ok(Some((*config).clone())),
            Ok(None) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        }
    }

    /// Invalidate a cache entry after create/update/delete.
    pub async fn invalidate(&self, name: &str) {
        self.cache.invalidate(name).await;
    }
}
