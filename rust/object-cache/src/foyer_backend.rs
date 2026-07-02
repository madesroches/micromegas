use anyhow::{Result, ensure};
use async_trait::async_trait;
use bytes::Bytes;
use foyer::{CacheHint, DirectFsDeviceOptions, Engine, HybridCache, HybridCacheBuilder, LruConfig};
use micromegas_tracing::prelude::*;

use super::backend::{FillHint, RangeCacheBackend};

pub struct FoyerBackend {
    cache: HybridCache<String, Bytes>,
}

impl FoyerBackend {
    pub async fn new(dir: &str, ram_bytes: usize, disk_bytes: usize) -> Result<Self> {
        Self::new_with_shards(dir, ram_bytes, disk_bytes, 8).await
    }

    pub async fn new_with_shards(
        dir: &str,
        ram_bytes: usize,
        disk_bytes: usize,
        shards: usize,
    ) -> Result<Self> {
        ensure!(shards > 0, "shards must be > 0");
        let cache = HybridCacheBuilder::new()
            .memory(ram_bytes)
            .with_weighter(|_key: &String, value: &Bytes| value.len())
            .with_shards(shards)
            // Pin the RAM tier to LRU explicitly: only LRU maps
            // `CacheHint::Low` to a low-priority eviction hint in foyer 0.14.x
            // (Lfu/S3Fifo/Fifo silently discard it). This is the crate's
            // current default; pinning it defensively guards `FillHint`
            // against a future foyer default change.
            .with_eviction_config(LruConfig::default())
            .storage(Engine::Large)
            .with_device_options(DirectFsDeviceOptions::new(dir).with_capacity(disk_bytes))
            .build()
            .await?;
        Ok(Self { cache })
    }

    pub async fn close(&self) -> Result<()> {
        self.cache.close().await?;
        Ok(())
    }
}

impl From<FillHint> for CacheHint {
    fn from(hint: FillHint) -> Self {
        match hint {
            FillHint::Demand => CacheHint::Normal,
            FillHint::Prefetch => CacheHint::Low,
        }
    }
}

#[async_trait]
impl RangeCacheBackend for FoyerBackend {
    async fn get(&self, key: &str) -> Option<Bytes> {
        match self.cache.obtain(key.to_string()).await {
            Ok(Some(entry)) => Some(entry.value().clone()),
            Ok(None) => None,
            // A backend (disk/IO) error must not fail the read: treat it as a
            // miss so the caller falls back to origin, but surface it as a
            // metric + log so a degraded SSD volume is observable rather than
            // silently inflating origin traffic.
            Err(e) => {
                imetric!("range_cache_backend_error", "count", 1_u64);
                warn!("range_cache backend get error key={key}: {e}");
                None
            }
        }
    }

    async fn put(&self, key: String, value: Bytes, hint: FillHint) {
        self.cache.insert_with_hint(key, value, hint.into());
    }
}
