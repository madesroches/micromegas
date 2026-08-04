//! Tests for `ScopedMemoryPool` (`tasks/1406_per_query_peak_memory_plan.md`):
//! - cross-query isolation: two concurrent queries over independently-scoped `RuntimeEnv`s
//!   built on the same shared inner pool don't see each other's memory usage
//! - balance at quiescence: every grow is matched by a shrink, so nothing leaks once all
//!   reservations are dropped
//! - delegation: `register`/`unregister`/`try_grow`/`memory_limit` all forward to the inner
//!   pool unchanged
//! - the `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` spill-cap helper

use datafusion::datasource::MemTable;
use datafusion::execution::memory_pool::{
    GreedyMemoryPool, MemoryConsumer, MemoryLimit, MemoryPool, TrackConsumersPool,
};
use datafusion::execution::runtime_env::{RuntimeEnv, RuntimeEnvBuilder};
use datafusion::prelude::{SessionConfig, SessionContext};
use micromegas_analytics::lakehouse::runtime::{
    apply_max_temp_directory_mb, parse_max_temp_directory_mb, scoped_runtime,
};
use micromegas_analytics::lakehouse::scoped_memory_pool::ScopedMemoryPool;
use std::num::NonZeroUsize;
use std::sync::Arc;

/// A schema/data set large enough that an `ORDER BY` over it drives DataFusion's
/// `ExternalSorter` to reserve real memory (see `sort.rs::reserve_memory_for_merge`,
/// gated on the runtime's disk manager having `tmp_files_enabled()`, which is the default).
fn make_sortable_table() -> Arc<MemTable> {
    use datafusion::arrow::array::Int64Array;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int64, false)]));
    // Several batches of descending values, so an ORDER BY actually has to sort rather
    // than short-circuit on an already-sorted input.
    let batches: Vec<RecordBatch> = (0..20)
        .map(|batch_idx| {
            let start = batch_idx * 10_000;
            let values: Vec<i64> = (0..10_000).map(|i| -(start + i)).collect();
            RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(values))])
                .expect("build record batch")
        })
        .collect();
    Arc::new(MemTable::try_new(schema, vec![batches]).expect("build MemTable"))
}

fn session_config() -> SessionConfig {
    // Set explicitly so the test isn't sensitive to the machine's core count.
    SessionConfig::new().with_target_partitions(2)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_query_isolation_and_balance_at_quiescence() {
    let shared: Arc<dyn MemoryPool> = Arc::new(TrackConsumersPool::new(
        GreedyMemoryPool::new(256 * 1024 * 1024),
        NonZeroUsize::new(5).unwrap(),
    ));

    let pool_sort = Arc::new(ScopedMemoryPool::new(shared.clone()));
    let pool_trivial = Arc::new(ScopedMemoryPool::new(shared.clone()));

    let runtime_sort = Arc::new(
        RuntimeEnvBuilder::new()
            .with_memory_pool(pool_sort.clone())
            .build()
            .expect("build sort runtime"),
    );
    let runtime_trivial = Arc::new(
        RuntimeEnvBuilder::new()
            .with_memory_pool(pool_trivial.clone())
            .build()
            .expect("build trivial runtime"),
    );

    let ctx_sort = SessionContext::new_with_config_rt(session_config(), runtime_sort);
    ctx_sort
        .register_table("t", make_sortable_table())
        .expect("register table");

    let ctx_trivial = SessionContext::new_with_config_rt(session_config(), runtime_trivial);

    let sort_fut = async {
        let df = ctx_sort
            .sql("SELECT * FROM t ORDER BY v")
            .await
            .expect("plan sort query");
        df.collect().await.expect("run sort query");
    };
    let trivial_fut = async {
        let df = ctx_trivial.sql("SELECT 1").await.expect("plan SELECT 1");
        df.collect().await.expect("run SELECT 1");
    };

    tokio::join!(sort_fut, trivial_fut);

    // Isolation: the trivial query never touched the pool; the sort did, and by more than
    // a loose threshold well under the guaranteed `sort_spill_reservation_bytes` floor
    // (default 10 MB) that `reserve_memory_for_merge` reserves per partition once the
    // sort processes any non-empty input batch.
    assert_eq!(pool_trivial.peak(), 0);
    assert!(
        pool_sort.peak() > 1_000_000,
        "expected the sort's peak to be well above 1 MB, got {}",
        pool_sort.peak()
    );

    // Balance at quiescence: both scoped pools and the shared pool they wrap are back
    // to zero once every reservation from these two queries has been dropped. This must
    // use pools the test owns (not a process-global pool) to avoid flakiness from other
    // concurrently running tests.
    drop(ctx_sort);
    drop(ctx_trivial);
    assert_eq!(pool_sort.current(), 0);
    assert_eq!(pool_trivial.current(), 0);
    assert_eq!(shared.reserved(), 0);
}

#[test]
fn try_grow_past_limit_fails_and_leaves_counters_unchanged() {
    let shared: Arc<dyn MemoryPool> = Arc::new(GreedyMemoryPool::new(1024));
    let scoped = Arc::new(ScopedMemoryPool::new(shared));
    let scoped_dyn: Arc<dyn MemoryPool> = scoped.clone();

    let reservation = MemoryConsumer::new("test-consumer").register(&scoped_dyn);

    reservation.try_grow(512).expect("grow within limit");
    assert_eq!(scoped.current(), 512);
    assert_eq!(scoped.peak(), 512);

    let err = reservation.try_grow(1024);
    assert!(err.is_err(), "growing past the pool limit should fail");
    assert_eq!(
        scoped.current(),
        512,
        "current must be unchanged after a failed try_grow"
    );
    assert_eq!(
        scoped.peak(),
        512,
        "peak must be unchanged after a failed try_grow"
    );
}

#[test]
fn memory_limit_forwards_through_the_wrapper() {
    let shared: Arc<dyn MemoryPool> = Arc::new(GreedyMemoryPool::new(4096));
    let scoped = ScopedMemoryPool::new(shared);

    match scoped.memory_limit() {
        MemoryLimit::Finite(n) => assert_eq!(n, 4096),
        _ => panic!("expected MemoryLimit::Finite(4096)"),
    }
}

#[test]
fn registered_consumer_is_visible_through_track_consumers_pool() {
    let inner = Arc::new(TrackConsumersPool::new(
        GreedyMemoryPool::new(1024 * 1024),
        NonZeroUsize::new(5).unwrap(),
    ));
    let shared: Arc<dyn MemoryPool> = inner.clone();
    let scoped: Arc<dyn MemoryPool> = Arc::new(ScopedMemoryPool::new(shared));

    let reservation = MemoryConsumer::new("scoped-consumer")
        .with_can_spill(true)
        .register(&scoped);
    reservation.grow(2048);

    // Proves `register`/`unregister` forwarding actually reached the inner
    // `TrackConsumersPool`, rather than just checking `reserved() == 0`, which would
    // hold even without that forwarding.
    let metrics = inner.metrics();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].name, "scoped-consumer");
    assert_eq!(metrics[0].reserved, 2048);

    let report = inner.report_top(5);
    assert!(
        report.contains("scoped-consumer"),
        "report_top should list the consumer registered through the wrapper: {report}"
    );
}

