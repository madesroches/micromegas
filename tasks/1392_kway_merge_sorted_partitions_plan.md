# Order-Preserving K-Way Merge for Non-Temporal Sort Keys Plan

[#1392](https://github.com/madesroches/micromegas/issues/1392) — follow-up to #1340/#1336.

## Overview

### Motivating use case

The concrete driver is an **aggregate metrics view**: a `SqlBatchView` rolling `measures` up into
per-metric, per-time-bucket aggregates, whose useful output sort key is
**`(name, time_bin)`** — metric name first, time second. `name` is the high-cardinality,
non-temporal clustering key: every partition spans the full set of metric names, so partitions
overlap totally on the leading sort column and the #1340 concatenation machinery cannot apply. Its
merges today need a full blocking `SortExec` over the whole range.

`(metric name, time_bin)` is what the design is validated against — the Phase 0 spike uses that
exact shape, including `name` as `Dictionary(Int32, Utf8)` per `metrics_table_schema()`. **The
mechanism itself is fully general**: nothing in `ScanOrdering::PerFile`, `QueryMerger`, or
`SqlBatchView::with_merge_sort_order` knows about metrics, names, or time buckets. Any view whose
partitions are internally sorted on any ascending column list can declare it; `log_stats` on
`(process_id, level, target, time_bin)` would work the same way. The view-author contract
(Design §5) is the only thing a new adopter has to satisfy.

The view groups **strictly by the two declared sort columns** and carries dimension columns through
as aggregates rather than as extra group keys — `first_value(unit)` rather than `GROUP BY ..., unit`:

```sql
-- merge query (the extract query has the same shape over `measures`, with the same ORDER BY)
SELECT name, time_bin, first_value(unit) AS unit, sum(measure) AS total
FROM {source}
GROUP BY name, time_bin
ORDER BY name, time_bin
```

with `.with_merge_sort_order(vec![Arc::new("name".to_string()), Arc::new("time_bin".to_string())])`
(column names, ascending-only — see Design §3). Verified to plan as the fully
streaming `ordering_mode=Sorted` shape of §5 — also with `first_value(unit ORDER BY time_bin)` and
with a fuller measure set (`count`/`min`/`max`/`avg`/`sum`). Two notes on this choice:

- Grouping by an extra key outside the sort order (`GROUP BY name, unit, time_bin`) is *also*
  supported — it degrades to `ordering_mode=PartiallySorted`, which still streams and still bounds
  the working set to the current `(name, time_bin)` prefix. Grouping strictly by the sort columns
  just gets the tighter `Sorted` mode. Either is fine; the spike covers both.
- `first_value` with no explicit inner `ORDER BY` is semantically unpinned (it takes whatever row
  the merge yields first). That is harmless for `unit`, which is functionally dependent on `name`,
  but a column that genuinely varies within a group should use an explicit
  `first_value(x ORDER BY …)` — which the spike confirms costs nothing in plan shape.

### Mechanism

A `SqlBatchView` whose desired output sort key does not lead with the event/insert-time column
cannot use the ordering machinery from #1340: that machinery models the merge source as a single
concatenated sorted stream, which requires partitions to be non-overlapping on the leading sort
column — impossible for a non-temporal, high-value clustering key (e.g. a metric name) that every
partition spans. Merging such views today requires a full blocking `SortExec`, which fails with
`Resources exhausted` on busy days, and `BatchPartitionMerger` only scales the problem (it batches
along the event-time axis while the sort runs along an orthogonal one).

This plan adds a second scan mode — a **per-file declared ordering** — where each already-sorted
partition file becomes its own DataFusion plan partition, all declaring the same `LexOrdering`.
DataFusion then coalesces them with a streaming `SortPreservingMergeExec` (a k-way merge over k
buffered batches) instead of a `SortExec`, and with ordered aggregation and the repartitioning
settings of Design §2 the whole merge query streams end to end. Peak memory scales with k (number of
partitions merged), not data volume. **The Phase 0 spike has been run and the answer is GO** — see
Design §5 for the verified plan shape and the three corrections it produced.

The issue's "Alternative considered" (key-range batching in `BatchPartitionMerger`) was designed
in full and rejected — see the issue comment: it shares most of this plan's prerequisites, adds a
larger delta of new machinery, and still sorts.

## Current State

### The #1340 concatenation mode

- `make_partitioned_execution_plan` (`rust/analytics/src/lakehouse/partitioned_execution_plan.rs:146`)
  puts every non-empty partition file into **one** file group
  (`with_file_groups(vec![file_group.into()])`, line 202). A declared `output_ordering` is only
  valid if the files concatenate in sorted order, so `sort_and_check_non_overlapping` (line 55)
  requires the leading column's per-partition bounds (`OrderingBounds::EventTime` or
  `InsertTime`) to be non-overlapping, and `attach_ordering_statistics` (line 90) attaches those
  bounds as file statistics so DataFusion accepts the multi-file-group ordering.
- `PartitionedTableProvider` (`partitioned_table_provider.rs`) carries
  `output_ordering: Vec<ScanSortColumn>` + `ordering_bounds: OrderingBounds` and threads them into
  the scan. Constructors: `new()` (unordered) and `with_ordering()`.
