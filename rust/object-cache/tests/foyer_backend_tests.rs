#![cfg(feature = "foyer")]

use bytes::Bytes;

use micromegas_object_cache::backend::{FillHint, RangeCacheBackend};
use micromegas_object_cache::foyer_backend::{FoyerBackend, WriteTuning};

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

    // ram_bytes is a byte budget (a value.len() weighter is installed) and the
    // memory tier uses a single shard, so the budget is one bucket: capacity 4096
    // exactly holds the first 4096-byte payload, so the subsequent puts push the
    // RAM tier over budget and evict "key", which enqueues it for the disk tier
    // (the disk write is triggered by memory eviction, not by insert itself).
    let backend =
        FoyerBackend::new_with_shards(dir_path, 4096, 16 * 1024 * 1024, 1, WriteTuning::default())
            .await
            .expect("create backend");
    backend
        .put("key".to_string(), data.clone(), FillHint::Demand)
        .await;
    backend
        .put(
            "evict-1".to_string(),
            Bytes::from(vec![1u8; 16]),
            FillHint::Demand,
        )
        .await;
    backend
        .put(
            "evict-2".to_string(),
            Bytes::from(vec![2u8; 16]),
            FillHint::Demand,
        )
        .await;
    // close() awaits the flusher, so the read below is guaranteed to see
    // "key" on disk rather than racing the background write.
    backend.close().await.expect("close backend");

    let got = backend.get("key").await.expect("get from disk tier");
    assert_eq!(got, data);
}

// A prefetch fill must be admitted to the SSD tier deterministically (via
// `.force()`, bypassing the disk admission picker) and must not retain RAM-tier
// residency: only an ephemeral record is held during the write, dropped
// immediately once the disk enqueue completes.
#[tokio::test]
async fn prefetch_fill_lands_on_disk_not_ram() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let data = Bytes::from(vec![9u8; 4096]);

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        16 * 1024 * 1024,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
    )
    .await
    .expect("create backend");

    let ram_before = backend.ram_usage();
    backend
        .put("prefetched".to_string(), data.clone(), FillHint::Prefetch)
        .await;
    let ram_after = backend.ram_usage();
    assert_eq!(
        ram_after, ram_before,
        "a prefetch fill must not grow RAM-tier usage"
    );

    // Flush the SSD tier so the async disk write is durable before reading.
    backend.close().await.expect("close backend");
    let got = backend.get("prefetched").await.expect("get from disk tier");
    assert_eq!(got, data);
}

// A non-default `WriteTuning` (more flushers, a bigger buffer pool and
// submit-queue threshold) must not change the backend's observable behavior:
// the disk round-trip still works, and `disk_stats()` reports the write that
// just landed.
#[tokio::test]
async fn round_trip_with_custom_write_tuning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let data = Bytes::from(vec![9u8; 4096]);

    let tuning = WriteTuning {
        flushers: 2,
        buffer_pool_bytes: 32 * 1024 * 1024,
        submit_queue_threshold_bytes: 64 * 1024 * 1024,
    };
    // The disk device must hold enough blocks for the flusher count: foyer's
    // block engine derives `blocks = capacity / block_size` (default block
    // size 16 MiB), and `flushers` flushers contending for too few blocks
    // deadlock on flush. A 16 MiB device is a single block -- fine for the
    // 1-flusher tests above, but `flushers: 2` needs headroom, so give this
    // one a 256 MiB device (16 blocks). Production disks are hundreds of GB,
    // so this constraint never binds there.
    let disk_bytes = 256 * 1024 * 1024;
    let backend = FoyerBackend::new_with_shards(dir_path, 4096, disk_bytes, 1, tuning)
        .await
        .expect("create backend with custom tuning");

    backend
        .put("key".to_string(), data.clone(), FillHint::Prefetch)
        .await;
    // close() awaits the flusher, so the disk write below is guaranteed
    // durable and `disk_stats()` reflects it deterministically.
    backend.close().await.expect("close backend");

    let got = backend.get("key").await.expect("get from disk tier");
    assert_eq!(got, data);

    let stats = backend
        .disk_stats()
        .expect("foyer backend reports disk stats");
    assert!(
        stats.write_bytes > 0,
        "a completed disk write must be reflected in disk_stats(): {stats:?}"
    );
}
