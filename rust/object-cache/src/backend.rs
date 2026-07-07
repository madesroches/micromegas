use async_trait::async_trait;
use bytes::Bytes;

/// Hints the backend's fill priority: prefetch fills should not evict hot
/// demand data from a bounded cache tier.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FillHint {
    Demand,
    Prefetch,
}

/// Point-in-time disk write-path counters (cumulative since process start).
/// foyer-independent so the trait stays buildable without the `foyer`
/// feature; `None` for backends with no disk tier (e.g. in-memory).
#[derive(Clone, Copy, Debug, Default)]
pub struct BackendDiskStats {
    pub write_bytes: u64,
    pub read_bytes: u64,
    pub write_ios: u64,
    pub read_ios: u64,
}

#[async_trait]
pub trait RangeCacheBackend: Send + Sync {
    async fn get(&self, key: &str) -> Option<Bytes>;
    async fn put(&self, key: String, value: Bytes, hint: FillHint);

    /// Disk write-path counters, for the saturation monitor's per-second
    /// gauges. Defaulted to `None` so backends without a disk tier (e.g.
    /// `MemoryBackend`) need no override.
    fn disk_stats(&self) -> Option<BackendDiskStats> {
        None
    }
}
