//! DB-backed tests for the `RetireMatch::Overlap` contract as `net_spans_view` uses it.
//!
//! `net_spans_view` uses the same `BlockOrder::EventTime` grouping, `blocks_insert_time_range`
//! bounds, and `RetireMatch::Overlap` retirement with a `same_run_ranges` accumulator as
//! `thread_spans_view`, but unlike `thread_spans` it has no end-to-end coverage: net events are
//! produced only by the Unreal / C-API side, so there is no Rust-side stream type an ingestion
//! test could push blocks through
//! (contrast `ThreadStream` in `thread_spans_ordering_db_test.rs`). Synthesizing transit-encoded
//! net event payloads by hand would test the encoder more than the partitioning.
//!
//! What *is* net_spans-specific and testable without payloads is the retirement contract its
//! `update_partition` drives: `retire_partitions` is keyed only by
//! `(view_set_name, view_instance_id, range)`, so exercising it directly under
//! `view_set_name = "net_spans"` with synthetic `lakehouse_partitions` rows covers everything the
//! net_spans write path depends on -- the inclusive-bounds intersection predicate (overlap,
//! containment, degenerate, and touching shapes) and the same-run identity exclusion. The
//! block-grouping half of the change is already covered view-agnostically by
//! `jit_partition_grouping_tests.rs` and `jit_partition_bounds_tests.rs`.
//!
//! Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`; does not
//! run under a plain `cargo test`.

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_analytics::lakehouse::write_partition::{RetireMatch, retire_partitions};
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use std::sync::Arc;

const VIEW_SET_NAME: &str = "net_spans";

async fn connect() -> Result<DataLakeConnection> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    connect_to_data_lake(&connection_string, &object_store_uri).await
}

/// Inserts a synthetic partition row. `file_path` is unique per row so the cleanup queue the
/// retire path writes to stays unambiguous; no object-store file backs it, which is fine because
/// `retire_partitions` only enqueues the path into `temporary_files` (physical deletion happens
/// an hour later, out of band).
async fn insert_partition_row(
    lake: &DataLakeConnection,
    view_instance_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<String> {
    let file_path = format!("test/{}/{}.parquet", view_instance_id, uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO lakehouse_partitions
         (view_set_name, view_instance_id, begin_insert_time, end_insert_time,
          min_event_time, max_event_time, updated, file_path, file_size,
          file_schema_hash, source_data_hash, num_rows)
         VALUES ($1, $2, $3, $4, $3, $4, NOW(), $5, 1, $6, $7, 1);",
    )
    .bind(VIEW_SET_NAME)
    .bind(view_instance_id)
    .bind(begin)
    .bind(end)
    .bind(&file_path)
    .bind(vec![2u8])
    .bind(1i64.to_le_bytes().to_vec())
    .execute(&lake.db_pool)
    .await
    .with_context(|| "inserting synthetic partition row")?;
    Ok(file_path)
}

async fn surviving_ranges(
    lake: &DataLakeConnection,
    view_instance_id: &str,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let rows: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT begin_insert_time, end_insert_time FROM lakehouse_partitions
         WHERE view_set_name = $1 AND view_instance_id = $2
         ORDER BY begin_insert_time, end_insert_time;",
    )
    .bind(VIEW_SET_NAME)
    .bind(view_instance_id)
    .fetch_all(&lake.db_pool)
    .await
    .with_context(|| "listing surviving partitions")?;
    Ok(rows)
}

