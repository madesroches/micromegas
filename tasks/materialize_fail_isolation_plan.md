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
`extract_query`, which can `SELECT` from other registered views (`SqlBatchView::new`,
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
  only in logs. The tags are a `&'static PropertySet` built via
  `PropertySet::find_or_create(vec![Property::new("view_set_name", intern_string(&view_set_name)),
  Property::new("view_instance_id", intern_string(&view_instance_id))])` — the same
  `find_or_create`/`intern_string` pattern `pg_stats.rs` uses for its own tag helpers
  (`index_tags`/`table_tags`). Daemon-materialized views always have `view_instance_id == "global"`,
  so the tag cardinality is small and fixed, and interning per-failure is safe.

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
            let tags = PropertySet::find_or_create(vec![
                Property::new("view_set_name", intern_string(&view_set_name)),
                Property::new("view_instance_id", intern_string(&view_instance_id)),
            ]);
            imetric!("materialize_view_failure", "count", tags, 1);
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

The tag-list syntax above (`PropertySet::find_or_create(vec![Property::new(...), ...])` passed as
the `tags` argument to `imetric!`) matches the existing call shape in `pg_stats.rs:45-59, 86-104`
and is what should land — the two-argument-plus-tags form of `imetric!` expects a `&'static
PropertySet`, not an inline array literal.

## Implementation Steps

1. In `rust/public/src/servers/maintenance.rs`:
   - Update the update-group comment (currently `maintenance.rs:44`) to state explicitly that it
     refers to `SqlBatchView` cross-view SQL dependencies (see Current State), and that same-group
     views are therefore safe to materialize independently.
   - Change `materialize_all_views` to the isolate-and-collect loop in Design §1/§3.
   - Add a doc comment above `materialize_all_views` (or immediately above the loop) recording the
     cross-group policy decision from Design §2, so it doesn't read as an oversight again.
2. Add the failure-counter `imetric!` call (§3). `imetric!` itself is already available via the
   existing `use micromegas_tracing::prelude::*;`, but `PropertySet`, `Property`, and
   `intern_string` are not re-exported by the prelude — add explicit imports, matching
   `pg_stats.rs`: `use micromegas_tracing::intern_string::intern_string;` and
   `use micromegas_tracing::property_set::{Property, PropertySet};`.
3. Add a DB-backed regression test (see Testing Strategy) exercising same-group isolation.
4. In `rust/public/Cargo.toml`, append a `[[test]]` block for the new test file, mirroring the
   existing `pg_stats_test` entry:
   ```toml
   [[test]]
   name = "materialize_fail_isolation_tests"
   path = "tests/materialize_fail_isolation_tests.rs"
   required-features = ["server"]
   ```
   This is required because `pub mod servers;` in `rust/public/src/lib.rs` is gated behind the
   `server` feature (not a default feature); without this entry, a plain `cargo test` (no
   `--features server`) fails to compile the new test.

## Files to Modify

- `rust/public/src/servers/maintenance.rs` — `materialize_all_views`, update-group comment.
- `rust/public/tests/materialize_fail_isolation_tests.rs` (new) — regression test.
- `rust/public/Cargo.toml` — add the `[[test]]` entry for the new test file.
- `mkdocs/docs/admin/maintenance.md` — document `materialize_view_failure` and update the
  failure/starvation behavior description.

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

## Documentation

Update `mkdocs/docs/admin/maintenance.md`: add `materialize_view_failure` (count, tags
`{view_set_name, view_instance_id}`) to the metrics table alongside the existing
self-observability metrics, and adjust the task-table/behavior description so it no longer implies
a single failing view starves later ones — per-view failures are now isolated and individually
observable.

## Testing Strategy

Add `rust/public/tests/materialize_fail_isolation_tests.rs`, following the DB-backed harness
pattern in `rust/analytics/tests/sql_view_test.rs` / `thread_spans_ordering_db_test.rs`:
`#[ignore] #[tokio::test]`, reading `MICROMEGAS_SQL_CONNECTION_STRING` /
`MICROMEGAS_OBJECT_STORE_URI`, connecting via `connect_to_data_lake`.

Build the test views with `SqlBatchView::new` in the *same* `update_group`:
- A "failing" view whose `count_src_query` selects from a nonexistent table (or otherwise
  guarantees a DB error), so `make_batch_partition_spec` always errors. Its `extract_query` must
  stay valid — e.g. a trivial select from `log_entries`, same
  as the succeeding view below — because `SqlBatchView::new` unconditionally runs `extract_query` at
  construction time to derive the schema (`sql_batch_view.rs:97-101`), independent of
  `count_src_query`. If both queries target the same nonexistent table, `SqlBatchView::new()` itself
  errors out during test setup, before `materialize_all_views` is ever reached, so only
  `count_src_query` should be broken.
- A "succeeding" view with a trivial, always-valid `count_src_query`/`extract_query` (e.g.
  counting/copying from `log_entries`, as `sql_view_test.rs` already does).

Call `materialize_all_views` once with both views in the same update group and assert:
1. it returns `Err`; and
2. the succeeding view actually produced a partition (query it back), despite being ordered after
   (or before) the failing one.

The aggregated-error format (multiple failures listed) and cross-group continuation follow
directly from the same uniform per-view isolate-and-collect loop as same-group isolation, so they
don't need separate test scenarios.

## Open Questions

None — the cross-group policy and observability mechanism are decided in Design §2/§3 above.
