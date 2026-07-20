#![cfg(feature = "foyer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;

use micromegas_object_cache::backend::{FillHint, RangeCacheBackend};
use micromegas_object_cache::foyer_backend::{
    DISK_FORMAT_MARKER, DISK_FORMAT_VERSION, FoyerBackend, WriteTuning,
};
use micromegas_tracing::event::in_memory_sink::InMemorySink;
use micromegas_tracing::metrics::MetricsMsgQueueAny;
use micromegas_tracing::property_set::{PropertySet, property_get};
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;

/// `(value, properties)` for every firing of the tagged float metric `name`
/// since the guard was created. Requires `dispatch::flush_metrics_buffer`
/// first. Duplicated from `object-cache-srv/tests/saturation_tests.rs`'s
/// `float_metric_values`/`integer_metric_values`: that binary's helpers are
/// private free functions in a different crate's integration-test binary,
/// unreachable from here, so there's no import path to share them without
/// promoting them to a library crate.
fn tagged_float_metric_values(sink: &InMemorySink, name: &str) -> Vec<(f64, &'static PropertySet)> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedFloatMetricEvent(e) = evt
                && e.desc.name == name
            {
                out.push((e.value, e.properties));
            }
        }
    }
    out
}

/// `(value, properties)` for every firing of the tagged integer metric
/// `name` since the guard was created.
fn tagged_integer_metric_values(
    sink: &InMemorySink,
    name: &str,
) -> Vec<(u64, &'static PropertySet)> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::TaggedIntegerMetricEvent(e) = evt
                && e.desc.name == name
            {
                out.push((e.value, e.properties));
            }
        }
    }
    out
}

// foyer 0.22.3's default hash builder is `BuildHasherDefault<XxHash64>`
// (`DefaultHasher`), which foyer documents as guaranteeing the same key hashes
// identically across different runs/instances -- unlike foyer 0.14's
// per-instance random ahash seed, which this test used to work around by
// staying within a single `FoyerBackend` instance. Forcing eviction from the
// RAM tier still exercises the disk serialize/deserialize path directly,
// without needing a second instance or a process restart.
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
    let backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
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
        Arc::from(Vec::new()),
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

// A demand-admitted block must be detached (copied) from its coalesced-GET
// parent buffer, or the RAM tier's eviction structure keeps the whole parent
// allocation alive even though the weigher only charges the slice length --
// see the demand-fill-copy plan (#1276). `ram_bytes` is generous (well above
// the 4096-byte block) so the `get` below deterministically hits the memory
// tier rather than the disk tier, which would deserialize into a fresh buffer
// regardless of the fix and make the assertion pass vacuously.
#[tokio::test]
async fn demand_fill_detaches_from_parent_buffer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        1024 * 1024,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend");

    let parent = Bytes::from(vec![7u8; 8192]);
    let block = parent.slice(0..4096);
    let block_ptr = block.as_ptr();
    backend
        .put("k".to_string(), block.clone(), FillHint::Demand)
        .await;

    let got = backend.get("k").await.expect("hit");
    assert_eq!(got, vec![7u8; 4096]);
    assert_ne!(
        got.as_ptr(),
        block_ptr,
        "demand admission must copy, detaching the cached block from its parent GET buffer"
    );
}

struct DropFlag {
    data: Vec<u8>,
    dropped: Arc<AtomicBool>,
}

impl AsRef<[u8]> for DropFlag {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

// A prefetch fill must be detached (copied) from its coalesced-GET parent
// buffer, or the async disk-write pipeline (submit queue, io buffer encode,
// pending piece_refs) keeps the whole parent allocation alive for as long as
// the entry is in flight -- see #1317.
#[tokio::test]
async fn prefetch_fill_detaches_from_parent_buffer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        16 * 1024 * 1024,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend");

    let dropped = Arc::new(AtomicBool::new(false));
    let parent = Bytes::from_owner(DropFlag {
        data: vec![7u8; 8192],
        dropped: dropped.clone(),
    });
    let block = parent.slice(0..4096);
    drop(parent);
    assert!(
        !dropped.load(Ordering::SeqCst),
        "sanity: the slice must still keep the owner alive"
    );

    backend
        .put("k".to_string(), block.clone(), FillHint::Prefetch)
        .await;
    drop(block);

    assert!(
        dropped.load(Ordering::SeqCst),
        "prefetch admission must copy, detaching the cached block from its \
         parent GET buffer instead of retaining a slice into it across the \
         async disk-write pipeline"
    );

