# On-Demand Materialized Views Plan

Issue: [#1778](https://github.com/madesroches/micromegas/issues/1778)

## Overview

A DDL view defined without `update_group` becomes an **on-demand view**. The maintenance daemon
never touches it. A query fills and refreshes the partitions covering its own time range, so
nothing is materialized for ranges nobody reads. This is how the built-in per-process JIT views
already work. It also builds the engine #1495's `cached_range` will reuse: a JIT driver keyed on
a per-bucket source count, with no dependency on `blocks`.

## Current State

- **DDL parsing.** `view_ddl.rs::view_definition_from_options` requires `update_group`
  (`update_group.context("missing required option 'update_group'")`). It defaults
  `source_partition_delta` to `'1 day'`, and `merge_partition_delta` to the source delta.
- **Definition shape.** `ViewDefinition.update_group: i32` (`view_definition.rs`).
  `ViewDefinitionRow.update_group: i32` and the upsert are in `view_definition_store.rs`.
  `DdlRow.update_group: i32` is in `list_view_set_definitions_table_function.rs`. Its output
  column is already `Int32` nullable, because built-in rows report `None`.
  `build_sql_batch_view` always passes `Some(def.update_group)`.
- **Schema.** `lakehouse_view_set_definitions.update_group INTEGER NOT NULL`
  (`migration.rs::upgrade_v9_to_v10`). `LATEST_LAKEHOUSE_SCHEMA_VERSION = 11`.
- **Daemon.** `maintenance.rs::get_global_views_with_update_group` keeps only views whose
  `get_update_group()` is `Some`. Every built-in global view (`blocks`, `processes`, `streams`,
  `log_entries`, `measures`) returns `Some`. So among global views, `None` currently never
  occurs, and after this change it will mean "on-demand DDL view".
- **JIT entry point.** `MaterializedView::scan` calls `view.jit_update(lakehouse, query_range)`
  before fetching partitions. `query_range` is the session's range, which is `None` for daemon
  and validation sessions. A JIT failure on a read-only connection becomes a query error with a
  clear message, not a silent gap. `SqlBatchView::jit_update` returns `Ok(())`.
- **SQL partitions.** `SqlBatchView::make_batch_partition_spec(lakehouse, partitions, range)`
  runs `count_src_query` over `range` and returns a `SqlPartitionSpec`. The count is in
  `get_source_data_hash()`. `write()` runs `extract_query` and writes one partition whose insert
  range is exactly `[range.begin, range.end]`, retiring with `RetireMatch::Containment`. On a
  lost race, the insert fails with `PartitionOverlapConflict`, under the per-range advisory lock
  and the `lakehouse_partitions_no_overlap` constraint. The daemon's freshness rule
  (`batch_update.rs::verify_overlapping_partitions`) compares the sum of the covered rows'
  `source_data_hash` against the count. A merged partition records the sum of its inputs
  (`merge.rs`).
- **#1496 machinery.**
  - `partition_ladder.rs`: `select_rung`, `plan_buckets`, `align_outward`. `plan_buckets`
    descends open buckets down to the finest rung. It needs per-minute block counts.
  - `jit_partitions.rs::update_insert_time_jit_view`: the block-driven driver.
    `resolve_bucket_coverage` and `judge_bucket` assume block-hull ranges: inclusive
    `[min, max]` of block insert times, which never touch a bucket edge.
  - `jit_promotion.rs`: `promote_closed_buckets` and `promote_one`. They test rows with
    `row_inside` (`end < bucket.end`) and treat any row with `end >= bucket.begin` as
    straddling. Both tests are wrong for SQL partitions, whose `end_insert_time` equals the
    bucket end and touches the next bucket's begin.
- **Validation.** `validate_view_definition` walks the extract, count and merge plans
  (`walk_plan_for_scans` / `inspect_scan`) and computes the highest source `update_group`. It
  ignores sources with `None`.
- **Registry build order.** `build_factory` folds rows in `(update_group, view_set_name)` order
  and builds each row against the factory so far. A view that reads a not-yet-folded view fails
  to build.
- **Access.** `AudienceGuard::authorize_view_instance` rule 3 passes any `'global'` instance on
  the stated invariant "`'global'` never triggers JIT".
- **Admission.** `flight_sql_service_impl.rs` runs `estimate_scan_cost` on the physical plan,
  *after* `create_physical_plan`. `scan()`, and therefore `jit_update`, runs inside
  `create_physical_plan`. So JIT work, built-in or on-demand, happens before the admission gate
  and is not part of the estimate.

## Design

### 1. Spelling and definition

- An on-demand view is a DDL view with **no `update_group`**. `ViewDefinition.update_group`
  becomes `Option<i32>`, and so does `ViewDefinitionRow.update_group` (and `DdlRow`).
  `build_sql_batch_view` passes `def.update_group` through as is.
- `view_definition_from_options`:
  - An absent `update_group` is accepted.
  - On an on-demand view, an explicit `source_partition_delta` or `merge_partition_delta` is a
    hard error: "`source_partition_delta` only applies to daemon-updated views (set
    `update_group`), on-demand partition sizes follow the query range".
  - The stored defaults are unchanged (`'1 day'`), so `ViewOptions` keeps its shape and no
    serde change is needed.
