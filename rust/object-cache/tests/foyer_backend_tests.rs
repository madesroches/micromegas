#![cfg(feature = "foyer")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use futures::future::join_all;

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

/// Values for every firing of the untagged integer metric `name` since the
/// guard was created (e.g. `range_cache_load_coalesced`, which carries no
/// `PropertySet`).
fn integer_metric_values(sink: &InMemorySink, name: &str) -> Vec<u64> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::IntegerMetricEvent(e) = evt
                && e.desc.name == name
            {
                out.push(e.value);
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

    let got = backend
        .get("key", data.len() as u64)
        .await
        .expect("get from disk tier");
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
    let got = backend
        .get("prefetched", data.len() as u64)
        .await
        .expect("get from disk tier");
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

    let got = backend.get("k", 4096).await.expect("hit");
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

    let got = backend
        .get("key", data.len() as u64)
        .await
        .expect("get from disk tier");
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

    let got = backend
        .get("blobs/key", data.len() as u64)
        .await
        .expect("get from disk tier");
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

// A disk->RAM promotion of a `blk:`-prefixed key must fire exactly one
// `object_cache_promotion_count{prefix=other}` (value 1) and exactly one
// `object_cache_promotion_bytes{prefix=other}` (value == the block length),
// verifying the promotion-volume telemetry added alongside `disk_tier_hit`
// in `promote_if_valid` (#1321). The key is `blk:`-prefixed (rather than the
// bare `"blobs/key"` used elsewhere in this file) because both new metrics
// only fire inside `promote_if_valid`'s `is_block_key` branch; `prefix=other`
// is expected (not `"blobs"`) because a `blk:`-prefixed key never matches a
// content-label prefix (see `is_block_key`'s doc comment and the caveat in
// `mkdocs/docs/admin/object-cache.md`).
#[tokio::test]
#[serial]
async fn promotion_volume_metrics_fire_on_disk_read() {
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
        .put(
            "blk:ns:blobs/key:0".to_string(),
            data.clone(),
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
    // close() awaits the flusher, so the read below is guaranteed to see
    // "blk:ns:blobs/key:0" on disk rather than racing the background write.
    backend.close().await.expect("close backend");

    let guard = init_in_memory_tracing();

    let got = backend
        .get("blk:ns:blobs/key:0", data.len() as u64)
        .await
        .expect("promote from disk tier");
    assert_eq!(got, data);

    micromegas_tracing::dispatch::flush_metrics_buffer();

    let counts = tagged_integer_metric_values(&guard.sink, "object_cache_promotion_count");
    let other_counts: Vec<u64> = counts
        .iter()
        .filter(|(_, props)| property_get(props.get_properties(), "prefix") == Some("other"))
        .map(|(v, _)| *v)
        .collect();
    assert_eq!(
        other_counts,
        vec![1],
        "expected exactly one promotion_count sample of 1 for prefix=other, got {counts:?}"
    );

    let bytes = tagged_integer_metric_values(&guard.sink, "object_cache_promotion_bytes");
    let other_bytes: Vec<u64> = bytes
        .iter()
        .filter(|(_, props)| property_get(props.get_properties(), "prefix") == Some("other"))
        .map(|(v, _)| *v)
        .collect();
    assert_eq!(
        other_bytes,
        vec![data.len() as u64],
        "expected exactly one promotion_bytes sample equal to the block length for prefix=other, \
         got {bytes:?}"
    );
}

// -- Validated-promotion two-step read (#1318) -------------------------------

// A disk entry whose stored length does not match the caller's
// `expected_len` (the poisoned-short-prefetch scenario the design doc's
// promotion gate exists for) must never be promoted into RAM: `get` reports
// it as a miss and RAM usage stays flat, and neither promotion metric fires
// (#1321 -- a mismatch must not count as a promotion; only
// `range_cache_promotion_len_mismatch` covers that case). A subsequent
// `put(Demand)` with the full-length bytes then heals the key normally.
// `#[serial]`: `init_in_memory_tracing` touches global dispatch state shared
// by every test in this file that uses it.
#[tokio::test]
#[serial]
async fn short_block_never_promoted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let full_len = 4096u64;
    let short_data = Bytes::from(vec![1u8; 100]);

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

    // `.force()` (the prefetch admission path) writes disk-only, so `close`
    // is enough to make the short entry durable without any RAM-eviction
    // dance. The key is `blk:`-prefixed so the mismatching `get` below
    // actually reaches `promote_if_valid`'s `is_block_key` branch -- a bare
    // key would make the negative metric assertion pass vacuously.
    backend
        .put(
            "blk:key".to_string(),
            short_data.clone(),
            FillHint::Prefetch,
        )
        .await;
    backend.close().await.expect("close backend");

    let ram_before = backend.ram_usage();
    let guard = init_in_memory_tracing();

    assert!(
        backend.get("blk:key", full_len).await.is_none(),
        "a length-mismatched disk entry must be reported as a miss, not promoted"
    );
    assert_eq!(
        backend.ram_usage(),
        ram_before,
        "a rejected disk entry must not grow RAM usage"
    );

    micromegas_tracing::dispatch::flush_metrics_buffer();
    assert!(
        tagged_integer_metric_values(&guard.sink, "object_cache_promotion_count").is_empty(),
        "a length mismatch must not fire object_cache_promotion_count"
    );
    assert!(
        tagged_integer_metric_values(&guard.sink, "object_cache_promotion_bytes").is_empty(),
        "a length mismatch must not fire object_cache_promotion_bytes"
    );

    let full_data = Bytes::from(vec![2u8; full_len as usize]);
    backend
        .put("blk:key".to_string(), full_data.clone(), FillHint::Demand)
        .await;
    let got = backend
        .get("blk:key", full_len)
        .await
        .expect("hit after healing put(Demand)");
    assert_eq!(got, full_data);
}

// A disk hit whose length matches `expected_len` must be promoted into the
// RAM tier (observable as RAM usage growth), and a repeat `get` must be
// served from RAM without touching disk again.
#[tokio::test]
async fn matching_disk_hit_is_promoted_into_ram() {
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

    backend
        .put("key".to_string(), data.clone(), FillHint::Prefetch)
        .await;
    backend.close().await.expect("close backend");

    let ram_before = backend.ram_usage();
    let got = backend
        .get("key", data.len() as u64)
        .await
        .expect("promote from disk");
    assert_eq!(got, data);
    let ram_after_promotion = backend.ram_usage();
    assert!(
        ram_after_promotion > ram_before,
        "a validated disk hit must be promoted into RAM: before={ram_before} \
         after={ram_after_promotion}"
    );

    let disk_stats_before = backend.disk_stats().expect("disk stats");
    let got2 = backend
        .get("key", data.len() as u64)
        .await
        .expect("second hit");
    assert_eq!(got2, data);
    assert_eq!(
        backend.ram_usage(),
        ram_after_promotion,
        "a repeat get must be a RAM hit, not a second promotion"
    );
    let disk_stats_after = backend.disk_stats().expect("disk stats");
    assert_eq!(
        disk_stats_after.read_ios, disk_stats_before.read_ios,
        "a RAM hit must not touch disk again"
    );
}

// N concurrent `get`s on the same cold (disk-resident, evicted-from-RAM) key
// must coalesce to exactly one disk read via the per-key single-flight
// (`FoyerBackend::load_from_disk`), and the follower count must be
// observable through `range_cache_load_coalesced`. `#[serial]`:
// `init_in_memory_tracing` touches global dispatch state shared by every
// test in this file that uses it.
#[tokio::test]
#[serial]
async fn concurrent_gets_on_cold_key_coalesce_to_one_disk_read() {
    const N: usize = 8;

    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let data = Bytes::from(vec![9u8; 4096]);

    // Same capacity-pressure pattern as `round_trip_through_disk_tier`: the
    // first 4096-byte put exactly fills the budget, so the following puts
    // evict it to the disk tier.
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
    backend.close().await.expect("close backend");

    let guard = init_in_memory_tracing();
    let backend = Arc::new(backend);
    let disk_stats_before = backend.disk_stats().expect("disk stats");

    let gets: Vec<_> = (0..N)
        .map(|_| {
            let backend = backend.clone();
            let data = data.clone();
            async move {
                let got = backend
                    .get("key", data.len() as u64)
                    .await
                    .expect("concurrent hit");
                assert_eq!(got, data);
            }
        })
        .collect();
    join_all(gets).await;

    let disk_stats_after = backend.disk_stats().expect("disk stats");
    assert_eq!(
        disk_stats_after.read_ios - disk_stats_before.read_ios,
        1,
        "N concurrent gets on the same cold key must coalesce to one disk read"
    );

    micromegas_tracing::dispatch::flush_metrics_buffer();
    let coalesced: u64 = integer_metric_values(&guard.sink, "range_cache_load_coalesced")
        .iter()
        .sum();
    assert_eq!(
        coalesced,
        (N - 1) as u64,
        "exactly N-1 of the N concurrent gets should join the single owner's load"
    );
}

// Regression for the original foyer #1318 clobber: a short (poisoned) disk
// entry seeded under a key, healed by a `put(Demand)` racing many concurrent
// reader loops. With every RAM write now promotion-gated or canonical, no
// writer in the system can ever produce non-canonical RAM contents, so the
// final read deterministically observes the healed bytes regardless of how
// the readers and the heal interleave.
#[tokio::test]
async fn heal_survives_concurrent_readers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");
    let full_len = 4096u64;
    let short_data = Bytes::from(vec![1u8; 100]);
    let healed_data = Bytes::from(vec![2u8; full_len as usize]);

    let backend = Arc::new(
        FoyerBackend::new_with_shards(
            dir_path,
            16 * 1024 * 1024,
            16 * 1024 * 1024,
            1,
            WriteTuning::default(),
            Arc::from(Vec::new()),
        )
        .await
        .expect("create backend"),
    );

    backend
        .put("key".to_string(), short_data.clone(), FillHint::Prefetch)
        .await;
    backend.close().await.expect("close backend");

    let stop = Arc::new(AtomicBool::new(false));
    let mut reader_handles = Vec::new();
    for _ in 0..8 {
        let backend = backend.clone();
        let stop = stop.clone();
        reader_handles.push(tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                let _ = backend.get("key", full_len).await;
                tokio::task::yield_now().await;
            }
        }));
    }

    backend
        .put("key".to_string(), healed_data.clone(), FillHint::Demand)
        .await;
    stop.store(true, Ordering::SeqCst);
    for h in reader_handles {
        h.await.expect("reader task join");
    }

    let got = backend.get("key", full_len).await.expect("get after heal");
    assert_eq!(got, healed_data);
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
    let got = backend.get("key", 16).await.expect("get after wipe");
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
