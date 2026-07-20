use async_trait::async_trait;
use bytes::Bytes;
use foyer_memory::{Cache, CacheBuilder, LfuConfig};

use super::backend::{FillHint, RangeCacheBackend};

/// A byte-weighted, sharded, in-memory `RangeCacheBackend` bounded to a fixed
/// capacity, built on the standalone `foyer-memory` crate's `Cache` -- not the
/// umbrella `foyer` crate (which `FoyerBackend` uses via `HybridCache`, and
/// which transitively pulls in a disk-backed storage engine). `foyer-memory`
/// is the same in-memory engine `HybridCache` uses internally for its RAM
/// tier, so this reuses that eviction implementation without the disk-IO
/// dependencies, via `LfuConfig` rather than `FoyerBackend`'s `LruConfig`.
///
/// Used as the shared RAM budget backing the in-process L1 cache
/// (`l1_store.rs`).
pub struct BoundedMemoryBackend {
    cache: Cache<String, Bytes>,
}

impl BoundedMemoryBackend {
    /// `budget_bytes` bounds the total weighted (byte) size of cached
    /// entries; foyer handles eviction and sharded concurrency internally.
    pub fn new(budget_bytes: usize) -> Self {
        // The explicit `Cache<String, Bytes>` type here (matching the field's
        // type) pins `build`'s inferred `Properties` generic to `Cache`'s
        // default (`CacheProperties`); `build` alone has no default for it.
        let cache: Cache<String, Bytes> = CacheBuilder::new(budget_bytes)
            .with_weighter(|_key: &String, value: &Bytes| value.len())
            .with_eviction_config(LfuConfig::default())
            .build();
        Self { cache }
    }

    /// Current weighted (byte) usage, exposed for tests.
    pub fn usage(&self) -> usize {
        self.cache.usage()
    }
}

#[async_trait]
impl RangeCacheBackend for BoundedMemoryBackend {
    async fn get(&self, key: &str, _expected_len: u64) -> Option<Bytes> {
        self.cache.get(key).map(|entry| entry.value().clone())
    }

    async fn put(&self, key: String, value: Bytes, _hint: FillHint) {
        // No disk tier, so demand and prefetch fills are treated identically
        // (see `FillHint`'s docs and the L1 design notes): there is no
        // SSD-only admission path to route a prefetch fill through, and
        // `foyer_memory::Cache` exposes only a plain `insert`.
        //
        // Copy so the cached block does not retain its coalesced-GET parent
        // buffer -- see `FoyerBackend::put`'s identical copy for the full
        // rationale.
        let owned = Bytes::copy_from_slice(&value);
        self.cache.insert(key, owned);
    }
}
