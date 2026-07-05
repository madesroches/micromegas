use async_trait::async_trait;
use object_store::path::Path;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Wire format for `POST /prefetch`: `Content-Type: application/x-ndjson`, one
/// `PrefetchItem` JSON object per `\n`-terminated line. There is no wrapper
/// type — the body is a stream of lines, parsed and enqueued incrementally so
/// the server never buffers the whole batch.
///
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
///
/// `Debug` is a supertrait so `Arc<dyn ObjectPrefetch>` can be embedded in a
/// `#[derive(Debug)]` struct (e.g. `DataLakeConnection`).
#[async_trait]
pub trait ObjectPrefetch: Send + Sync + std::fmt::Debug {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse>;
}

/// Prepends a root prefix to each `PrefetchItem`'s key before delegating, so a
/// warm keyed by a lake-root-relative path (`views/…`) targets the same cache
/// key a demand read produces through `object_store::PrefixStore` (`root/views/…`).
/// This mirrors, for the prefetch path, what `PrefixStore` does for reads.
#[derive(Debug)]
pub struct PrefixPrefetch {
    inner: Arc<dyn ObjectPrefetch>,
    prefix: Path,
}

impl PrefixPrefetch {
    pub fn new(inner: Arc<dyn ObjectPrefetch>, prefix: Path) -> Self {
        Self { inner, prefix }
    }
}

#[async_trait]
impl ObjectPrefetch for PrefixPrefetch {
    async fn prefetch(&self, mut items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse> {
        for item in &mut items {
            // Compose exactly as PrefixStore does: chain the prefix's path parts
            // with the key's parts, so the resulting string equals the key
            // CacheClientStore.get() sees for a demand read of the same object.
            let full = Path::from_iter(
                self.prefix
                    .parts()
                    .chain(Path::from(item.key.as_str()).parts()),
            );
            item.key = full.as_ref().to_string();
        }
        self.inner.prefetch(items).await
    }
}