    backend.close().await.expect("close backend");
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
    let backend =
        FoyerBackend::new_with_shards(dir_path, 4096, disk_bytes, 1, tuning, Arc::from(Vec::new()))
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

// A capacity-driven RAM eviction (forced the same way as
// `round_trip_through_disk_tier`) must fire
// `object_cache_ram_tier_eviction_count{reason=evict, prefix=blobs}` and
// `object_cache_ram_tier_eviction_age_ms{prefix=blobs}` with a plausible
// (>= 0) age. `#[serial]`: `init_in_memory_tracing` touches global dispatch
// state shared by every test in this file that uses it.
#[tokio::test]
#[serial]
async fn ram_eviction_emits_count_and_age_metrics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let prefix_labels: Arc<[&'static str]> = Arc::from(vec!["blobs"]);

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        prefix_labels,
    )
    .await
    .expect("create backend");

    let guard = init_in_memory_tracing();

    // Same capacity-pressure pattern as `round_trip_through_disk_tier`: the
    // first 4096-byte put exactly fills the budget, so the following puts
    // evict it.
    backend
        .put(
            "blobs/key".to_string(),
            Bytes::from(vec![9u8; 4096]),
            FillHint::Demand,
        )
        .await;
    backend
        .put(
            "blobs/evict-1".to_string(),
            Bytes::from(vec![1u8; 16]),
            FillHint::Demand,
        )
        .await;
    backend
        .put(
            "blobs/evict-2".to_string(),
            Bytes::from(vec![2u8; 16]),
            FillHint::Demand,
        )
        .await;
    backend.close().await.expect("close backend");

    micromegas_tracing::dispatch::flush_metrics_buffer();

    let counts = tagged_integer_metric_values(&guard.sink, "object_cache_ram_tier_eviction_count");
    assert!(
        counts.iter().any(|(_, props)| {
            property_get(props.get_properties(), "prefix") == Some("blobs")
                && property_get(props.get_properties(), "reason") == Some("evict")
        }),
        "expected an evict-reason RAM eviction count for prefix=blobs, got {counts:?}"
    );

    let ages = tagged_float_metric_values(&guard.sink, "object_cache_ram_tier_eviction_age_ms");
    let blobs_ages: Vec<f64> = ages
        .iter()
        .filter(|(_, props)| property_get(props.get_properties(), "prefix") == Some("blobs"))
        .map(|(v, _)| *v)
        .collect();
    assert!(
        !blobs_ages.is_empty(),
        "expected a RAM eviction age sample for prefix=blobs"
    );
    assert!(
        blobs_ages.iter().all(|age| *age >= 0.0),
        "RAM eviction age must not be negative: {blobs_ages:?}"
    );
}

// A disk-tier hit (`Source::Disk`) must fire exactly one
// `object_cache_disk_tier_read_age_ms{prefix=blobs}` sample, verifying both
// the disk read-age instrumentation and (indirectly) that `CachedBlock`'s
// `Code` round-trip preserves `disk_write_ms` through the encode-to-disk /
// decode-on-read path -- if it didn't, `disk_write_ms` would decode as
// `DISK_WRITE_NONE` and no metric would fire at all.
#[tokio::test]
#[serial]
async fn disk_read_age_metric_fires_on_disk_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let data = Bytes::from(vec![9u8; 4096]);
    let prefix_labels: Arc<[&'static str]> = Arc::from(vec!["blobs"]);

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        prefix_labels,
    )
    .await
    .expect("create backend");

    backend
        .put("blobs/key".to_string(), data.clone(), FillHint::Demand)
        .await;
    backend
        .put(
            "blobs/evict-1".to_string(),
            Bytes::from(vec![1u8; 16]),
            FillHint::Demand,
        )
        .await;
    backend
        .put(
            "blobs/evict-2".to_string(),
            Bytes::from(vec![2u8; 16]),
            FillHint::Demand,
        )
        .await;
    // close() awaits the flusher, so the read below is guaranteed to see
    // "blobs/key" on disk rather than racing the background write.
    backend.close().await.expect("close backend");

    let guard = init_in_memory_tracing();

    let got = backend.get("blobs/key").await.expect("get from disk tier");
    assert_eq!(got, data);

    micromegas_tracing::dispatch::flush_metrics_buffer();
    let ages = tagged_float_metric_values(&guard.sink, "object_cache_disk_tier_read_age_ms");
    let blobs_ages: Vec<f64> = ages
        .iter()
        .filter(|(_, props)| property_get(props.get_properties(), "prefix") == Some("blobs"))
        .map(|(v, _)| *v)
        .collect();
    assert_eq!(
        blobs_ages.len(),
        1,
        "expected exactly one disk read-age sample for prefix=blobs, got {ages:?}"
    );
    assert!(
        blobs_ages[0] >= 0.0,
        "disk read-age must not be negative: {}",
        blobs_ages[0]
    );
}

