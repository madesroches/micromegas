//! Coverage for the foyer disk write-path gauges in `saturation_monitor`
//! (`object_cache_foyer_disk_*`), which replaced the sysinfo-based
//! `object_cache_ssd_*` gauges that always read 0 in the deployed container.
//! Drives `sample_once` directly against a `FoyerBackend`-backed `RangeCache`
//! with `prev` threaded across two ticks, per the write-tuning plan's
//! testing strategy.

use std::sync::Arc;

use bytes::Bytes;
use micromegas::object_cache::backend::{BackendDiskStats, FillHint, RangeCacheBackend};
use micromegas::object_cache::foyer_backend::{FoyerBackend, WriteTuning};
use micromegas::object_cache::range_cache::{
    DEFAULT_BLOCK_SIZE, DEFAULT_DEMAND_RESERVED_FETCH_PERMITS, DEFAULT_MAX_COALESCED_GET_BYTES,
    DEFAULT_PROMOTE_WHOLE_BATCH, DEFAULT_TOTAL_FETCH_PERMITS, RangeCache,
};
use micromegas::tracing::event::in_memory_sink::InMemorySink;
use micromegas::tracing::metrics::MetricsMsgQueueAny;
use micromegas::tracing::test_utils::init_in_memory_tracing;
use micromegas_object_cache_srv::saturation_monitor::sample_once;
use micromegas_transit::HeterogeneousQueue;
use object_store::ObjectStore;
use object_store::memory::InMemory;
use serial_test::serial;
use sysinfo::Networks;
use tokio::sync::{Semaphore, mpsc};

/// Values fired for the untagged float metric `name` since the guard was
/// created. Requires `dispatch::flush_metrics_buffer` first.
fn float_metric_values(sink: &InMemorySink, name: &str) -> Vec<f64> {
    let state = sink.state.lock().expect("sink lock");
    let mut out = Vec::new();
    for block in &state.metrics_blocks {
        for evt in block.events.iter() {
            if let MetricsMsgQueueAny::FloatMetricEvent(e) = evt
                && e.desc.name == name
            {
                out.push(e.value);
            }
        }
    }
    out
}

/// Values fired for the untagged integer metric `name` since the guard was
/// created. Requires `dispatch::flush_metrics_buffer` first.
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

/// (a) The first `sample_once` tick, with `prev` seeded `None`, must emit no
/// `object_cache_foyer_disk_*` gauge (there's no prior sample to diff
/// against yet). (b) After a write lands on disk and a second tick runs,
/// `object_cache_foyer_disk_write_bytes_per_sec` must fire with a positive
/// rate.
#[tokio::test]
#[serial]
async fn foyer_disk_gauges_emit_only_after_a_second_tick() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let foyer = Arc::new(
        FoyerBackend::new_with_shards(
            dir_path,
            4096,
            16 * 1024 * 1024,
            1,
            WriteTuning::default(),
            Arc::from(Vec::new()),
        )
        .await
        .expect("create FoyerBackend"),
    );
    let store = Arc::new(InMemory::new());
    let cache = RangeCache::new(
        store as Arc<dyn ObjectStore>,
        foyer.clone(),
        DEFAULT_BLOCK_SIZE,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    let mem_permits = Arc::new(Semaphore::new(4));
    let (prefetch_tx, _prefetch_rx) = mpsc::channel(1);
    let mut networks = Networks::new_with_refreshed_list();
    let mut prev_disk_stats: Option<BackendDiskStats> = None;

    let guard = init_in_memory_tracing();

    // First tick: no prior sample, so no foyer disk gauge fires regardless
    // of whether any writes have happened yet.
    sample_once(
        &cache,
        &mem_permits,
        64,
        &prefetch_tx,
        &mut networks,
        &mut prev_disk_stats,
        5.0,
    );
    micromegas::tracing::dispatch::flush_metrics_buffer();
    assert!(
        float_metric_values(&guard.sink, "object_cache_foyer_disk_write_bytes_per_sec").is_empty(),
        "the first tick must not emit a rate: there is no prior sample to diff against"
    );
    assert!(
        prev_disk_stats.is_some(),
        "the first tick must still seed prev from the foyer backend's Some disk_stats"
    );

    // Force-insert a block (bypassing admission, like a real prefetch fill)
    // and close() to await the flusher, so the write is durable and
    // `disk_stats()` reflects it deterministically before the second tick.
    foyer
        .put(
            "key".to_string(),
            Bytes::from(vec![9u8; 8192]),
            FillHint::Prefetch,
        )
        .await;
    foyer.close().await.expect("close backend flushes to disk");

    sample_once(
        &cache,
        &mem_permits,
        64,
        &prefetch_tx,
        &mut networks,
        &mut prev_disk_stats,
        5.0,
    );
    micromegas::tracing::dispatch::flush_metrics_buffer();
    let write_rates =
        float_metric_values(&guard.sink, "object_cache_foyer_disk_write_bytes_per_sec");
    assert_eq!(
        write_rates.len(),
        1,
        "the second tick must emit exactly one write-bytes rate"
    );
    assert!(
        write_rates[0] > 0.0,
        "a completed disk write between the two ticks must produce a positive rate"
    );
}