- Migration v11→v12:
  `ALTER TABLE lakehouse_view_set_definitions ALTER COLUMN update_group DROP NOT NULL`. Existing
  rows keep their groups, so no definition changes meaning.
- `ViewDefinitionStore::list` orders by `update_group NULLS LAST, view_set_name`. That is
  already Postgres's default for `ASC`. Spell it out so on-demand rows come last on purpose.
- `SqlBatchView` gains `fn is_on_demand(&self) -> bool { self.update_group.is_none() }`. That is
  enough: the daemon already filters on `get_update_group()`.

### 2. Dependencies

- **Daemon view reading an on-demand view: rejected.** `inspect_scan` additionally records
  whether any scanned `MaterializedView` is a `'global'` instance with
  `get_update_group() == None`. `validate_view_definition` fails a definition with
  `update_group = Some(_)` that has such a source: "`X` reads on-demand view `Y`; a
  daemon-updated view cannot read an on-demand one". This goes next to the existing
  update-group ordering check.
- **On-demand view reading on-demand views: allowed, made explicit.** The JIT driver must bring
  its on-demand sources up to date *before* it snapshots source partitions. The extract and count
  sessions are `SessionRange::Snapshot` sessions over a `PartitionCache` loaded beforehand. A
  scan inside them cannot refresh upstream in time (see Trade-offs and Decisions). So:
  - The same walk also collects the on-demand source views:
    `walk_plan_for_scans(..., on_demand_sources: &mut Vec<Arc<dyn View>>)`.
    `validate_view_definition` returns them: `Result<Vec<Arc<dyn View>>>` instead of `Result<()>`.
  - `build_factory` attaches them with `view.with_on_demand_sources(sources)` before
    `add_global_view`. `build_factory` is the only attach site for served views. The DDL
    executor's direct validation build (`build_sql_batch_view` + `validate_view_definition`)
    ignores the returned sources.
- **Registry ordering for chains.** On-demand rows have no group to sort by, so `B` may sort
  before an on-demand `A` it reads. `build_factory` becomes a fixpoint:
  1. Fold every row that builds and validates.
  2. Retry the rows that failed while the previous pass added at least one view.
  3. Report as failed only the rows that still fail when a pass adds nothing.

  Daemon rows are unaffected: their group order already puts sources first. A cycle (`A` reads
  `B` reads `A`) can never build. The executor's direct validation build refuses the statement
  that would create it, because its source is not in the validation rows yet.
  `check_dependents_survive` is a backstop.

### 3. JIT driver

New module `rust/analytics/src/lakehouse/on_demand_jit.rs`, with one entry point:

```rust
pub async fn update_on_demand_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    on_demand_sources: &[Arc<dyn View>],
    query_range: TimeRange,
) -> Result<()>
```

`SqlBatchView::jit_update` behaves like this:

- `update_group.is_some()`: it stays a no-op.
- `query_range == None`: a no-op. `MaterializedView::scan` has already rejected
  `SessionRange::Unbounded` and short-circuited `NoPartitions` (§3a). `None` therefore means
  `Snapshot`, whose caller already refreshed the on-demand sources.