// -- Disk format-version guard (#1287) ---------------------------------------

/// Force a RAM->disk eviction the same way `round_trip_through_disk_tier`
/// does, then `close()` so the write is durable. Leaves at least one
/// `foyer-storage-direct-fs-*` region file on disk for the caller to inspect.
async fn write_one_disk_entry(dir_path: &str) {
    let backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend");
    backend
        .put(
            "key".to_string(),
            Bytes::from(vec![9u8; 4096]),
            FillHint::Demand,
        )
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
    backend.close().await.expect("close backend");
}

fn foyer_region_files(dir_path: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir_path)
        .expect("read disk dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("foyer-storage-direct-fs-"))
        })
        .collect()
}

#[tokio::test]
async fn first_boot_writes_format_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let _backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend");

    let marker_contents =
        std::fs::read_to_string(dir.path().join(DISK_FORMAT_MARKER)).expect("marker written");
    assert_eq!(marker_contents.trim(), DISK_FORMAT_VERSION.to_string());
}

#[tokio::test]
async fn same_version_reuse_does_not_wipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    write_one_disk_entry(dir_path).await;
    let region_files_before = foyer_region_files(dir.path());
    assert!(
        !region_files_before.is_empty(),
        "expected at least one foyer region file after the first backend's disk write"
    );
    let marker_before =
        std::fs::read_to_string(dir.path().join(DISK_FORMAT_MARKER)).expect("marker present");

    // Same directory, same format version: must reuse warm, not wipe.
    let _backend2 = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create second backend");

    let region_files_after = foyer_region_files(dir.path());
    assert_eq!(
        region_files_before, region_files_after,
        "a matching format marker must not wipe the existing region files"
    );
    let marker_after =
        std::fs::read_to_string(dir.path().join(DISK_FORMAT_MARKER)).expect("marker still present");
    assert_eq!(marker_before, marker_after);
}

#[tokio::test]
async fn mismatched_version_wipes_and_reclaims() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    std::fs::write(dir.path().join(DISK_FORMAT_MARKER), "0").expect("write stale marker");
    let dummy_region = dir.path().join("foyer-storage-direct-fs-00000000");
    std::fs::write(&dummy_region, b"stale region data").expect("write dummy region file");

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend on mismatched-version dir");

    // foyer's device reuses the same region filename convention, so it may
    // recreate a file at this exact path -- check the stale *content* is
    // gone rather than mere file existence.
    let contents_after = std::fs::read(&dummy_region).unwrap_or_default();
    assert_ne!(
        contents_after, b"stale region data",
        "the stale-format dummy region file's content must be wiped on a version mismatch"
    );
    let marker_contents =
        std::fs::read_to_string(dir.path().join(DISK_FORMAT_MARKER)).expect("marker rewritten");
    assert_eq!(marker_contents.trim(), DISK_FORMAT_VERSION.to_string());

    // The backend must still be fully usable after the wipe.
    backend
        .put(
            "key".to_string(),
            Bytes::from(vec![1u8; 16]),
            FillHint::Demand,
        )
        .await;
    let got = backend.get("key").await.expect("get after wipe");
    assert_eq!(got, Bytes::from(vec![1u8; 16]));
}

#[tokio::test]
async fn missing_marker_wipes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    // No marker at all (pre-versioning store): only a stale region file.
    let dummy_region = dir.path().join("foyer-storage-direct-fs-00000000");
    std::fs::write(&dummy_region, b"stale region data").expect("write dummy region file");

    let _backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend on unmarked dir");

    // See the content-check note in `mismatched_version_wipes_and_reclaims`:
    // foyer may recreate a file at this exact path.
    let contents_after = std::fs::read(&dummy_region).unwrap_or_default();
    assert_ne!(
        contents_after, b"stale region data",
        "a missing marker (pre-versioning store) must be treated as a mismatch and wiped"
    );
    let marker_contents =
        std::fs::read_to_string(dir.path().join(DISK_FORMAT_MARKER)).expect("marker written");
    assert_eq!(marker_contents.trim(), DISK_FORMAT_VERSION.to_string());
}

#[tokio::test]
async fn wipe_preserves_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    std::fs::write(dir.path().join(DISK_FORMAT_MARKER), "0").expect("write stale marker");

    let _backend = FoyerBackend::new_with_shards(
        dir_path,
        4096,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend");

    assert!(
        dir.path().exists(),
        "the disk directory itself (e.g. a mount point) must survive a wipe -- only contents are removed"
    );
}
