use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{Error, Result};

use super::{ApiKey, ApiKeyValidator};

pub struct TtlCacheValidation<T> {
    inner: T,
    ttl: Duration,
    cache: RwLock<ttl_cache::TtlCache<ApiKey, bool>>,
}

impl<T> TtlCacheValidation<T> {
    pub fn new(inner: T, capacity: usize, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: RwLock::new(ttl_cache::TtlCache::new(capacity)),
        }
    }
}

#[async_trait]
impl<T: ApiKeyValidator + Send + Sync> ApiKeyValidator for TtlCacheValidation<T> {
    async fn validate_api_key(&self, api_key: ApiKey) -> Result<()> {
        let exists = self.cache.read().await.get(&api_key).copied();

        match exists {
            Some(true) => Ok(()),
            Some(false) => Err(Error::InvalidApiKey(api_key)),
            None => match self.inner.validate_api_key(api_key.clone()).await {
                Ok(()) => {
                    self.cache.write().await.insert(api_key, true, self.ttl);
                    Ok(())
                }
                Err(Error::InvalidApiKey(err_api_key)) => {
                    self.cache.write().await.insert(api_key, false, self.ttl);
                    Err(Error::InvalidApiKey(err_api_key))
                }
                Err(err) => Err(err),
            },
        }
    }
}
