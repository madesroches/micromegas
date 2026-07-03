use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// One key to warm, whole-object or ranged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchItem {
    pub key: String,
    /// The object's file size, supplied by the caller. Both triggers already know
    /// it: `Partition.file_size` (persisted in PostgreSQL) for query/write warming,
    /// and `PartitionWriteResult.file_size` for the write path. Supplying it lets
    /// the server drive fills through `prefetch_blocks(key, file_size, indices)`
    /// with no origin HEAD (prefetch targets cold objects, so a server-side
    /// `size()` would force an avoidable HEAD).
    ///
    /// Contract: this must be the object's exact current size. The server
    /// trusts it without verification; an undersized value stores a truncated
    /// final block under the same block key demand reads use. `RangeCache`
    /// mitigates this with a hit-path length guard that detects and refetches a
    /// wrong-length block on the next correctly-sized read. An oversized value
    /// is safe — the origin GET past EOF fails and nothing is stored.
    pub size: u64,
    /// None or empty = warm the whole object `[0, size)`. Present = warm only these
    /// ranges (validated against `size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges: Option<Vec<[u64; 2]>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchRequest {
    pub keys: Vec<PrefetchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefetchResponse {
    /// Enqueued onto the prefetch queue.
    pub accepted: usize,
    /// Failed key/prefix/range validation, skipped.
    pub rejected: usize,
    /// Queue full, load-shed.
    pub dropped: usize,
}

/// Dyn-compatible capability seam so downstream consumers that hold
/// `Arc<dyn ObjectStore>` (not `CacheClientStore` directly) can still drive
/// prefetch without downcasting.
#[async_trait]
pub trait ObjectPrefetch: Send + Sync {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse>;
}