#[test]
fn apply_max_temp_directory_mb_sets_disk_manager_limit() {
    let builder = apply_max_temp_directory_mb(RuntimeEnvBuilder::new(), 42);
    let runtime: RuntimeEnv = builder.build().expect("build runtime env");
    assert_eq!(
        runtime.disk_manager.max_temp_directory_size(),
        42 * 1024 * 1024
    );
}

#[test]
fn parse_max_temp_directory_mb_rejects_non_numeric_value() {
    let result = parse_max_temp_directory_mb(Ok("not-a-number".to_string()));
    assert!(result.is_err());
}

#[test]
fn parse_max_temp_directory_mb_is_none_when_unset() {
    let result = parse_max_temp_directory_mb(Err(std::env::VarError::NotPresent));
    assert_eq!(result.expect("should not error"), None);
}

#[test]
fn parse_max_temp_directory_mb_errors_on_non_unicode_value() {
    // The exact bytes don't matter here, only that a set-but-non-UTF-8 value is treated
    // as an error rather than silently coerced to "unset" (which `VarError::NotPresent`
    // represents).
    let result =
        parse_max_temp_directory_mb(Err(std::env::VarError::NotUnicode("not-unicode".into())));
    assert!(
        result.is_err(),
        "a set-but-non-UTF-8 value must be treated as an error, not as unset"
    );
}

#[test]
fn scoped_runtime_preserves_shared_disk_manager_and_its_spill_cap() {
    let shared_builder = apply_max_temp_directory_mb(RuntimeEnvBuilder::new(), 42);
    let shared: RuntimeEnv = shared_builder.build().expect("build shared runtime");

    let scoped_pool = Arc::new(ScopedMemoryPool::new(shared.memory_pool.clone()));
    let scoped: Arc<RuntimeEnv> =
        scoped_runtime(&shared, scoped_pool).expect("build scoped runtime");

    assert!(
        Arc::ptr_eq(&shared.disk_manager, &scoped.disk_manager),
        "scoped_runtime must reuse the exact same DiskManager instance, not just an equal value"
    );
    assert_eq!(
        scoped.disk_manager.max_temp_directory_size(),
        42 * 1024 * 1024,
        "the configured spill cap must still apply to the per-query runtime"
    );
}