/// `object_cache_ram_tier_usage_bytes` must fire every tick (unlike the foyer
/// disk rate gauges, it needs no prior sample), and after a demand `put` of
/// one block, the sampled value must be at least that block's size --
/// confirming the accounted RAM-tier usage gauge from the demand-fill-copy
/// plan (#1276) makes the tier's residency observable.
#[tokio::test]
#[serial]
async fn ram_tier_usage_gauge_reflects_demand_put() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let foyer = Arc::new(
        FoyerBackend::new_with_shards(
            dir_path,
            1024 * 1024,
            16 * 1024 * 1024,
            1,
            WriteTuning::default(),
            Arc::from(Vec::new()),
        )
        .await
        .expect("create FoyerBackend"),
    );
    let store = Arc::new(InMemory::new());
    let cache = RangeCache::new(
        store as Arc<dyn ObjectStore>,
        foyer.clone(),
        DEFAULT_BLOCK_SIZE,
        "test".to_string(),
        DEFAULT_TOTAL_FETCH_PERMITS,
        DEFAULT_DEMAND_RESERVED_FETCH_PERMITS,
        DEFAULT_MAX_COALESCED_GET_BYTES,
        DEFAULT_PROMOTE_WHOLE_BATCH,
    );

    let mem_permits = Arc::new(Semaphore::new(4));
    let (prefetch_tx, _prefetch_rx) = mpsc::channel(1);
    let mut networks = Networks::new_with_refreshed_list();
    let mut prev_disk_stats: Option<BackendDiskStats> = None;

    let guard = init_in_memory_tracing();

    sample_once(
        &cache,
        &mem_permits,
        64,
        &prefetch_tx,
        &mut networks,
        &mut prev_disk_stats,
        5.0,
    );
    micromegas::tracing::dispatch::flush_metrics_buffer();
    let before = integer_metric_values(&guard.sink, "object_cache_ram_tier_usage_bytes");
    assert_eq!(
        before.len(),
        1,
        "the gauge must fire every tick, with no prior-sample requirement"
    );

    let block = Bytes::from(vec![9u8; 4096]);
    foyer
        .put("key".to_string(), block.clone(), FillHint::Demand)
        .await;

    sample_once(
        &cache,
        &mem_permits,
        64,
        &prefetch_tx,
        &mut networks,
        &mut prev_disk_stats,
        5.0,
    );
    micromegas::tracing::dispatch::flush_metrics_buffer();
    let after = integer_metric_values(&guard.sink, "object_cache_ram_tier_usage_bytes");
    assert_eq!(after.len(), 2, "the gauge must fire on the second tick too");
    assert!(
        after[1] >= block.len() as u64,
        "the sampled RAM-tier usage after a demand put must reflect that block's size: {after:?}"
    );
}
