//! Fixture helpers shared by the DB-backed integration test files (`*_db_test.rs`), each of which
//! is its own `#[ignore]`d `#[tokio::test]` binary requiring a live
//! `MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI`. Reached via `mod common;`.
//! Not every caller uses every helper here, so unused-item warnings are expected per binary --
//! silenced wholesale rather than duplicating helpers per file.
#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::TimeDelta;
use micromegas_analytics::lakehouse::batch_update::regenerate_partition_range;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::write_partition::{RetireMatch, retire_partitions};
use micromegas_analytics::response_writer::Logger;
use micromegas_analytics::time::TimeRange;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::levels::LevelFilter;
use std::sync::Arc;

/// Ensures the process-wide telemetry guard (ctrlc handler, global tracing subscriber) is
/// initialized exactly once for a `#[tokio::test]` in the calling file. `TelemetryGuardBuilder::build`
/// does process-global, one-time setup (`ctrlc::set_handler` allows exactly one handler; the
/// global tracing subscriber can only be installed once), and more than one DB-backed test can run
/// in the same test binary process -- so only the first caller actually builds and installs it.
/// The guard is intentionally leaked (never dropped): there is no natural per-test teardown point
/// when initialization is process-wide, and the process exits at the end of the test binary
/// regardless.
pub fn ensure_telemetry_guard() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let guard = TelemetryGuardBuilder::default()
            .with_ctrlc_handling()
            .with_local_sink_max_level(LevelFilter::Info)
            .build()
            .expect("telemetry guard");
        std::mem::forget(guard);
    });
}

/// Force-regenerates a global view's bucket(s) covering `insert_range` (which must exactly tile
/// `TimeDelta::hours(1)`, matching `materialize_global_view`'s own bucket size), bypassing
/// `materialize_partition_range`'s "already covered by *an* (even if stale) overlapping
/// partition" freshness check. Needed to make a re-materialization after new source rows have
/// been added (rather than the initial, first-time materialization) actually pick them up.
pub async fn regenerate_global_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
            .await?,
    );
    regenerate_partition_range(
        partitions,
        lakehouse,
        view,
        insert_range,
        TimeDelta::hours(1),
        logger,
    )
    .await?;
    Ok(())
}

/// Retires every partition of `view` that *overlaps* `insert_range`, then regenerates the range
/// from source.
///
/// `regenerate_global_view` alone is not enough on a shared, persistent dev lake:
/// `regenerate_partition_range` refuses a bucket that does not *fully contain* each existing
/// partition it would replace, so a single partition straddling a bucket boundary -- e.g. one an
/// older, non-hour-aligned version of a test left behind, or one written with a different
/// partition delta -- fails the run with "regeneration bucket ... does not fully contain existing
/// partition ...". Retiring by overlap first (`RetireMatch::Overlap`, which unlike
/// `RetireMatch::Containment` matches a partition that merely straddles the boundary) makes the
/// subsequent regeneration independent of whatever shape the lake happened to be in. Global
/// metadata views are derived from Postgres tables, so discarding and rebuilding them is free of
/// data loss.
pub async fn reset_global_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let mut tr = lakehouse.lake().db_pool.begin().await?;
    retire_partitions(
        &mut tr,
        &view.get_view_set_name(),
        &view.get_view_instance_id(),
        insert_range.begin,
        insert_range.end,
        RetireMatch::Overlap,
        &[],
        logger.clone(),
    )
    .await
    .with_context(|| "retiring overlapping partitions before regeneration")?;
    tr.commit().await.with_context(|| "commit")?;
    regenerate_global_view(lakehouse, view, insert_range, logger).await
}

/// A `CallerContext` scoped to `read_scope`, with `unstamped_audience` naming the escape hatch
/// that makes an unstamped process/row visible when it's in the caller's own scope.
pub fn caller_with_unstamped_audience(
    read_scope: ReadScope,
    unstamped_audience: &str,
) -> CallerContext {
    CallerContext {
        read_scope,
        is_admin: false,
        isolation_config: Arc::new(IsolationConfig {
            unstamped_audience: Some(unstamped_audience.to_string()),
            public_view_sets: vec![],
        }),
        admin_principal_possible: true,
    }
}
