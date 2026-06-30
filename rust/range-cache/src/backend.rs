use async_trait::async_trait;
use bytes::Bytes;

#[async_trait]
pub trait RangeCacheBackend: Send + Sync {
    async fn get(&self, key: &str) -> Option<Bytes>;
    async fn put(&self, key: String, value: Bytes);
}