- Otherwise it calls the entry point above with `Arc::new(self.clone())` as the view, because
  promotion hands the view to `create_merged_partition`. `SqlBatchView` derives `Clone` (all
  fields are `Arc` or `Copy`), so this needs no new state.

**The probe is `View::make_batch_partition_spec`.** It already runs the authored count over a
range, and it returns a spec that can write that exact range. The driver touches nothing
block-specific. #1495 plugs in by implementing `make_batch_partition_spec` on its anonymous view
type, with a count derived from source partitions' `num_rows`. No new trait.

Steps, for one `jit_update`:

1. **Range.**
   - `rung = select_rung(query_range)`.
   - `insert_range = align_outward(query_range, rung)`, with `end` clamped to
     `align_outward(now)` at `LADDER[0]`. This avoids materializing empty future buckets.
   - The query range is used as the insert range. That is how every DDL view keys partitions
     (`{begin}`/`{end}` are insert times), and the read side already selects partitions by event
     time. Rows inserted long after their event time are discussed under Trade-offs.
2. **Plan.** Call `plan_dense_buckets(insert_range, rung, LADDER[0], now, grace)`. This is a new
   `partition_ladder` function that shares `plan_bucket`'s recursion through a count closure;
   `plan_buckets` keeps its signature. Every bucket is assumed non-empty, and there is no
   object-count descent. Closed buckets are emitted at `rung`. Only the open edge descends,
   down to one minute, as in #1496. Any sub-bucket whose `begin >= insert_range.end` (the clamped
   now) is dropped, so the open edge stops at the current minute. `grace` comes from
   `JitPartitionConfig::default()` (90s), the same rule the built-in JIT views follow.
3. **Rows.** `fetch_freshness_candidates(view_meta, insert_range)` gets this view's rows.
4. **Units (pure).** Call `plan_units(&planned, &rows) -> Vec<Unit>`. Each planned bucket grows
   until it contains every row that intersects it. Rows are half-open (`begin < unit.end &&
   end > unit.begin`), because SQL partitions touch at bucket edges. Units that come to overlap
   are coalesced. A `Unit` records:
   - `range`;
   - `rows`: indices into `rows`, all fully inside `range`;
   - `closed`: every bucket it absorbed was closed;
   - `planned_rung`: `Some(rung)` when the unit is exactly one planned bucket.

   Normally a unit is just the planned bucket, holding its finer rows. It grows when a coarser
   row covers the bucket (an earlier query at a coarser rung, or a promotion), or when a view
   converted from daemon-updated left rows on the daemon's grid. Because units contain every row they touch,
   `Containment` always retires everything a write replaces, and no `Overlap` mode is needed.
5. **Upstream and snapshot.** Units can extend past `insert_range`, so the sources must cover
   their hull: `source_range` = hull of all unit ranges (a superset of `insert_range`).
   - If `source_range` grew, refetch the rows over it and re-run `plan_units` once (units are
     contained by construction, so the hull does not grow again).
   - For each on-demand source, call `source.jit_update(lakehouse, Some(source_range))`.
   - `PartitionCache::fetch_overlapping_insert_range(pool, source_range)` gets every view's
     partitions. This is the source provider for the probes and extracts, the same shape the
     daemon hands `make_batch_partition_spec`.
6. **Judge (pure, after one probe per unit).** `spec = view.make_batch_partition_spec(lakehouse,
   cache, unit.range)`. `judge_unit(unit, rows, file_schema_hash, probed_count) -> UnitVerdict`
   returns `Covered` when the unit has rows, every row has the view's `file_schema_hash`, and
   `sum(source_data_hash) == probed_count`. Otherwise it returns `Stale`. This is
   `verify_overlapping_partitions`' rule (equality, summed), so a definition gives the same
   verdicts under either writer.
7. **Write stale units.** Call `spec.write(lake, null_logger)`. On `PartitionOverlapConflict`,
   which means a concurrent reader wrote an overlapping coarser or finer unit first:
   1. Refetch the rows over the unit's range.
   2. Recompute the unit (it may grow).
   3. Probe and judge again: `Covered` drops the write; `Stale` retries once.

   A second conflict is an error. This mirrors `write_spec_with_conflict_retry`'s
   drop-or-retry shape.