async fn retire(
    lake: &DataLakeConnection,
    view_instance_id: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    same_run_ranges: &[TimeRange],
) -> Result<()> {
    let mut tr = lake.db_pool.begin().await?;
    retire_partitions(
        &mut tr,
        VIEW_SET_NAME,
        view_instance_id,
        begin,
        end,
        RetireMatch::Overlap,
        same_run_ranges,
        Arc::new(ResponseWriter::new(None)),
    )
    .await
    .with_context(|| "retire_partitions")?;
    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

/// A fresh, random `view_instance_id` per test: `net_spans` instances are keyed by process id, and
/// these tests share a persistent dev lake with every other test and prior run.
fn instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A base instant truncated to microsecond precision: Postgres `timestamptz` columns only store
/// microseconds, so a nanosecond-precision `Utc::now()` would never compare equal to the same
/// value read back out of `lakehouse_partitions`.
fn base_time() -> Result<DateTime<Utc>> {
    Ok((Utc::now() - TimeDelta::minutes(30)).duration_trunc(TimeDelta::microseconds(1))?)
}

/// The overlap arm: a stale, wider partition from a previous run that merely *overlaps* the new,
/// narrower range (without containing it, and without being contained by it) must be retired.
/// This is the case `RetireMatch::Containment` cannot express, and the reason `net_spans_view`
/// passes `Overlap` -- its `BlockOrder::EventTime` grouping can move an earlier cut point between
/// `jit_update` runs, leaving exactly this shape behind.
#[ignore]
#[tokio::test]
async fn net_spans_overlap_retires_stale_wider_partition() -> Result<()> {
    let lake = connect().await?;
    let view_instance_id = instance_id();
    let t0 = base_time()?;

    // Stale run-1 partition covering [t0, t0+10s].
    insert_partition_row(&lake, &view_instance_id, t0, t0 + TimeDelta::seconds(10)).await?;

    // Run 2 writes [t0+5s, t0+15s]: overlaps the stale one, contains neither direction.
    retire(
        &lake,
        &view_instance_id,
        t0 + TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(15),
        &[],
    )
    .await?;

    assert!(
        surviving_ranges(&lake, &view_instance_id).await?.is_empty(),
        "the stale, merely-overlapping partition must be retired under RetireMatch::Overlap"
    );
    Ok(())
}

/// A degenerate new range: when the new partition's range is a single instant
/// (`begin_insert_time == end_insert_time`, i.e. every block in it shares one insert time), a
/// wider existing partition containing that instant must still be retired. With Postgres's
/// default half-open bounds `tstzrange(t, t)` is empty and `&&` is vacuously false; the
/// predicate's `'[]'` bounds make the degenerate range a non-empty singleton so the intersection
/// matches.
#[ignore]
#[tokio::test]
async fn net_spans_degenerate_new_range_retires_containing_partition() -> Result<()> {
    let lake = connect().await?;
    let view_instance_id = instance_id();
    let t0 = base_time()?;

    insert_partition_row(&lake, &view_instance_id, t0, t0 + TimeDelta::seconds(10)).await?;

    // New range is the single instant t0+5s, strictly inside the existing partition.
    let point = t0 + TimeDelta::seconds(5);
    retire(&lake, &view_instance_id, point, point, &[]).await?;

    assert!(
        surviving_ranges(&lake, &view_instance_id).await?.is_empty(),
        "a degenerate new range must still retire the wider partition containing its instant"
    );
    Ok(())
}

/// The same-run identity exclusion: partitions this run already wrote (or found up to date)
/// earlier in its own `jit_update` loop must survive a later partition's retire step, even when
/// they would otherwise match the intersection predicate. `net_spans_view::jit_update` threads a
/// fresh `same_run_ranges` accumulator through its loop for exactly this reason.
#[ignore]
#[tokio::test]
async fn net_spans_same_run_partitions_survive() -> Result<()> {
    let lake = connect().await?;
    let view_instance_id = instance_id();
    let t0 = base_time()?;

    // Two partitions written earlier in *this* run, plus one stale row from a previous run that
    // overlaps the range about to be written.
    let same_run = [
        TimeRange::new(t0, t0 + TimeDelta::seconds(4)),
        TimeRange::new(t0 + TimeDelta::seconds(4), t0 + TimeDelta::seconds(8)),
    ];
    for range in &same_run {
        insert_partition_row(&lake, &view_instance_id, range.begin, range.end).await?;
    }
    insert_partition_row(
        &lake,
        &view_instance_id,
        t0 + TimeDelta::seconds(9),
        t0 + TimeDelta::seconds(20),
    )
    .await?;

    // The third partition of this run spans [t0, t0+12s] -- it overlaps or contains all three
    // existing rows, but only the previous-run one may be retired.
    retire(
        &lake,
        &view_instance_id,
        t0,
        t0 + TimeDelta::seconds(12),
        &same_run,
    )
    .await?;

    let survivors = surviving_ranges(&lake, &view_instance_id).await?;
    assert_eq!(
        survivors,
        vec![
            (t0, t0 + TimeDelta::seconds(4)),
            (t0 + TimeDelta::seconds(4), t0 + TimeDelta::seconds(8)),
        ],
        "both same-run siblings must survive and the previous-run row must be retired"
    );
    Ok(())
}

/// Same-run protection is by *identity of this run's own writes*, not by range shape: several
/// consecutive same-run partitions sharing one identical degenerate range must all survive. An
/// earlier version of the predicate tried to carve same-run siblings out with ad hoc range-shape
/// clauses and could not distinguish this case from a genuinely stale cross-run partition of the
/// same shape.
#[ignore]
#[tokio::test]
async fn net_spans_same_run_degenerate_siblings_survive() -> Result<()> {
    let lake = connect().await?;
    let view_instance_id = instance_id();
    let t0 = base_time()?;

    // Two same-run partitions both degenerate at exactly t0 (possible when enough blocks share a
    // single insert_time), plus a stale previous-run row overlapping the range being written.
    let degenerate = TimeRange::new(t0, t0);
    insert_partition_row(&lake, &view_instance_id, t0, t0).await?;
    insert_partition_row(&lake, &view_instance_id, t0, t0).await?;
    insert_partition_row(
        &lake,
        &view_instance_id,
        t0 + TimeDelta::seconds(1),
        t0 + TimeDelta::seconds(5),
    )
    .await?;

    retire(
        &lake,
        &view_instance_id,
        t0,
        t0 + TimeDelta::seconds(6),
        &[degenerate, degenerate],
    )
    .await?;

    let survivors = surviving_ranges(&lake, &view_instance_id).await?;
    assert_eq!(
        survivors,
        vec![(t0, t0), (t0, t0)],
        "both identically-degenerate same-run siblings must survive; only the stale row goes"
    );
    Ok(())
}

/// A stale row from a *previous* run is never in `same_run_ranges` (a fresh, empty accumulator
/// every `jit_update`), so it is retired even when its range coincides with a shape this run also
/// touches -- here a degenerate row at an instant the new, wider range covers.
#[ignore]
#[tokio::test]
async fn net_spans_stale_degenerate_predecessor_is_retired() -> Result<()> {
    let lake = connect().await?;
    let view_instance_id = instance_id();
    let t0 = base_time()?;

    insert_partition_row(&lake, &view_instance_id, t0, t0).await?;

    // Run 2 regroups that block together with later ones into one wider partition starting at the
    // same instant -- the intersection must match the degenerate predecessor.
    retire(
        &lake,
        &view_instance_id,
        t0,
        t0 + TimeDelta::seconds(2),
        &[],
    )
    .await?;

    assert!(
        surviving_ranges(&lake, &view_instance_id).await?.is_empty(),
        "a stale degenerate partition must be retired by a growing partition sharing its left edge"
    );
    Ok(())
}

/// Touching ranges: a partition's `[begin_insert_time, end_insert_time]` is the *inclusive*
/// min/max of its blocks' insert times, so a stale partition ending exactly where the new range
/// begins can hold a block at that shared instant that the new grouping also covers -- it must be
/// retired, or both files would serve that block. Half-open ranges call this shape disjoint
/// (which is why the exclusion constraint permits it); the predicate's `'[]'` bounds catch it.
/// Cross-run only: a same-run touching sibling is protected by `same_run_ranges` identity, as
/// `net_spans_same_run_partitions_survive` asserts.
#[ignore]
#[tokio::test]
async fn net_spans_touching_stale_predecessor_is_retired() -> Result<()> {
    let lake = connect().await?;
    let view_instance_id = instance_id();
    let t0 = base_time()?;

    // Stale run-1 partition ending exactly at t0+5s, and a stale degenerate row at that instant.
    insert_partition_row(&lake, &view_instance_id, t0, t0 + TimeDelta::seconds(5)).await?;

    // Run 2 writes [t0+5s, t0+10s]: touches the stale partition at t0+5s without overlapping it
    // in the half-open sense.
    retire(
        &lake,
        &view_instance_id,
        t0 + TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(10),
        &[],
    )
    .await?;
    assert!(
        surviving_ranges(&lake, &view_instance_id).await?.is_empty(),
        "a stale partition touching the new range's left edge must be retired"
    );

    // Same shape with a degenerate *new* range: existing [t0, t0+5s], new [t0+5s, t0+5s].
    insert_partition_row(&lake, &view_instance_id, t0, t0 + TimeDelta::seconds(5)).await?;
    let point = t0 + TimeDelta::seconds(5);
    retire(&lake, &view_instance_id, point, point, &[]).await?;
    assert!(
        surviving_ranges(&lake, &view_instance_id).await?.is_empty(),
        "a stale partition ending at a degenerate new range's instant must be retired"
    );
    Ok(())
}
