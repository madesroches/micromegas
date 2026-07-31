# Materialize-All-Views Failure Isolation Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1393

## Overview

[#1393](https://github.com/madesroches/micromegas/issues/1393): `materialize_all_views`
(`rust/public/src/servers/maintenance.rs:30`) aborts the entire materialization pass on the
first view that fails, via a bare `?` inside the per-view loop. Because the daemon's cron tasks
only look a short way back (`EveryDayTask`: 2 days, `EveryHourTask`: 2 hours), a view that fails
deterministically starves every view ordered after it, in every subsequent tick, with no signal
that anything was skipped. This plan makes per-view failures independent — one view's failure no
longer prevents any other view (same group or a later group) from being materialized — aggregates
the failures into a single reported error so the `CronTask` still records failure, and makes each
failure individually observable via a log line and a counter.

## Current State

`materialize_all_views` (`rust/public/src/servers/maintenance.rs:30-66`) iterates `views` (already
sorted by `get_update_group()` — `daemon`, `maintenance.rs:308`) and awaits
`materialize_partition_range(...).await?` for each one. The `?` means the first `Err` returned by
any view unwinds out of the function immediately: every view after it in `views` — regardless of
whether it's in the same update group or a later one — is skipped for that pass, silently.

The comment immediately above the group-boundary refetch (`maintenance.rs:44`, "views in the same
group should have no inter-dependencies") is about `SqlBatchView`'s `count_src_query` /
`transform_query`, which can `SELECT` from other registered views (`SqlBatchView::new`,
`sql_batch_view.rs:65-83`, takes a `view_factory` for exactly this). `update_group` is what
sequences that dependency: a SQL view that queries another view must be in a *later* group than
the view it queries, so that the queried view's partitions for `insert_range` already exist by the
time the query runs. Two views in the *same* group therefore cannot depend on each other's output
by construction — the comment is asserting that same-group views are safe to materialize in any
order, or with one's failure isolated from the other's, not merely documenting scheduling order.
This is the load-bearing fact behind the fix below: within a group, isolating failures is always
safe. Across groups it is a real trade-off (see Design).

Each `CronTask` (`EveryDayTask`, `EveryHourTask`, `EveryMinuteTask`, `EverySecondTask`) calls
`materialize_all_views(...).await` as its entire body and returns whatever it returns; `daemon`'s
`run_tasks_forever` logs any `Err` via `log_task_result` (`maintenance.rs:182-189`) but does not
otherwise inspect it — there is no per-view attribution today, only a single error for the whole
pass, blamed on whichever view happened to fail first.

## Design

### 1. Isolate failures within and across groups

Replace the `?` on the per-view `materialize_partition_range` call with an isolate-and-collect
pattern: on `Err`, log it (see §3) and push it into a `Vec`, then continue the loop instead of
returning. This naturally isolates failures both within a group (the existing requirement) and
across groups.

### 2. Cross-group policy: continue anyway

Per the issue's §2, the cross-group behavior needs an explicit, documented choice. This plan picks
**continue anyway**: a later group's view still gets its own materialization attempt this pass even
if an earlier group failed. Rationale:
- Materialization is idempotent and re-attempted every tick; if a later-group view genuinely reads
  an earlier-group view's output and that output is stale or missing for part of `insert_range`,
  the later view either produces a correspondingly partial/stale result this tick and a correct one
  next tick (once the dependency catches up), or its own query fails cleanly and is reported the
  same way as any other view failure — it does not corrupt anything.
- "Skip later groups when an earlier group failed" was the alternative; it reintroduces exactly the
  starvation this issue is about, just at group granularity instead of per-view, for a
  fail-persistently dependency. Given how rare cross-group SQL dependencies are today (a handful of
  `SqlBatchView`s can take one), the simpler uniform policy wins.
- This makes the within-group and cross-group cases the same code path: the loop simply never
  aborts on a view error, which keeps `materialize_all_views` a single, uniform loop rather than
  two different behaviors spliced together.

### 3. Make failures observable per view

On each view failure, before continuing the loop:
- `error!(...)` with the view's `get_view_set_name()` / `get_view_instance_id()` and the error
  (`{e:?}`), so an operator can grep logs for exactly which view/instance failed, without having to
  reconstruct it from a single aggregated message.
- `imetric!("materialize_view_failure", "count", tags, 1)` (pattern per `pg_stats.rs`'s use of
  `imetric!`/`fmetric!`) tagged with the view's `view_set_name` and `view_instance_id`, so a
  persistently failing view is visible as a non-zero counter over time via FlightSQL, rather than
  only in logs.

At the end of the pass, if any view failed, return a single aggregated `anyhow::Error` listing
every failed view's identity and error — so `CronTask`/`log_task_result` still records the pass as
failed (preserving today's "an error surfaces" behavior) without attributing it to only the first
view.

### Sketch

```rust
pub async fn materialize_all_views(
    lakehouse: Arc<LakehouseContext>,
    views: Views,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
) -> Result<()> {
    let mut last_group = views.first().unwrap().get_update_group();
    let mut partitions_all_views = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
            .await?,
    );
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    let mut failures = Vec::new();
    for view in &*views {
        if view.get_update_group() != last_group {
            last_group = view.get_update_group();
            partitions_all_views = Arc::new(
                PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
                    .await?,
            );
        }
        let view_set_name = view.get_view_set_name();
        let view_instance_id = view.get_view_instance_id();
        if let Err(e) = materialize_partition_range(
            partitions_all_views.clone(),
            lakehouse.clone(),
            view.clone(),
            insert_range,
            partition_time_delta,
            null_response_writer.clone(),
        )
        .await
        {
            error!("materialize_all_views: {view_set_name} {view_instance_id} failed: {e:?}");
            imetric!(
                "materialize_view_failure",
                "count",
                [
                    ("view_set_name", view_set_name.to_string()),
                    ("view_instance_id", view_instance_id.to_string()),
                ],
                1
            );
            failures.push(format!("{view_set_name} {view_instance_id}: {e:?}"));
        }
    }
    if !failures.is_empty() {
        anyhow::bail!(
            "materialize_all_views: {} view(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
    Ok(())
}
```

The `PartitionCache::fetch_overlapping_insert_range` calls (initial fetch and per-group refetch)
keep their `?`: those are infrastructure/DB errors unrelated to any specific view, and if the DB is
unreachable nothing in the pass can succeed anyway, so aborting immediately (as today) is still
correct — only the per-view materialization step gets isolated.

`imetric!`'s exact tag-list syntax should be checked against its current call sites
(`pg_stats.rs:86-104` etc.) before landing; the sketch above shows intent, not necessarily the
literal macro invocation shape.

## Implementation Steps

1. In `rust/public/src/servers/maintenance.rs`:
   - Update the update-group comment (currently `maintenance.rs:44`) to state explicitly that it
     refers to `SqlBatchView` cross-view SQL dependencies (see Current State), and that same-group
     views are therefore safe to materialize independently.
   - Change `materialize_all_views` to the isolate-and-collect loop in Design §1/§3.
   - Add a doc comment above `materialize_all_views` (or immediately above the loop) recording the
     cross-group policy decision from Design §2, so it doesn't read as an oversight again.
2. Add `micromegas_tracing::prelude::*`'s `imetric!` usage (already imported via the existing
   `use micromegas_tracing::prelude::*;`) for the failure counter.
3. Add a DB-backed regression test (see Testing Strategy) exercising same-group isolation and
   multi-failure aggregation.

## Files to Modify

- `rust/public/src/servers/maintenance.rs` — `materialize_all_views`, update-group comment.
- `rust/public/tests/materialize_fail_isolation_tests.rs` (new) — regression test.

## Trade-offs

- **Continue-anyway vs skip-later-groups** (Design §2): continue-anyway was chosen for its
  uniformity and because it can't make things worse than today's fail-fast (which already lets
  arbitrary partial completion happen from tick to tick); skip-later-groups was rejected as
  reintroducing coarser-grained starvation.
- **Aggregate error vs `JoinSet`/parallel materialization**: this plan keeps the loop sequential
  and just stops propagating the first error immediately, rather than switching to
  `tokio::task::JoinSet` to run views concurrently. Concurrent materialization is a bigger, unrelated
  change (shared `partitions_all_views` cache reads, DB connection pool pressure, no established
  precedent in this file for concurrent per-view work) and isn't needed to fix the starvation bug —
  isolating errors within the existing sequential loop is sufficient and minimal.
- **Metric emission on every failure vs periodic aggregate**: emitting one counter increment per
  failed view per pass (rather than, say, a per-pass "any failures" gauge) gives per-view
  attribution for free from the tags, matching the issue's ask that starvation be individually
  visible per view.

## Testing Strategy

Add `rust/public/tests/materialize_fail_isolation_tests.rs`, following the DB-backed harness
pattern in `rust/analytics/tests/sql_view_test.rs` / `thread_spans_ordering_db_test.rs`:
`#[ignore] #[tokio::test]`, reading `MICROMEGAS_SQL_CONNECTION_STRING` /
`MICROMEGAS_OBJECT_STORE_URI`, connecting via `connect_to_data_lake`.

Build the test views with `SqlBatchView::new` in the *same* `update_group`:
- A "failing" view whose `count_src_query` selects from a nonexistent table (or otherwise
  guarantees a DB error), so `make_batch_partition_spec` always errors.
- A "succeeding" view with a trivial, always-valid `count_src_query`/`transform_query` (e.g.
  counting/copying from `log_entries`, as `sql_view_test.rs` already does).

Assertions:
1. **Same-group isolation**: call `materialize_all_views` with both views in one group; assert it
   returns `Err`, the error text names the failing view, and the succeeding view actually produced
   a partition (query it back, or assert no error was attributed to it).
2. **Multiple failures reported**: add a second failing view (distinct `view_set_name`, same or
   different group); assert the aggregated error mentions both failing views, not just the first
   one encountered in iteration order.
3. **Cross-group continuation**: put the failing view in an earlier group than the succeeding view;
   assert the succeeding (later-group) view still materializes despite the earlier group's failure.

## Open Questions

None — the cross-group policy and observability mechanism are decided in Design §2/§3 above.