8. **Promote.** A unit qualifies when it is `Covered`, `closed`, and `planned_rung ==
   Some(rung)`, and holds at least 2 rows that pass `is_promotable`. It becomes a
   `PromotionTarget { bucket: unit.range, inputs }` and goes to `promote_closed_buckets`, which
   runs the authored `merge_partitions_query` through `create_merged_partition`. The driver passes
   `i64::MAX` as `max_nb_objects` (the cap has no meaning for authored counts), and
   `is_promotable` (private today) becomes `pub(crate)` for the eligibility check above. Units written
   in this run are not promoted until the next query, the same as #1496.

Planned bucket count for a 7-day query: `rung = 6h`, 28 closed buckets, plus the open edge's
descent up to the current minute: in the worst case up to 5 1h + 11 5min + 4 1min buckets under
the last 6h bucket. So about 50 probes, each one `make_session_context` plus one count query.
The cost is close to the daemon's per-partition cost and is paid only by the reader who asks.

### 3a. Unbounded queries are rejected, not answered from whatever exists

If an on-demand view is queried with no time range, it would serve only the partitions earlier
reads happened to leave, and the reader would get no sign the answer is incomplete. So a user
query must give an on-demand view a range, and fails otherwise:

> `od_log_counts` is an on-demand view and needs a query time range (pass begin/end with the query)

The obstacle is that today's `query_range: None` means three different things, and only one of
them should fail:

| What the session is | Today | Should an on-demand view… |
|---|---|---|
| A user query with no range | `None` | fail |
| An internal session over a partition snapshot its caller prepared: daemon and on-demand extract and count sessions (`make_batch_partition_spec`), merges, `export_log_view` | `None` + `PartitionCache` | read the snapshot; its caller already refreshed upstream (§2) |
| Planning only, never reading data: validation contexts, `SqlBatchView::new`'s schema discovery | `None` + `NullPartitionProvider` | yield nothing, run no JIT |

The read scope can't separate them: unauthenticated user queries also get `ReadScope::All`. So
the range is made explicit. `make_session_context` and `MaterializedView` take a `SessionRange`
instead of `Option<TimeRange>`:

```rust
pub enum SessionRange {
    /// User query over a time range: event-time pruning (`TableScanRewrite`) + JIT over it.
    Query(TimeRange),
    /// User query with no time range: no pruning; views that need a range refuse.
    Unbounded,
    /// Internal session over the caller's partition snapshot. The caller already brought on-demand
    /// sources up to date for the snapshot's insert range, so no range-driven JIT runs here.
    Snapshot,
    /// Planning only: an explicit absence of partitions. Scans yield an empty plan with the view's
    /// schema, with no partition fetch and no JIT.
    NoPartitions,
}
```

- `View` gains `fn requires_query_range(&self) -> bool { false }`. `SqlBatchView` returns
  `is_on_demand()`.
- `MaterializedView::scan`:
  - `NoPartitions`: returns an `EmptyExec` over the schema.
  - `Unbounded` with `view.requires_query_range()`: the error above.
  - Otherwise as today. `jit_update` receives `Some(range)` for `Query` and `None` for the
    other variants, so built-in views behave exactly as they do now.
- `TableScanRewrite` is installed only for `Query`.
- Changing the parameter type makes the compiler enumerate every call site, about 20 in the
  sources and tests. Each picks its variant from the table above. FlightSQL picks `Query` or
  `Unbounded`. Callers that pass `NullPartitionProvider` only to plan switch to `NoPartitions`.
- `materialize_partitions` and `regenerate_partitions` reject on-demand views (see Decisions).

### 4. Promotion with aligned ranges

SQL partitions use bucket-aligned ranges, so `jit_promotion` needs a second row convention.
Add a `RangeConvention` enum:

- `BlockHull` (today's inclusive tests): inside is `begin >= b && end < e`; intersects is
  `begin < e && end >= b`.
- `Aligned`: inside is `begin >= b && end <= e`; intersects is `begin < e && end > b`.

Make `row_inside` / `row_intersects` methods on it, and thread it through
`promote_closed_buckets` and `promote_one`'s re-check and straddle test.
`update_insert_time_jit_view` passes `BlockHull`; the on-demand driver passes `Aligned`.
`promotion_candidates` stays block-only. The on-demand driver builds its targets from units in
step 8.

### 5. Access control

- Rule 3 of `authorize_view_instance` stays `Ok(())`, but its stated basis changes. An
  on-demand view's `'global'` instance *does* trigger JIT, and that is acceptable:
  - Rows are still filtered at read time by `OwnershipRewrite`'s `audience` branch, which
    `check_audience_reachable` makes mandatory for every DDL view.
  - The materialization runs under `CallerContext::maintenance()` and writes global, unfiltered
    partitions. Nothing caller-specific is persisted.
  - Triggering it is a cost concern, not a confidentiality one, and the reader who asks pays.

  Rewrite the doc comment accordingly, and drop the "a future view set … would need to revisit
  this rule" sentence. The same applies to the `'global'` sentence in
  `MaterializedView::instance_is_audience_guarded`'s doc.
- DDL stays admin-gated (`authorize_view_ddl`). Unchanged.
- Admission: no change in v1. JIT already runs inside `create_physical_plan`, before both the
  estimate and the gate, for every JIT view. The estimate then reflects the partitions that
  exist after materialization, which is what the read actually scans. Gating the
  materialization itself would need JIT moved out of `scan()` for all views.

### 6. Read-only connections and retention

- On a read-only connection, `MaterializedView::scan` already turns a JIT write failure into a
  query error naming the view. If every unit is `Covered`, no write is attempted and the query
  succeeds. Promotion skips on read-only. No change.
- `retire_expired_partitions` covers every partition whichever writer produced it. No change.

## Implementation Steps

**Phase 1: definition and schema**

1. `migration.rs`: add `upgrade_v11_to_v12`, which drops `NOT NULL` on `update_group`, and bump
   `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 12.
2. `view_definition.rs`: change to `ViewDefinition.update_group: Option<i32>`;
   `build_sql_batch_view` passes it through.
3. `view_definition_store.rs`: `Option<i32>` in the row and the upsert, with explicit
   `NULLS LAST` ordering in both `ORDER BY`s.
4. `list_view_set_definitions_table_function.rs`: `DdlRow.update_group: Option<i32>`, and stop
   wrapping it in `Some`.
5. `view_ddl.rs`: optional `update_group`; reject the explicit delta options without it; update
   the module and function docs.
6. `log_stats_view.rs` and the tests that build `ViewDefinition`: `Some(..)`. The compiler
   enumerates them.

**Phase 2: validation and registry**

7. `view_definition.rs`:
   - `inspect_scan` and `walk_plan_for_scans` collect on-demand sources (global
     `MaterializedView`, `get_update_group() == None`).
   - `validate_view_definition` rejects a daemon view with any on-demand source and returns the
     source list.
8. `view_registry.rs`: `build_factory` becomes a fixpoint over pending rows and attaches
   `with_on_demand_sources`.
9. `sql_batch_view.rs`: add `on_demand_sources: Vec<Arc<dyn View>>`,
   `with_on_demand_sources`, and `is_on_demand`; derive `Clone` on `SqlBatchView` (the driver
   passes `Arc::new(self.clone())`).

**Phase 3: session range and driver**

10. `SessionRange` (Design §3a). This is a mechanical signature change, and the driver's
    sessions build on it:
    - `query.rs`: define the enum; `make_session_context` and `register_table` take it, and
      `TableScanRewrite` is installed only for `Query`;
    - `view.rs`: `requires_query_range`;
    - `materialized_view.rs`: store the range, and handle `NoPartitions` and `Unbounded` in
      `scan`;
    - every `make_session_context` / `MaterializedView::new` caller picks its variant, including
      `flight_sql_service_impl.rs` (`Query` or `Unbounded`), `view_definition.rs` and
      `SqlBatchView::new` (`NoPartitions`), and `make_batch_partition_spec`, `merge.rs` and
      `export_log_view.rs` (`Snapshot`);
    - `materialize_partitions_table_function.rs` and
      `regenerate_partitions_table_function.rs`: reject on-demand views.
11. `partition_ladder.rs`: factor `plan_bucket` over a count closure and add
    `plan_dense_buckets`.
12. `jit_promotion.rs`: add `RangeConvention`, thread it through `promote_closed_buckets` and
    `promote_one`, and move `row_inside` / `row_intersects` onto it. `jit_partitions.rs` passes
    `BlockHull`.
13. New `on_demand_jit.rs`:
    - pure `plan_units` and `judge_unit`;
    - `update_on_demand_view`, with upstream refresh, snapshot, probe, write with the
      conflict-retry, and promote;
    - metrics `on_demand_jit` count by `outcome = covered | written | conflict_dropped`.
14. `sql_batch_view.rs::jit_update`: dispatch as in Design §3. Register the module in
    `lakehouse/mod.rs`.

**Phase 4: access control docs, docs, changelog**

15. `audience_guard.rs` rule 3 and `materialized_view.rs::instance_is_audience_guarded`: doc
    rewrites only.
16. Documentation and `CHANGELOG.md` (see below).

## Files to Modify

- `rust/analytics/src/lakehouse/migration.rs`
- `rust/analytics/src/lakehouse/view_definition.rs`
- `rust/analytics/src/lakehouse/view_definition_store.rs`
- `rust/analytics/src/lakehouse/view_ddl.rs`
- `rust/analytics/src/lakehouse/view_registry.rs`
- `rust/analytics/src/lakehouse/list_view_set_definitions_table_function.rs`
- `rust/analytics/src/lakehouse/log_stats_view.rs`
- `rust/analytics/src/lakehouse/sql_batch_view.rs`
- `rust/analytics/src/lakehouse/partition_ladder.rs`
- `rust/analytics/src/lakehouse/jit_promotion.rs`
- `rust/analytics/src/lakehouse/jit_partitions.rs` (pass `RangeConvention::BlockHull`)
- `rust/analytics/src/lakehouse/on_demand_jit.rs` (new)
- `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/src/lakehouse/audience_guard.rs` (doc)
- `rust/analytics/src/lakehouse/materialized_view.rs` (unbounded check, doc)
- `rust/analytics/src/lakehouse/view.rs`, `query.rs`, and every `make_session_context` / `MaterializedView::new` caller
  (`metadata.rs`, `merge.rs`, `export_log_view.rs`, `parse_block_table_function.rs`,
  `perfetto_trace_execution_plan.rs`, `process_spans_table_function.rs`,
  `view_instance_table_function.rs`, `ownership_rewrite.rs` sources, and DB and no-DB tests)
- `rust/analytics/src/lakehouse/materialize_partitions_table_function.rs`,
  `regenerate_partitions_table_function.rs`
- `rust/public/src/servers/flight_sql_service_impl.rs`
- Tests: `view_ddl_parse_tests.rs`, `view_definition_validation_tests.rs`,
  `view_registry_tests.rs`, `partition_ladder_tests.rs`, `jit_promotion_tests.rs`, new
  `on_demand_jit_tests.rs`, new `session_range_tests.rs`, plus the `ViewDefinition` constructors the compiler flags.
- `mkdocs/docs/admin/materialized-views.md`, `mkdocs/docs/admin/views-as-code.md`,
  `mkdocs/docs/admin/functions-reference.md`, `CHANGELOG.md`

## Trade-offs

- **Missing `update_group` versus `materialization = 'on_demand'`.** The issue's call. The
  missing group already says it, and the daemon already keys on it.
- **Explicit upstream refresh versus the scan-time `jit_update` inside the extract session.**
  Upstream could be refreshed by running the extract session as `SessionRange::Query(unit)`. But that
  installs `TableScanRewrite` and makes every source's partition fetch prune by *event* time,
  while extracts filter by *insert* time. Rows inserted inside the unit but carrying an earlier
  event time could then be pruned away. It would also make the same definition read sources
  differently under the two writers. The explicit refresh keeps the daemon's
  insert-time-snapshot semantics, at the cost of carrying a dependency list on `SqlBatchView`.
- **Units that grow to contain every intersecting row, versus #1496's `Overlap` retire and
  cascade.** Ladder alignment makes nesting the normal case. Growing the unit makes
  `Containment` sufficient, keeps the verdict a single summed-count comparison, and makes
  verdicts independent, so there is no cascade loop. The cost: a coarse row whose range is
  stale is rewritten whole, never partially. The ladder caps that at one day.
- **Dense planning (no per-minute counts) versus probing at the finest rung.** The authored
  count is a full query. Probing every minute of a 7-day range would mean about 10,000 queries.
  Dense planning probes each emitted bucket once. Empty buckets are written as empty partition
  records (`num_rows = 0`), so later queries see them as `Covered` without running the extract.
- **No object-count descent.** The authored count is in the author's units (rows, blocks…), not
  objects, so the cap has no meaning. Bucket width is bounded by the ladder (at most one day),
  the same as the daemon's default `source_partition_delta`. That cap is not small enough for
  dense sources: #1779.
- **Query range used as the insert range.** On-demand views share the DDL keying, so source rows
  inserted after `query_range.end` are not read for that query, even if their event time is
  inside it. Daemon views pick those up eventually, by covering every insert range. A view over
  late-arriving data (uploads delayed by hours) will miss them on a narrow query. A later query
  whose range covers their insert time picks them up. Accepted for v1; see Decisions.

## Decisions

- Converting a view between daemon-updated and on-demand with `CREATE OR REPLACE` is neither
  rejected nor specially handled. Units grow to contain daemon-written rows, and the daemon's
  freshness check sums JIT rows inside its own buckets, so either direction converges. The issue
  leaves the conversion out of scope, and nothing here needs to block it.
- Rejecting `source_partition_delta` / `merge_partition_delta` on on-demand views is a v1 stopgap.
  With the 24-bucket target, very dense sources get partitions that are too large, and authors
  have no way to cap them. Follow-up: #1779.
- `SessionRange::Snapshot` carries no range. Letting on-demand sources materialize themselves
  over a `Snapshot(insert_range)` would miss the already-taken snapshot (a second, live fetch
  path would be needed). It would also materialize upstream at each unit's finer rung, once per
  probe. The explicit refresh in §2 runs once over the whole aligned range. With no JIT use,
  nothing would read the range: the caller already applied it when building the
  `PartitionCache`.
- An unbounded user query over an on-demand view is an error, not a partial answer (§3a).
  Materializing all of history instead was rejected: the extent of the source is unknown, and
  that first read would be unbounded in cost.
- `materialize_partitions` / `regenerate_partitions` reject on-demand views. A backfill is a query
  over the range, and repairing a range means `retire_partitions` followed by a query. Accepting
  them would bypass the upstream refresh in §2.
- No insert-time look-ahead option in v1; late-arriving rows are picked up by a later query whose
  range covers their insert time; revisit on a concrete need.
- Admission is unchanged in v1: on-demand materialization runs before the gate, as built-in JIT
  does; gating JIT work is a follow-up.

## Documentation

- `mkdocs/docs/admin/materialized-views.md`:
  - `update_group` becomes optional in the options table.
  - New "On-demand views" section: behavior, open-edge grace, chain rule, the rejection of daemon
    views reading on-demand ones, the delta options being rejected, and the late-insert caveat.
  - Fix the "this is the only dependency mechanism" sentence.
- `mkdocs/docs/admin/views-as-code.md`: mention that the `update_group` line can be omitted.
- `mkdocs/docs/admin/functions-reference.md`: in the `list_view_set_definitions()` column table,
  `update_group` is "NULL for an on-demand DDL view or a built-in view set without one".
- `CHANGELOG.md`, Unreleased:
  - the feature;
  - **Minor breaking change**: `ViewDefinition.update_group` / `ViewDefinitionRow.update_group`
    are `Option<i32>`, `validate_view_definition` returns the on-demand source list, and
    `promote_closed_buckets` takes a `RangeConvention`, and `make_session_context` and
    `MaterializedView::new` take a `SessionRange` instead of `Option<TimeRange>`;
  - schema v12.

## Testing Strategy

No-DB unit tests under `rust/analytics/tests/`:

- `view_ddl_parse_tests.rs`:
  - a definition without `update_group` parses to `None`;
  - `source_partition_delta` or `merge_partition_delta` without `update_group` is rejected;
  - an explicit `update_group` is unchanged.
- `view_definition_validation_tests.rs`, over a factory with a fake on-demand `SqlBatchView`
  (lazily-connected pool):
  - a daemon view reading it is rejected;
  - an on-demand view reading it validates and returns it as a source;
  - a daemon view reading a daemon view is unchanged.
- `view_registry_tests.rs`, `build_factory` fixpoint:
  - on-demand `a_view` reading on-demand `b_view` builds in either name order;
  - a two-view cycle reports both as failed.
- `partition_ladder_tests.rs`: `plan_dense_buckets` emits closed buckets at the rung, descends
  only the open edge, and leaves `plan_buckets` output unchanged (existing tests); no emitted bucket begins at or
  after the clamped end.
- `on_demand_jit_tests.rs`, pure `plan_units` / `judge_unit` over canned rows and counts:
  - empty bucket;
  - exact row matching the count, `Covered`;
  - count mismatch, `Stale`;
  - fine rows summing to the count, `Covered`;
  - coarser row grows the unit and covers several planned buckets;
  - misaligned straddling row grows and coalesces neighbors;
  - schema-hash mismatch, `Stale`;
  - touching neighbor rows are not absorbed;
  - promotion eligibility (closed, at rung, at least 2 rows, not grown).
- `session_range_tests.rs`: `MaterializedView::scan` over a fake view that counts its
  `jit_update` calls, using a lazily connected pool:
  - `Unbounded`, with `requires_query_range() == true`: errors, naming the view, and
    `jit_update` is not called;
  - `Unbounded`, with a view that doesn't require a range (built-in): unchanged;
  - `NoPartitions`: an empty plan with the view's schema, no `jit_update`, and no provider
    fetch (a provider that panics on `fetch`);
  - `Snapshot`: `jit_update(None)`, then fetches from the given provider;
  - `Query(r)`: `jit_update(Some(r))`.

  `make_session_context` with `Query` installs `TableScanRewrite`; with the other variants it
  doesn't.

  The `materialize_partitions` and `regenerate_partitions` rejection is checked at the call
  boundary in the same file.
- `jit_promotion_tests.rs`: `RangeConvention::Aligned` inside and intersects at touching edges.
  Existing `BlockHull` cases keep passing.

No new live-DB test: per the project's testing rule, none is added for feature acceptance. The
concurrency paths reuse the already-exercised advisory lock, exclusion constraint and conflict
error. The new decision logic on top of them (`judge_unit` after a refetch) is covered by the
pure tests.

## Manual Verification

These steps check end-to-end wiring (registry, daemon filter, scan-time JIT, real writes) that a
fake cannot reach. A break shows up immediately as empty results or daemon log lines.

1. `python3 local_test_env/ai_scripts/start_services.py`, then generate some telemetry (any
   instrumented test binary).
2. `micromegas-query "CREATE MATERIALIZED VIEW od_log_counts WITH (extract_query = '…', count_src_query = '…', merge_partitions_query = '…', time_column = 'time_bin')"`,
   using the web app's create skeleton minus `update_group` and the delta lines. Expected:
   success.
3. `micromegas-query "SELECT count(*) FROM od_log_counts" --begin 24h`, then
   `micromegas-query "SELECT begin_insert_time, end_insert_time, num_rows, updated FROM list_partitions() WHERE view_set_name = 'od_log_counts' ORDER BY begin_insert_time" --begin 24h`.
   Expected: non-zero count; 1-hour partitions, minute-sized ones at the newest edge.
4. Generate more telemetry and run step 3 again. Expected: only the newest-edge partitions have
   a new `updated` time.
5. Query `--begin 7d`. Expected: 6-hour partitions replace the closed 1-hour ones (promotion).
6. `grep od_log_counts /tmp/daemon.log`. Expected: no materialization lines.
7. `micromegas-query "SELECT count(*) FROM od_log_counts"` with no `--begin`/`--end`. Expected:
   the "needs a query time range" error.
