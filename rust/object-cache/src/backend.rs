use async_trait::async_trait;
use bytes::Bytes;

/// Hints the backend's fill priority: prefetch fills should not evict hot
/// demand data from a bounded cache tier.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FillHint {
    Demand,
    Prefetch,
}

#[async_trait]
pub trait RangeCacheBackend: Send + Sync {
    async fn get(&self, key: &str) -> Option<Bytes>;
    async fn put(&self, key: String, value: Bytes, hint: FillHint);
}
