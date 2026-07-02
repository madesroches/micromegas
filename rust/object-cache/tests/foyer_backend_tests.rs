#![cfg(feature = "foyer")]

use bytes::Bytes;

use micromegas_object_cache::backend::RangeCacheBackend;
use micromegas_object_cache::foyer_backend::FoyerBackend;

// Deliberately does not close/reopen the backend to force a disk read: foyer's
// default hash builder (ahash `RandomState::default()`) picks a fresh random
// seed per `HybridCacheBuilder` call, so a second `FoyerBackend::new` in the
// same process hashes the same key differently and can never find entries the
// first instance wrote to disk. Forcing eviction from the RAM tier within a
// single instance exercises the same disk serialize/deserialize path without
// that pitfall.
//
// TODO: if this test ever needs to wait on background disk activity, prefer a
// deterministic wait (like `FoyerBackend::close`, which awaits the flusher)
// over `tokio::time::sleep` with a hardcoded duration -- fixed sleeps make
// tests flaky under load and slow under normal conditions.
#[tokio::test]
async fn round_trip_through_disk_tier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let data = Bytes::from(vec![9u8; 4096]);

    // ram_bytes is a max entry count here (no weighter is installed, so the
    // default per-entry weight is 1): capacity 1 means each put below evicts
    // the previous entry from the RAM tier, which is what enqueues it for the
    // disk tier (the disk write is triggered by memory eviction, not by
    // insert itself).
    let backend = FoyerBackend::new(dir_path, 1, 16 * 1024 * 1024)
        .await
        .expect("create backend");
    backend.put("key".to_string(), data.clone()).await;
    backend
        .put("evict-1".to_string(), Bytes::from(vec![1u8; 16]))
        .await;
    backend
        .put("evict-2".to_string(), Bytes::from(vec![2u8; 16]))
        .await;
    // close() awaits the flusher, so the read below is guaranteed to see
    // "key" on disk rather than racing the background write.
    backend.close().await.expect("close backend");

    let got = backend.get("key").await.expect("get from disk tier");
    assert_eq!(got, data);
}
