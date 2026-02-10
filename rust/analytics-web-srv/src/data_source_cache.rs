use crate::app_db::DataSourceConfig;
use anyhow::Result;
use moka::future::Cache;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

/// In-memory cache mapping data source names to their configs.
///
/// - Lazy loading: entries are loaded from PG on first access
/// - TTL expiry: entries expire after a configurable duration so updates from
///   other processes are picked up
/// - CRUD on this process: invalidate the cache entry so the next resolve
///   fetches fresh from PG
#[derive(Clone)]
pub struct DataSourceCache {
    cache: Cache<String, Arc<DataSourceConfig>>,
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

        let result: Result<Arc<DataSourceConfig>, Arc<anyhow::Error>> = self
            .cache
            .try_get_with(name_owned.clone(), async {
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
                        Ok(Arc::new(config))
                    }
                    None => Err(anyhow::anyhow!("not found")),
                }
            })
            .await;

        match result {
            Ok(config) => Ok(Some((*config).clone())),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("{msg}"))
                }
            }
        }
    }

    /// Invalidate a cache entry after create/update/delete.
    pub async fn invalidate(&self, name: &str) {
        self.cache.invalidate(name).await;
    }
}