- `QueryMerger::execute_merge_query` (`merge.rs:93`) has an ordering-declared branch (non-empty
  `merge_scan_ordering`): sets `repartition_file_scans = false`, builds the physical plan once,
  bails if the plan is not single-partition, computes
  `ordering_honored = !plan.contains("SortExec") && !plan.contains("SortPreservingMergeExec")`
  (warn-only, fail-open on memory), and executes that exact plan.
- `View::get_scan_output_ordering()` (`view.rs:165`, returns `Vec<ScanSortColumn>`) declares a
  trusted scan ordering for consumer queries via `MaterializedView::scan`
  (`materialized_view.rs:96`). Only `ThreadSpansView` overrides it.
- `View::get_merged_partition_sort_order()` (`view.rs:134`) computes the recorded
  `lakehouse_partitions.sort_order` for a merge's output, from the inputs alone.
- `BlocksView` (`blocks_view.rs:40-45, 194-229`) is the gating precedent: it holds an
  `ordered_merger` and a `plain_merger`, picks the ordered one only when at least one input is
  non-empty **and** every input is empty or carries the exact recorded
  `sort_order == ["insert_time"]`, and records the guarantee under the same gate.

### SqlBatchView records and declares nothing

- `SqlBatchView` (`sql_batch_view.rs`) overrides neither `get_merged_partition_sort_order` nor
  `get_scan_output_ordering`. Its merger is built at construction (`sql_batch_view.rs:104-112`) —
  the default `QueryMerger` with no declared ordering, or a custom one via `merger_maker`
  (how downstream deployments plug in `BatchPartitionMerger`).
- `SqlPartitionSpec::write` (`sql_partition_spec.rs:98`) hardcodes `sort_order: None` on the
  fresh-partition write path, so even a view whose extract query has a top-level `ORDER BY`
  records `NULL`.
- `Partition::sort_order`, the `regenerate_partitions()` admin UDF, and the v7 schema column all
  already exist from #1340 — this plan reuses them unchanged.

### Adjacent pruning limitations (issue "Related")

- `WriterProperties` in `write_partition.rs:666-672` never sets `max_row_group_size`, so it
  defaults to 1,048,576 rows — a partition is a handful of row groups, making clustering-based
  row-group pruning very coarse.
- `datafusion.execution.parquet.enable_page_index` is `false` in `make_session_context`
  (`query.rs:219`) for legacy-file compatibility, while the writer emits
  `EnabledStatistics::Page` — page statistics are written and never read.

## Design

### 1. `ScanOrdering` enum — a per-file mode alongside concatenation

Replace the `(output_ordering: &[ScanSortColumn], ordering_bounds: OrderingBounds)` parameter pair
with one enum in `partitioned_execution_plan.rs`:

```rust
/// How a partition scan's declared output ordering is realized.
#[derive(Clone, Debug)]
pub enum ScanOrdering {
    /// No declared ordering (today's default).
    Unordered,
    /// All files form one sequential file group that concatenates in globally-sorted order.
    /// Requires non-overlapping leading-column bounds (checked against `bounds`).
    Concatenated {
        columns: Vec<ScanSortColumn>,
        bounds: OrderingBounds,
    },
    /// Each file is internally sorted by `columns`; files may overlap arbitrarily. The scan
    /// yields one ordered plan partition per file, for a downstream SortPreservingMergeExec.
    PerFile { columns: Vec<ScanSortColumn> },
}
```

`make_partitioned_execution_plan(schema, reader_factory, state, projection, filters, limit,
partitions, scan_ordering: &ScanOrdering)`:

- `Unordered` / `Concatenated`: byte-for-byte today's behavior (`Concatenated` runs
  `sort_and_check_non_overlapping` + `attach_ordering_statistics` exactly as now).
- `PerFile`: one file group **per** non-empty file (`with_file_groups(file_groups)`), the same
  `LexOrdering` from `make_lex_ordering` declared via `with_output_ordering`. No overlap check, no
  ordering statistics — DataFusion's multi-file-group ordering validation only needs min/max stats
  to prove cross-file order *within* a group; single-file groups pass trivially (confirmed by the
  Phase 0 spike).
- **Recorded-sort_order gate, in `PerFile` mode only**: before declaring anything, check that
  every non-empty partition's recorded `Partition::sort_order` *certifies* the declared columns —
  the declared column names are a prefix of the recorded list and all declared columns are
  ascending (recorded `sort_order` is ascending-implied, so a descending `ScanSortColumn` can
  never be certified). If any non-empty partition fails, degrade to an unordered scan (same plan
  shape as `Unordered`). Centralizing the gate here makes **both** consumers safe during rollout:
  user-query scans via `MaterializedView` and merge scans see pre-rollout unsorted partitions and
  silently fall back to sorting, with no per-caller logic. Add the certification helper as a
  method on `Partition` (`partition.rs`), e.g.
  `pub fn certifies_sort_order(&self, columns: &[ScanSortColumn]) -> bool`
  (empty partitions certify vacuously).

`PartitionedTableProvider` holds a single `scan_ordering: ScanOrdering` field; `new()` keeps
`Unordered`, and `with_ordering(...)` becomes
`with_scan_ordering(schema, reader_factory, partitions, ScanOrdering)`.

`View::get_scan_output_ordering()` changes to return `ScanOrdering` (default `Unordered`).
`ThreadSpansView` returns `Concatenated { columns, bounds: OrderingBounds::EventTime }`;
`MaterializedView::scan` passes the value through unchanged. This is a breaking trait change with
exactly one in-repo overrider — see Trade-offs.

