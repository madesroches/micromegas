use anyhow::{Result, ensure};
use async_trait::async_trait;
use bytes::Bytes;
use foyer::{
    BlockEngineConfig, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder, LruConfig,
};
use micromegas_tracing::prelude::*;

use super::backend::{BackendDiskStats, FillHint, RangeCacheBackend};

/// foyer disk-engine write-path tuning. Defaults reproduce foyer's own
/// `BlockEngineConfig` defaults (`flushers=1`, `buffer_pool_size=16 MiB`),
/// with the submit-queue threshold pinned to 2x the buffer pool -- which
/// foyer's doc comment describes as its intended default but which 0.22 no
/// longer applies automatically (its actual default is 1x, see
/// `BlockEngineConfig::new`) -- so existing callers/tests are unaffected
/// unless they opt into a different tuning.
#[derive(Clone, Copy, Debug)]
pub struct WriteTuning {
    /// `BlockEngineConfig::with_flushers`.
    pub flushers: usize,
    /// `BlockEngineConfig::with_buffer_pool_size`, in bytes.
    pub buffer_pool_bytes: usize,
    /// `BlockEngineConfig::with_submit_queue_size_threshold`, in bytes.
    pub submit_queue_threshold_bytes: usize,
}

impl Default for WriteTuning {
    fn default() -> Self {
        let buffer = 16 * 1024 * 1024;
        Self {
            flushers: 1,
            buffer_pool_bytes: buffer,
            submit_queue_threshold_bytes: buffer * 2,
        }
    }
}

pub struct FoyerBackend {
    cache: HybridCache<String, Bytes>,
}

impl FoyerBackend {
    pub async fn new(dir: &str, ram_bytes: usize, disk_bytes: usize) -> Result<Self> {
        Self::new_with_shards(dir, ram_bytes, disk_bytes, 8, WriteTuning::default()).await
    }

    pub async fn new_with_shards(
        dir: &str,
        ram_bytes: usize,
        disk_bytes: usize,
        shards: usize,
        tuning: WriteTuning,
    ) -> Result<Self> {
        ensure!(shards > 0, "shards must be > 0");

        // Direct I/O (bypassing the page cache) matches the old `DirectFs`
        // engine's behavior; the flag only exists on Linux.
        #[cfg(target_os = "linux")]
        let device = FsDeviceBuilder::new(dir)
            .with_capacity(disk_bytes)
            .with_direct(true)
            .build()?;
        #[cfg(not(target_os = "linux"))]
        let device = FsDeviceBuilder::new(dir)
            .with_capacity(disk_bytes)
            .build()?;

        let cache = HybridCacheBuilder::new()
            .memory(ram_bytes)
            .with_weighter(|_key: &String, value: &Bytes| value.len())
            .with_shards(shards)
            // Pin the RAM tier to LRU explicitly: LRU is the crate's current
            // default eviction policy; pinning it here guards against a
            // future foyer default change silently altering RAM-tier
            // eviction behavior for demand fills.
            .with_eviction_config(LruConfig::default())
            .storage()
            .with_engine_config(
                BlockEngineConfig::new(device)
                    .with_flushers(tuning.flushers)
                    .with_buffer_pool_size(tuning.buffer_pool_bytes)
                    .with_submit_queue_size_threshold(tuning.submit_queue_threshold_bytes),
            )
            .build()
            .await?;
        Ok(Self { cache })
    }

    pub async fn close(&self) -> Result<()> {
        self.cache.close().await?;
        Ok(())
    }

    /// Current RAM-tier byte usage. Exposed so integration tests (which
    /// compile as a separate crate and cannot reach the private `cache`
    /// field) can assert prefetch fills do not grow RAM-tier residency.
    pub fn ram_usage(&self) -> usize {
        self.cache.memory().usage()
    }
}

#[async_trait]
impl RangeCacheBackend for FoyerBackend {
    async fn get(&self, key: &str) -> Option<Bytes> {
        match self.cache.get(key).await {
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
        match hint {
            // SSD-only admission: `.force()` bypasses the disk admission
            // picker so the block is always admitted deterministically (no
            // silent decline). The write holds only an ephemeral RAM record
            // that is dropped immediately (no eviction-structure residency),
            // so a prefetch fill never retains RAM residency.
            FillHint::Prefetch => {
                let entry = self.cache.storage_writer(key).force().insert(value);
                if entry.is_none() {
                    // Should not occur under `.force()`, which always admits.
                    imetric!(
                        "range_cache_prefetch_admission_unexpected_none",
                        "count",
                        1_u64
                    );
                    warn!("prefetch storage_writer().force().insert() unexpectedly returned None");
                }
            }
            FillHint::Demand => {
                self.cache.insert(key, value);
            }
        }
    }

    fn disk_stats(&self) -> Option<BackendDiskStats> {
        let stats = self.cache.statistics();
        Some(BackendDiskStats {
            write_bytes: stats.disk_write_bytes() as u64,
            read_bytes: stats.disk_read_bytes() as u64,
            write_ios: stats.disk_write_ios() as u64,
            read_ios: stats.disk_read_ios() as u64,
        })
    }
}