### 2. QueryMerger: per-file merge branch

`QueryMerger::merge_scan_ordering` becomes a `ScanOrdering` (default `Unordered`);
`with_merge_scan_ordering` takes `ScanOrdering` directly. `BlocksView` and its tests update to
pass `Concatenated { .., bounds: InsertTime }` — no behavior change.

`execute_merge_query` gains a third branch for `PerFile` (the `Unordered` and `Concatenated`
branches keep today's exact behavior):

- Session config, set the same way the existing branch sets `repartition_file_scans`
  (`ctx.state_ref().write().config_mut().options_mut()`). All five settings were exercised as a
  matrix in the Phase 0 spike (§5); the notes below record which are load-bearing and why:
  - `optimizer.repartition_file_scans = false` — keep one plan partition per file group. Verified
    load-bearing: left at its default, DataFusion byte-range-splits the k files into
    `target_partitions` groups (`part_0.parquet:0..146`, …), multiplying open readers and
    partial-aggregate working sets for no benefit on an I/O-bound serial merge.
  - `optimizer.enable_round_robin_repartition = false` — **added after the Phase 0 spike.** The
    other four settings do not prevent `EnforceDistribution` from inserting an order-preserving
    `RepartitionExec: partitioning=RoundRobinBatch(target_partitions), preserve_order=true`
    between the scan and the partial aggregate. The plan still streams and is still correct, but
    the `SortPreservingMergeExec` above it then merges `target_partitions` streams instead of k,
    with that many partial-aggregate working sets — peak memory would scale with
    `target_partitions` rather than with k, defeating §4. With this setting, the observed plan is
    exactly the intended shape (see §5).
  - `optimizer.repartition_aggregations = false` — feeds a single-partition `AggregateExec`
    directly from the `SortPreservingMergeExec`. Verified load-bearing for the *shape*, not for
    correctness: left at its default the plan becomes
    `SPM → AggregateExec(FinalPartitioned) → RepartitionExec(Hash([keys], target_partitions),
    preserve_order=true) → AggregateExec(Partial)`, which still streams with
    `ordering_mode=Sorted` but fans the aggregate out again and moves the SPM above the aggregate.
    Keeping it `false` gives the simpler, predictable shape §4 reasons about.
  - `optimizer.prefer_existing_sort = true` — belt and braces. Verified **not** required for the
    streaming shape when the merge query carries the matching top-level `ORDER BY` (that `ORDER BY`
    alone forces DataFusion to pick order-preserving variants over re-sorting). It is what keeps
    the plan ordered when the `ORDER BY` is *absent* — which is why check 2 below cannot be relied
    on to detect a missing `ORDER BY`.
  - `optimizer.repartition_joins = false` — keeps any enrichment join in `CollectLeft` mode rather
    than hash-partitioning it. **This is necessary but not sufficient**: `CollectLeft` buffers its
    *left* (build) input and takes its output ordering from the *right* (probe) input, so the
    ordered stream must be the right-hand side. See the authoring constraint in §5.
- Build the physical plan once, then three checks before executing that exact plan (mirroring the
  existing branch's build-once-execute-same-plan discipline):
  1. **Hard**: `partition_count == 1`, same bail as today — `execute_stream` would otherwise
     coalesce and destroy the order.
  2. **Hard**: the plan's output ordering must satisfy the declared columns
     (`plan.properties().equivalence_properties().ordering_satisfy(...)` against the
     `make_lex_ordering` result — make that helper `pub`). Failure means the `sort_order` that
     `get_merged_partition_sort_order` is about to record would be a lie. That is a view
     configuration bug: fail loudly before writing anything. (`BlocksView` never needed this — its
     merge query is a fixed internal string; `SqlBatchView` merge queries are author-supplied.)

     The Phase 0 spike sharpened what this check does and does not prove. It validates the *actual
     plan's* output ordering, which is exactly what makes the recorded `sort_order` truthful — so
     it is sound. But it is **not** a detector for a missing top-level `ORDER BY`: with
     `prefer_existing_sort = true`, a merge query with no `ORDER BY` at all still planned to the
     fully ordered streaming shape, so check 2 passes. The matching `ORDER BY` therefore stays a
     documented authoring requirement (it is what makes the ordering a property of the query rather
     than of an optimizer preference); check 2 is the runtime backstop that fails closed if a
     future DataFusion version stops volunteering the ordering.
  3. **Warn-only**: `ordering_honored = !plan_str.contains("SortExec")`. A surviving `SortExec`
     means the streaming shape regressed (memory, not correctness — check 2 already proved the
     output order). Unlike the `Concatenated` branch, a `SortPreservingMergeExec` is the
     *expected* operator here, never a failure — note `"SortPreservingMergeExec"` does not contain
     the substring `"SortExec"`, so the check reads naturally. A single-non-empty-file merge may
     legitimately contain neither operator.

### 3. SqlBatchView: declare, gate, and record

Mirror the `BlocksView` dual-merger pattern, configured per view:

- New builder method (avoids growing the already-`too_many_arguments` constructor):

  ```rust
  /// Declares that this view's partitions are internally sorted, ascending, by `columns` in
  /// order: the extract query and the merge query must both end with a matching top-level
  /// ORDER BY. A CTE-internal ORDER BY that is later joined does NOT count -- the join
  /// discards it.
  pub fn with_merge_sort_order(mut self, columns: Vec<Arc<String>>) -> Self
  ```

  Column names only — there is no descending option. `ScanSortColumn` (`view.rs:42-45`) carries a
  `descending` flag, but a descending column can never be certified (Design §1: recorded
  `sort_order` is ascending-implied), so an API that accepted one would silently disable the whole
  feature on a one-field typo. Taking plain column names makes that mistake unrepresentable instead
  of relying on a runtime check.

  It stores `sort_order: Option<Vec<Arc<String>>>` and builds an
  `ordered_merger: Option<Arc<dyn PartitionMerger>>` — a `QueryMerger` over the view's
  `merge_partitions_query` with `ScanOrdering::PerFile { columns }`, converting each name to
  `ScanSortColumn { column, descending: false }`. The existing `merger` field
  (default `QueryMerger` or the `merger_maker` product, e.g. `BatchPartitionMerger`) is untouched
  and becomes the fallback. Keeping a custom `merger_maker` alongside `with_merge_sort_order` is
  deliberate and supported: during rollout, merges over not-yet-regenerated inputs still get the
  bounded batching merger instead of an unbounded blocking sort (see Rollout).

- `merge_partitions` override changes: pick `ordered_merger` only when it is configured **and** at
  least one input is non-empty **and** every input certifies the declared columns
  (`Partition::certifies_sort_order`); otherwise use `self.merger`. The any-non-empty condition
  matches `blocks_view.rs:201-204`: an all-empty source scans as `EmptyExec`, whose `SortExec` is
  never elided, and would trip the memory-regression warning on every quiet-day retry.

- `get_merged_partition_sort_order` override: when configured and every input certifies the
  declared columns → `Some(column names)` (all-empty merges included — vacuously true of the
  empty output, matching the blocks precedent); otherwise `None`.

- `get_scan_output_ordering` override: `ScanOrdering::PerFile { columns }` when configured. Safe
  for arbitrary user queries because of the recorded-sort_order gate inside
  `make_partitioned_execution_plan` (Design §1); user sessions get no optimizer-config changes —
  the declaration is opportunistic information DataFusion may exploit (e.g. row-group pruning on
  the leading column, order-aware aggregation), never a correctness risk. It does have a resource
  consequence, though: `FileGroupPartitioner::repartition_preserving_order`
  (`datafusion-datasource` 54.1, `rust/Cargo.toml:50`) only declines to repartition (returns `None`)
  once the file-group count reaches `target_partitions`; below that threshold it splits each
  single-file group into byte-range groups to fill out `target_partitions`. So on the user path
  scan concurrency (open Parquet readers, plan partitions) is `max(k, target_partitions)`, and the
  partial aggregate (`ordering_mode=Sorted`) runs with `target_partitions` working sets rather than
  k — confirmed empirically with a throwaway planning test replicating
  `SqlBatchView::register_table` in a default (user) session: 3 declared per-file groups became
  `target_partitions` byte-range groups under a `RepartitionExec(Hash(…), preserve_order=true)`.
  The declaration still improves user-path memory relative to today, though: each `Sorted` partial
  holds one group's prefix instead of every group's, also confirmed by the probe. k itself is
  bounded by query range, not by rollup cadence (see Trade-offs), so a wide-range query against a
  view that has not rolled up recently is where k can exceed `target_partitions`. Accepted here for
  the same no-hard-limits reason as the merge path's k.

- **Fresh-write recording**: `SqlPartitionSpec` gains a `sort_order: Option<Vec<String>>` field,
  threaded through `fetch_sql_partition_spec` (new parameter) into `write_partition_from_rows`
  (replacing the hardcoded `None` at `sql_partition_spec.rs:98`). `SqlBatchView` passes its
  configured column names; `ExportLogView` (`export_log_view.rs:191`), the other caller of
  `fetch_sql_partition_spec`, passes `None` — unchanged behavior. To prevent a config typo from
  recording a false guarantee,
  `SqlPartitionSpec::write` — only when `sort_order` is declared — builds the extract plan once
  (`df.create_physical_plan()`), bails unless the plan is single-partition and its output ordering
  satisfies the declared columns (same checks as Design §2's merge branch; a global top-level
  `ORDER BY` guarantees both), then executes that exact plan via
  `datafusion::physical_plan::execute_stream`. The undeclared path keeps today's
  `df.execute_stream()` untouched.

  This does not make the extract path stream, though: the required top-level `ORDER BY` sorts
  `SqlBatchView`'s extract query the same way it always has — an in-process, blocking `SortExec`
  (unlike `BlocksView`, whose fresh-write ordering is a Postgres `ORDER BY` streamed through
  `MetadataPartitionSpec`). Its input is the *already-aggregated* extract output (post-`GROUP BY`),
  not the raw source, but its memory cost still scales with the extract's insert-range bucket —
  i.e. with whatever range a single fresh-write or `regenerate_partitions()` call covers, not with
  k. Adopters must size that bucket accordingly; see Rollout step 2.

### 4. Memory expectation (stated precisely, per the issue comment)

With the scan sorted on `(key, time)` and both among the `GROUP BY` keys, the aggregate holds open
only the group combinations for the current `(key, time)` prefix value. In the verified plan shape
(§5) the partial aggregate runs in the k scan partitions and the final aggregate is
single-partition, so peak = k partial working sets + one final working set + k buffered batches in
the `SortPreservingMergeExec` + k open Parquet readers. It scales with k, not data volume; the
daemon's incremental rollup (seconds → minutes → hours → day) keeps k modest per merge. No cap on k
is introduced — the design needs no ceiling.

**Correction from the Phase 0 spike**: the parenthetical that enrichment joins are free because
they "sit downstream of the aggregate in `CollectLeft` mode" was wrong. `CollectLeft` buffers the
*left* input, so a merge query phrased the natural way — `(<ordered aggregate>) a LEFT JOIN dim d`
— collects the **entire aggregate result** into memory and re-sorts above the join, reintroducing
the exact blowup this plan exists to remove. A merge query that enriches must put the dimension
table on the left and the ordered aggregate on the right (§5). Views whose merge query has no join
(e.g. `log_stats`) are unaffected.

### 5. Go/no-go spike (Phase 0) — **DONE: GO**

The one genuine unknown was whether DataFusion 54.1's `AggregateExec` keeps the plan streaming — no
`SortExec` reinserted downstream of the `SortPreservingMergeExec` — when its input is ordered per
file. **It does.** The spike lives in `rust/analytics/tests/ordered_aggregation_spike_tests.rs`
(7 planning-only tests, no DB and no object store: a test-local `TableProvider` emits one
single-file group per fabricated file, each declaring `LexOrdering (name, time_bin)`, and nothing is
ever executed). All 7 pass under `cargo test`, `cargo fmt` and
`cargo clippy --tests -- -D warnings` are clean, and the tests stay as the permanent regression
guard. **Proceed to Phase 1**, with the three corrections recorded below folded in.

Observed plan for the aggregate-metrics merge query of the Overview
(`SELECT name, time_bin, first_value(unit) AS unit, sum(measure) AS total FROM source GROUP BY name,
time_bin ORDER BY name, time_bin`) over 3 overlapping per-file partitions, with the five settings
of §2:

```
ProjectionExec: expr=[name, time_bin, first_value(source.unit) as unit, sum(source.measure) as total]
  AggregateExec: mode=Final, gby=[name, time_bin],
                 aggr=[first_value(unit), sum(measure)], ordering_mode=Sorted
    SortPreservingMergeExec: [name ASC NULLS LAST, time_bin ASC NULLS LAST]
      AggregateExec: mode=Partial, gby=[name, time_bin],
                     aggr=[first_value(unit), sum(measure)], ordering_mode=Sorted
        DataSourceExec: file_groups={3 groups: [[part_0.parquet], [part_1.parquet], [part_2.parquet]]},
                        output_ordering=[name ASC NULLS LAST, time_bin ASC NULLS LAST]
```

Single-partition output, streaming k-way merge, `ordering_mode=Sorted` on both aggregate phases, no
`SortExec`, no `RepartitionExec`. Confirmed along the way:

- **Single-file groups need no statistics.** The declared ordering is accepted with no per-file
  min/max stats attached at all, as Design §1 assumed — `attach_ordering_statistics` is only
  needed by `Concatenated` mode.
- **A `Dictionary(Int32, Utf8)` leading sort column plans identically** to a plain `Utf8` one, so the
  real `measures.name` type is not a special case. The spike schema uses the dictionary type.
- **`"SortPreservingMergeExec"` does not contain the substring `"SortExec"`**, so §2's check 3 reads
  naturally (the spike asserts `spm == true` and `contains("SortExec") == false` simultaneously).
- **`ordering_mode=Sorted` is driven purely by the declared scan ordering plus the `GROUP BY` keys**,
  not by any of the five settings — it appeared in every config combination tested. The settings
  control how much the merge fans out, not whether ordered aggregation happens.
- **`GROUP BY` key order does not matter** (`GROUP BY time_bin, name` still streams), and a strict
  prefix of the declared columns (`GROUP BY name`) streams too.
- **Order-sensitive aggregates are free**: `first_value(unit)` and `first_value(unit ORDER BY
  time_bin)` both keep `ordering_mode=Sorted`, as does a fuller measure set
  (`count`/`min`/`max`/`avg`/`sum`).
- **An extra `GROUP BY` key outside the sort order degrades gracefully**, not catastrophically:
  `GROUP BY name, unit, time_bin` under a `(name, time_bin)` declaration gives
  `ordering_mode=PartiallySorted([0, 2])` — still streaming, still bounded to the current
  `(name, time_bin)` prefix, no `SortExec`.

Three corrections to the design, all folded into §2 and §4 above:

1. **A fifth setting is required**: `optimizer.enable_round_robin_repartition = false`. Without it
   an order-preserving `RoundRobinBatch(target_partitions)` repartition lands between the scan and
   the partial aggregate, making peak memory scale with `target_partitions` instead of k (§2).
2. **Check 2 does not detect a missing top-level `ORDER BY`** — with `prefer_existing_sort = true`
   the ordering-free query planned to the same ordered streaming shape, so the check passes. It is
   still sound as a guard on the recorded `sort_order` (§2, check 2), but the `ORDER BY` is enforced
   by documentation and review, not by the runtime check. The Testing Strategy item that expected a
   hard error here has been corrected.
3. **Enrichment joins must put the ordered stream on the probe side** (§4). `CollectLeft` buffers
   its left input and inherits its output ordering from its right input, so:

   | Merge query phrasing | Result |
   |---|---|
   | `(<ordered agg>) a LEFT JOIN dim d` | `CollectLeft` buffers the whole aggregate, blocking `SortExec` above the join |
   | `(<ordered agg>) a JOIN dim d` | same |
   | `dim d JOIN (<ordered agg>) a` | streams, no `SortExec` |
   | `dim d RIGHT JOIN (<ordered agg>) a` | streams, no `SortExec` |

**View-author contract** — the requirements this encodes, all covered by spike tests. This is the
list `with_merge_sort_order`'s rustdoc must carry:

1. **The declared sort columns must be a prefix subset of the merge query's `GROUP BY` keys.**
   Verified negative: declaring `(name, time_bin)` and grouping by `time_bin` alone loses
   `GroupOrdering` entirely and reinstates a blocking `SortExec`. Key order within `GROUP BY` is
   irrelevant; extra keys are tolerated (`PartiallySorted`).
2. **The merge query's top-level `ORDER BY` must be *exactly* the declared columns — not a
   superset.** This is the sharpest trap found, and it was understated as "must match" before the
   spike. `ORDER BY name, time_bin, unit` under a `(name, time_bin)` declaration asks for an
   ordering the scan cannot satisfy, so DataFusion buffers the entire aggregate result and sorts it
   — the exact blowup this plan removes. Note the recorded `sort_order` stays *truthful* (the output
   really is sorted by `(name, time_bin)`), so §2 check 2 passes and only the warn-only
   `ordering_honored` check catches it. The fix is either to drop the trailing column from the
   `ORDER BY` or to declare the longer sort order.
3. **Any enrichment join must put the dimension table on the left and the ordered stream on the
   right** (correction 3).
4. **Both the extract and the merge query need the `ORDER BY`** — and it must be top-level: a
   CTE-internal `ORDER BY` that is later joined does not count, because the join discards it. Unlike
   the merge query, the extract query's `ORDER BY` is a blocking sort (Design §3): size the
   extract/regeneration bucket accordingly, it is not streamed the way the merge path is.

### 6. Row-group size (separable)

Set `max_row_group_size` in `write_partition.rs`'s `WriterProperties` so a clustered sort key
actually pays off in row-group pruning. Proposed: 128 Ki rows (8× finer pruning than the 1 Mi
default; row-group metadata overhead stays negligible at partition scale). This is a one-line
change with fleet-wide effect on newly written partitions (old partitions are unaffected until
merged/regenerated); it is independent of everything above and can ship as its own commit or PR.

Re-enabling `enable_page_index` is **not** in this plan: it is gated on legacy Parquet files
(incomplete ColumnIndex metadata) aging out of every deployment, which is an operational condition
this repo can't verify — file it as a follow-up issue referencing `query.rs:215-219`.

## Implementation Steps

### Phase 0 — Spike (go/no-go) — **DONE, outcome: GO**
1. ~~New test file `rust/analytics/tests/ordered_aggregation_spike_tests.rs` (planning-only, Design
   §5).~~ Landed with 7 passing tests; verdict and the three resulting design corrections are in
   Design §5.

### Phase 1 — `ScanOrdering` refactor (behavior-preserving)
2. `partitioned_execution_plan.rs`: add `ScanOrdering`, restructure
   `make_partitioned_execution_plan` around it, implement the `PerFile` file-group construction
   and the recorded-sort_order degrade gate; make `make_lex_ordering` `pub`.
3. `partition.rs`: add `Partition::certifies_sort_order`.
4. `partitioned_table_provider.rs`: replace the field pair with `ScanOrdering`;
   `with_ordering` → `with_scan_ordering`.
5. `view.rs`: `get_scan_output_ordering() -> ScanOrdering` (default `Unordered`); update the
   trait doc to describe both non-trivial modes and their contracts.
6. `merge.rs`, `blocks_view.rs`, `thread_spans_view.rs`, `materialized_view.rs`: adapt call sites
   (no behavior change) — `merge.rs`: `merge_scan_ordering` becomes `ScanOrdering`,
   `with_merge_scan_ordering` takes `ScanOrdering` directly, and its `src_table` construction
   switches to `with_scan_ordering`; `blocks_view.rs`: pass
   `ScanOrdering::Concatenated { .., bounds: InsertTime }` at the `with_merge_scan_ordering` call
   site. Both are mechanical, signature-only updates — the new `PerFile` merge branch itself is
   added in Phase 2.
7. Update direct-caller tests: `thread_spans_ordering_tests.rs`,
   `blocks_view_merge_ordering_tests.rs` (signature updates only; these tests keep exercising
   `Concatenated` behavior until Phase 2 adds `PerFile` coverage).

### Phase 2 — QueryMerger per-file branch
8. `merge.rs`: add the `PerFile` branch — the **five** config settings + three checks (Design §2).
9. Planning tests for the per-file branch (see Testing Strategy).

### Phase 3 — SqlBatchView declaration and recording
10. `sql_partition_spec.rs`: `sort_order` field + `fetch_sql_partition_spec` parameter + declared
    -path plan verification in `write` (Design §3). `export_log_view.rs`: update its
    `fetch_sql_partition_spec` call site to pass `None` (mechanical, no behavior change).
11. `sql_batch_view.rs`: `with_merge_sort_order`, dual-merger selection in `merge_partitions`,
    `get_merged_partition_sort_order` and `get_scan_output_ordering` overrides.
12. Unit + planning tests for the gates and overrides.

### Phase 4 — Row-group size (separable, can land any time)
13. `write_partition.rs`: `.set_max_row_group_size(...)` (Design §6, pending Open Question 2).

## Files to Modify

| File | Change |
|---|---|
| `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` | `ScanOrdering` enum, per-file mode, degrade gate, `pub make_lex_ordering` |
| `rust/analytics/src/lakehouse/partition.rs` | `certifies_sort_order` |
| `rust/analytics/src/lakehouse/partitioned_table_provider.rs` | `ScanOrdering` field/constructor |
| `rust/analytics/src/lakehouse/view.rs` | `get_scan_output_ordering` returns `ScanOrdering` |
| `rust/analytics/src/lakehouse/materialized_view.rs` | pass-through |
| `rust/analytics/src/lakehouse/thread_spans_view.rs` | return `Concatenated` |
| `rust/analytics/src/lakehouse/merge.rs` | per-file branch in `QueryMerger` |
| `rust/analytics/src/lakehouse/blocks_view.rs` | call-site update |
| `rust/analytics/src/lakehouse/sql_batch_view.rs` | config, gating, overrides |
| `rust/analytics/src/lakehouse/sql_partition_spec.rs` | `sort_order` recording + verification |
| `rust/analytics/src/lakehouse/export_log_view.rs` | `fetch_sql_partition_spec` call site passes `None` |
| `rust/analytics/src/lakehouse/write_partition.rs` | `max_row_group_size` (Phase 4) |
| `rust/analytics/tests/thread_spans_ordering_tests.rs` | signature updates |
| `rust/analytics/tests/blocks_view_merge_ordering_tests.rs` | signature updates |
| `rust/analytics/tests/ordered_aggregation_spike_tests.rs` | new (Phase 0) — **done** |
| `rust/analytics/tests/sql_batch_view_merge_ordering_tests.rs` | new (Phases 2–3) |

## Trade-offs

- **Per-file k-way merge vs key-range batching**: evaluated in full and rejected in the issue
  comment. Both need sorted sources, recorded `sort_order` on both write paths, per-view sort-key
  decisions, and a regenerate-before-trusting rollout; key-range batching's remaining delta
  (distribution pre-query, greedy packing, oversized-key sub-slicing, five silent per-view
  contract hazards) is larger than this plan's, and it still sorts.
- **One `ScanOrdering` enum vs adding more parameters**: the `(columns, bounds)` pair already only
  makes sense together, and per-file mode has no `bounds` at all — an enum makes invalid
  combinations unrepresentable. It breaks `View::get_scan_output_ordering` for out-of-repo
  implementors, but the migration is mechanical (`vec![] → Unordered`, wrap existing vectors in
  `Concatenated`), and the alternative (a parallel `get_scan_ordering_mode()` getter that must be
  kept consistent with the first) is a standing correctness trap.
- **Certification gate inside `make_partitioned_execution_plan` vs per-caller**: the scan builder
  is the one place that sees both the declaration and the `Partition` records, and it serves both
  the merge path and user queries. `BlocksView`'s merge-path gate stays where it is (it must pick
  a merger *and* a recorded value before planning); its exact-equality check is a special case of
  the new prefix-based helper but is deliberately left untouched.
- **Hard-fail on unsatisfied output ordering vs warn**: mirrors #1340's split — memory regressions
  fail open (warn + execute), correctness hazards fail closed. A merge query without the matching
  `ORDER BY` would record a false `sort_order` that every *future* ordered merge and user-query
  scan would then trust; that must never write a partition.
- **Serial single-partition aggregation**: accepted — the merge is I/O bound and the alternative
  is spilling or failing (issue Proposal §2). The Phase 0 spike showed this is milder than feared:
  only the *final* aggregate is single-partition, while the partial aggregate still runs k-way in
  parallel (one per file partition, below the `SortPreservingMergeExec`). Disabling round-robin
  repartitioning (Design §2) caps that parallelism at k rather than `target_partitions`, which is
  the intended trade: bounded memory over extra CPU fan-out on an I/O-bound path.
- **No ceiling on k**: consistent with the project's no-hard-limits stance. On the merge path the
  rollup cadence bounds k in practice, and Design §2's disabled round-robin repartitioning keeps
  scan/partial-aggregate concurrency at k rather than `target_partitions`. On the user-query path
  (Design §3's `get_scan_output_ordering` override) k is instead the number of partitions
  overlapping the requested range — not bounded by rollup cadence, since a query can span
  partitions that have not yet rolled up — but there concurrency is `max(k, target_partitions)`,
  since DataFusion still repartitions per-file groups up to `target_partitions` on that path.
  Either way a pathological k degrades gradually (more open readers, more plan partitions), not
  with a failure cliff, and ordering correctness is unaffected.

## Documentation

- Rustdoc on `ScanOrdering`, `with_merge_sort_order`, `certifies_sort_order`, and the updated
  `View::get_scan_output_ordering` contract carry the authoritative contracts. `with_merge_sort_order`
  is the right home for the full view-author checklist, all three items now verified by the Phase 0
  spike: (a) declared sort columns must be a prefix subset of the merge query's `GROUP BY` keys;
  (b) both the extract and merge queries need a matching top-level `ORDER BY` — a CTE-internal
  `ORDER BY` that is later joined does not count; (c) an enrichment join must put the dimension
  table on the left and the ordered aggregate on the right, or `CollectLeft` buffers the whole
  aggregate and re-sorts (Design §5 correction 3).
- `mkdocs/docs/admin/functions-reference.md`: extend the `sort_order` column description and the
  `regenerate_partitions()` note (line 119) to mention `SqlBatchView` partitions as a second
  regeneration use case (currently blocks-specific).
- No architecture page currently documents partition merging; not adding one in this plan.

## Testing Strategy

All new tests are planning-only against the offline harness
(`blocks_view_merge_ordering_tests.rs:239` — in-memory object store, lazy PG pool, streams never
polled), so they run in plain `cargo test`:

1. **Spike/regression** (Phase 0) — **done**, `ordered_aggregation_spike_tests.rs`: ordered
   `GROUP BY` merge plan over per-file source → single-partition, `SortPreservingMergeExec` present,
   `ordering_mode=Sorted`, no `SortExec`, no `RepartitionExec`. Plus negative controls: undeclared
   scan still sorts; default `enable_round_robin_repartition` fans out; non-prefix `GROUP BY` loses
   `GroupOrdering`; k == 1 needs neither operator; and both enrichment-join directions.
2. **Per-file scan** (Phase 1): overlapping partitions with certifying `sort_order` → k plan
   partitions each declaring the ordering; one partition with `sort_order: NULL` → degraded,
   unordered plan (gate); descending declared column → degraded (ascending-only certification);
   declared columns a strict prefix of recorded → certified.
3. **QueryMerger per-file branch** (Phase 2): `ordering_honored: true` for overlapping sorted
   inputs; merge query with a contradicting `ORDER BY ... DESC` → hard error (check 2);
   single-non-empty-file merge → `ordering_honored: true` with neither operator; `SELECT` shape that
   defeats streaming → merge succeeds with `ordering_honored: false` (fail-open, mirroring
   `defeated_elision_reports_ordering_not_honored_without_erroring`) — use the build-side
   enrichment join from Design §5 correction 3 as that shape, since it is a realistic authoring
   mistake. Do **not** assert that a merge query *missing* the `ORDER BY` hard-errors: the Phase 0
   spike showed DataFusion still plans it ordered, so check 2 passes. A test asserting a
   no-`ORDER BY` query is accepted (and correctly reports `ordering_honored: true`) is the accurate
   version, and pins the behavior for future DataFusion upgrades.
4. **SqlBatchView gates** (Phase 3): `get_merged_partition_sort_order` and merger selection across
   the input matrix (all certified / one uncertified / all empty / mixed); `SqlPartitionSpec`
   declared-path verification bails when the extract plan doesn't guarantee the order.
5. **End-to-end (manual, local test env)**: start services, define a test `SqlBatchView` sorted on
   a non-temporal key, ingest, let the daemon merge, and verify via
   `micromegas-query "SELECT ... FROM list_partitions()"` that merged partitions record the
   `sort_order`, and via the analytics log that `ordering_honored` held.

## Rollout

Landing this plan changes no existing view's behavior (every current view is `Unordered`). For a
view adopting a sort order (in-repo or downstream):

1. Add the top-level `ORDER BY` to the extract and merge queries and call
   `with_merge_sort_order`. New fresh partitions immediately record `sort_order`; merges keep
   using the fallback merger (the gate sees uncertified old inputs) — keep any existing
   `BatchPartitionMerger` `merger_maker` in place as that fallback.
2. Regenerate live partitions with the existing `regenerate_partitions()` UDF (range/delta
   alignment rules per `functions-reference.md`). Each regenerated partition re-runs the extract
   query's blocking `ORDER BY` over that partition's regeneration bucket (Design §3) — size the
   bucket passed to `regenerate_partitions()` for that cost; regenerating an already-merged,
   day-sized partition sorts a full day's worth of the (already-aggregated) extract output in one
   shot.
3. Once every live partition certifies, ordered k-way merges engage automatically (per-merge input
   gate — no flag day). `sort_order` in `list_partitions()` makes progress auditable in SQL.
4. Drop the custom `merger_maker` once the view's partitions are fully regenerated.

## Open Questions

1. ~~**Phase 0 spike outcome**~~ — **answered: GO.** DataFusion 54.1 plans the ordered per-file
   `GROUP BY` merge as a fully streaming pipeline. Three design corrections resulted (a fifth
   optimizer setting, the limits of check 2, and the enrichment-join direction); all are folded into
   Design §2/§4/§5. Nothing blocks Phase 1.
2. **`max_row_group_size` value** (Phase 4): 128 Ki rows proposed — acceptable, or prefer a
   different value / a per-view knob / deferral to a separate PR?
3. ~~**Where does the aggregate metrics view live?**~~ — **answered: outside this repo.** It's a
   deployment-specific `SqlBatchView`, not an in-repo one — `metrics_view` stays the raw block-based
   `measures` view, and the only in-repo `SqlBatchView`s remain `log_stats`, `processes`, and
   `streams`, none of which need to adopt a sort order. This plan lands only the general mechanism
   (`ScanOrdering::PerFile`, the `QueryMerger` branch, `SqlBatchView::with_merge_sort_order`); the
   aggregate metrics rollup itself is configured downstream, in that deployment's view definitions.
   Phases 0–3's tests validate the mechanism against the same `(name, time_bin)` shape without
   requiring an in-repo end-to-end user; item 5 of the Testing Strategy's manual end-to-end pass uses
   a throwaway test view for the same reason.
