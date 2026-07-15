# Perfetto Trace Export — Eliminate Redundant Per-Thread Sort Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1297

## Overview

Exporting a Perfetto trace for a process with many threads over a wide time range can OOM the
flight-sql server. `generate_thread_spans_with_writer` runs one `SELECT ... ORDER BY begin` query
**per thread, concurrently**, and each `ORDER BY` forces DataFusion to build a full external sort.
Several concurrent `ExternalSorterMerge` instances share the single bounded
`datafusion.runtime.memory_limit` pool (default 1 GB) and exhaust it.

The `ORDER BY begin` is redundant: the physical scan behind `view_instance('thread_spans', ...)`
already emits rows in ascending `begin` order. The fix declares this existing input ordering to
DataFusion (per-view opt-in) so the `EnforceSorting` optimizer pass drops the `Sort` node instead of
materializing an `ExternalSorter`. The `ORDER BY` stays in the SQL and is still honored — it just
becomes free. This removes the memory pressure **at its source**: no per-thread sort buffer is ever
allocated, so concurrency no longer multiplies large sort allocations.

Per explicit direction, **concurrency is left unchanged** — we fix memory by optimizing the sort
away, not by capping `max_concurrent`. Async spans are **out of scope**: their query
(`generate_async_spans_with_writer`) ends in `ORDER BY b.begin_time` over a JOIN whose output
ordering is not guaranteed by a partition scan, so the sort-elimination technique does not apply.

## Current State

### The export path

`PerfettoTraceExecutionPlan::execute` → `generate_streaming_perfetto_trace` →
`generate_thread_spans_with_writer` (`rust/analytics/src/lakehouse/perfetto_trace_execution_plan.rs:331`).

For each thread it issues (`format_thread_spans_query`, line 305):

```sql
SELECT "begin", "end", name, filename, target, line
FROM view_instance('thread_spans', '<stream_id>')
WHERE begin <= TIMESTAMP '...' AND end >= TIMESTAMP '...'
ORDER BY begin
```

Queries run through `spawn_with_context` + `.buffered(max_concurrent)` (line 365) where `max_concurrent =
available_parallelism()` (line 337). Each concurrent query's `ORDER BY begin` builds an independent
external sort; together they blow the shared memory pool (the error trace shows multiple simultaneous
`ExternalSorterMerge` reservations).

### Why the scan is already sorted by `begin`

1. **Within a partition** — `thread_spans` partitions are built by a depth-first **preorder** walk of
   the call tree: `span_table.rs::for_each_node_in_tree` (line 172) visits a node, then recurses into
   its children. The call tree is constructed stack-based from the chronological event stream
   (`call_tree.rs` — `add_child_to_top`/`finish` push children in arrival = `begin` order), so
   siblings are already in ascending `begin` and are non-overlapping (a thread executes
   sequentially). Preorder over a properly-nested, time-ordered tree yields rows in ascending
   `begin`.

2. **Across partitions** — a `thread_spans` view instance is keyed by a single `stream_id` (one
   thread). Its JIT partitions (`thread_spans_view.rs::jit_update` →
   `generate_stream_jit_partitions`) cover contiguous, **non-overlapping** event-time windows of that
   one stream. `make_partitioned_execution_plan`
   (`rust/analytics/src/lakehouse/partitioned_execution_plan.rs:19`) bundles all partitions into a
   **single DataFusion file group** (`with_file_groups(vec![file_group.into()])`, line 64), which is
   scanned sequentially — files are never interleaved across parallel readers. So concatenating the
   (individually sorted, non-overlapping) partition files in ascending event-time order produces a
   globally `begin`-sorted stream.

   This cross-partition non-overlap is conditional, not structurally enforced: JIT partitions are
   sliced and ordered by `insert_time`
   (`generate_stream_jit_partitions`/`generate_stream_jit_partitions_segment` in `jit_partitions.rs`),
   so their event-time non-overlap holds only under the documented assumption that a stream's blocks
   are registered in event-time order (`thread_spans_view.rs:131-132`: "we assume that the blocks were
   registered in order since they are built based on begin_ticks, not insert_time"). That assumption
   is not enforced in code. If a stream ever received an out-of-order or late-arriving block, a later
   partition could contain an earlier `begin`, and — once the explicit `ORDER BY` is gone — the export
   would silently mis-order rows instead of being re-sorted.

The `Partition` struct (`partition.rs:8`) exposes `min_event_time()` / `max_event_time()`
(`partition.rs:34,39`) — the begin/end bounds of the partition's event-time range. We use
`min_event_time()` (the partition's minimum `begin`) as the robust cross-partition sort key (rather
than relying on the insert-time order the partition cache happens to return). Both return
`Option<DateTime<Utc>>`, so the sort considers only non-empty partitions.

### Plumbing

`view_instance(...)` → `ViewInstanceTableFunction::call_with_args` builds a `MaterializedView`
(`view_instance_table_function.rs:76`). `MaterializedView::scan`
(`materialized_view.rs:62`) fetches partitions and calls `make_partitioned_execution_plan`, which
builds a `FileScanConfig` via `FileScanConfigBuilder`. DataFusion 54's builder has
`with_output_ordering(Vec<LexOrdering>)` (`datafusion-datasource-54.0.0/.../file_scan_config/mod.rs:459`);
setting it also flips `preserve_order = true` in `build()` (line 536). No code currently sets it.

## Design

Add a **per-view opt-in** that declares the scan's already-satisfied output ordering, plumb it into
`make_partitioned_execution_plan`, and there sort the file group by the leading column's min value
and attach a `LexOrdering` to the `FileScanConfig`.

### 1. `View` trait opt-in (default: none)

Add a small value type and a defaulted trait method in `rust/analytics/src/lakehouse/view.rs`:

```rust
/// A column an ordering is expressed over (ascending unless `descending`).
#[derive(Clone, Debug)]
pub struct ScanSortColumn {
    pub column: Arc<String>,
    pub descending: bool,
}

// on trait View:
/// Declares an ordering the view's partition scan *already* emits, letting DataFusion
/// elide redundant `Sort` nodes for queries that `ORDER BY` these columns.
///
/// Returning a non-empty ordering is a correctness contract the view must guarantee:
/// - rows within each partition file are already sorted by these columns, AND
/// - the leading column is the view's min-event-time column, and partition event-time
///   ranges are non-overlapping (so files concatenate in globally-sorted order).
///
/// For `ThreadSpansView`, the non-overlapping-ranges half of this contract rests on JIT
/// partitions being sliced in event-time order, which in turn assumes a stream's blocks are
/// registered in event-time order — an assumption documented but not enforced (see
/// `thread_spans_view.rs:131-132`). If that assumption is ever violated, output would be
/// silently mis-ordered rather than re-sorted, since no `Sort` node remains once this ordering
/// is declared.
///
/// Default: empty (no declared ordering — DataFusion sorts as usual).
fn get_scan_output_ordering(&self) -> Vec<ScanSortColumn> {
    vec![]
}
```

`ThreadSpansView` (`thread_spans_view.rs`) overrides it to declare `begin` ascending:

```rust
fn get_scan_output_ordering(&self) -> Vec<ScanSortColumn> {
    vec![ScanSortColumn { column: MIN_TIME_COLUMN.clone(), descending: false }]
}
```

All other views keep the default empty vec, so their scans are unaffected (open/closed).

### 2. Plumb through `MaterializedView::scan`

`materialized_view.rs` passes `self.view.get_scan_output_ordering()` as a new argument to
`make_partitioned_execution_plan`. `PartitionedTableProvider::scan`
(`partitioned_table_provider.rs:63`) — the other caller — passes an empty slice (it has no such
guarantee).

### 3. `make_partitioned_execution_plan`

New signature:

```rust
pub fn make_partitioned_execution_plan(
    schema: SchemaRef,
    reader_factory: Arc<ReaderFactory>,
    state: &dyn Session,
    projection: Option<&Vec<usize>>,
    filters: &[Expr],
    limit: Option<usize>,
    partitions: Arc<Vec<Partition>>,
    output_ordering: &[ScanSortColumn], // NEW
) -> datafusion::error::Result<Arc<dyn ExecutionPlan>>
```

Changes inside:

- When `output_ordering` is non-empty, before building `PartitionedFile`s, collect the non-empty
  partitions and **sort them by `min_event_time()` ascending** (with `file_path` as a deterministic
  tiebreak). This makes the globally-sorted concatenation self-contained — independent of whatever
  order the partition cache returned. When empty, preserve today's behavior exactly (no sort).
- Build a `LexOrdering` from `output_ordering` against `schema` and pass
  `vec![lex]` to `FileScanConfigBuilder::with_output_ordering`:

```rust
use datafusion::physical_expr::{LexOrdering, PhysicalSortExpr, expressions::Column};
use datafusion::arrow::compute::SortOptions;

let mut builder = FileScanConfigBuilder::new(object_store_url, source)
    .with_limit(limit)
    .with_projection_indices(projection.cloned())?
    .with_file_groups(vec![file_group.into()]);

if !output_ordering.is_empty() {
    let sort_exprs = output_ordering
        .iter()
        .map(|c| {
            let col = Column::new_with_schema(&c.column, &schema)?;
            Ok(PhysicalSortExpr::new(
                Arc::new(col),
                SortOptions { descending: c.descending, nulls_first: false },
            ))
        })
        .collect::<datafusion::error::Result<Vec<_>>>()?;
    if let Some(lex) = LexOrdering::new(sort_exprs) {
        builder = builder.with_output_ordering(vec![lex]);
    }
}
let file_scan_config = builder.build();
```

`SortOptions { descending: false, nulls_first: false }` matches DataFusion's default `ORDER BY begin`
(ASC NULLS LAST); `begin` is non-nullable so nulls placement is moot, but we match it so
`EnforceSorting` recognizes the orderings as equivalent.

**`preserve_order` side effect.** Setting `with_output_ordering` flips `preserve_order = true` in
`FileScanConfigBuilder::build()`. In our case — a single file group containing multiple files — this
is a definitive no-op: `repartition_preserving_order` (`file_groups.rs:270-308`) returns `None` when
`target_partitions == 1`, and when `target_partitions > 1` the multi-file group is filtered out of the
heap (only `group.len() == 1` groups are split), leaving the heap empty → `None`. `DataSourceExec::
repartitioned` therefore returns `Ok(None)`, the plan stays at `UnknownPartitioning(1)`, and no
`SortPreservingMergeExec`/`RepartitionExec` is introduced.

### Resulting plan transformation

Before: `SortExec(begin) ← FilterExec ← DataSourceExec` → `SortExec` materializes an `ExternalSorter`.

After: `DataSourceExec` advertises output ordering `[begin ASC]`; `FilterExec` and the projection
preserve it; `EnforceSorting` sees the required `ORDER BY begin` is already satisfied and drops the
`SortExec`. The scan streams with bounded per-query memory, so N concurrent thread queries no longer
allocate N sort buffers.

```
per-thread query plan
  ┌───────────────────────────────┐        ┌───────────────────────────────┐
  │ SortExec [begin ASC]          │        │ (SortExec removed by           │
  │   FilterExec (begin<=..)      │  ──►    │  EnforceSorting)               │
  │     DataSourceExec            │        │   FilterExec (begin<=..)       │
  │       (no declared ordering)  │        │     DataSourceExec             │
  └───────────────────────────────┘        │       output_ordering=[begin]  │
     builds ExternalSorter                  └───────────────────────────────┘
                                                streaming, bounded memory
```

## Implementation Steps

1. **`view.rs`** — add `ScanSortColumn` struct and the defaulted `get_scan_output_ordering()` method
   on the `View` trait.
2. **`thread_spans_view.rs`** — override `get_scan_output_ordering()` to return `begin` ascending
   (reuse `MIN_TIME_COLUMN`).
3. **`partitioned_execution_plan.rs`** — add the `output_ordering: &[ScanSortColumn]` parameter; when
   non-empty, sort partitions by `min_event_time()` (tiebreak `file_path`) before building the file
   group, and attach the `LexOrdering` via `with_output_ordering`.
4. **`materialized_view.rs`** — pass `self.view.get_scan_output_ordering()` into the call.
5. **`partitioned_table_provider.rs`** — pass `&[]` into the call.
6. Update any other `make_partitioned_execution_plan` callers if present (grep confirms only the two
   above) and run `cargo fmt` + `cargo clippy --workspace -- -D warnings`.
7. Add the regression test (see Testing Strategy).

## Files to Modify

- `rust/analytics/src/lakehouse/view.rs` — new `ScanSortColumn` type + trait method.
- `rust/analytics/src/lakehouse/thread_spans_view.rs` — override the method.
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` — new param, file-group sort, ordering
  declaration.
- `rust/analytics/src/lakehouse/materialized_view.rs` — pass the view's ordering.
- `rust/analytics/src/lakehouse/partitioned_table_provider.rs` — pass empty ordering.
- A new DB-backed test file under `rust/analytics/tests/` (e.g. `thread_spans_ordering_tests.rs`) —
  regression test via the `MaterializedView`/`view_instance` path, modeled on the existing
  `histo_view_test.rs` / `sql_view_test.rs` harness pattern (`connect_to_data_lake` +
  `LakehouseContext::new`, `#[ignore]`-gated and requiring a live `MICROMEGAS_SQL_CONNECTION_STRING`).

## Trade-offs

- **Declare ordering vs. cap concurrency.** The issue suggested both. Per direction we take only the
  ordering fix: it removes the large allocations entirely rather than serializing them, so throughput
  is preserved and the fix is hardware-independent. Concurrency (`max_concurrent`) is intentionally
  untouched.
- **Sort the file group by `min_event_time` vs. trust cache order.** The partition cache returns
  `ORDER BY begin_insert_time, file_path`, which *usually* matches `begin` order for a single stream
  but is not guaranteed to. Sorting explicitly by `min_event_time` makes the declared ordering
  self-contained and cheap (an in-memory sort of partition metadata), removing the reliance on an
  insert-time≈event-time coincidence.
- **Per-view opt-in vs. global.** A blanket ordering declaration would be wrong for views whose scans
  aren't `begin`-sorted (e.g. multi-stream/global views with overlapping partitions). The defaulted
  empty method keeps every other view untouched and puts the correctness contract next to the view
  that can honor it.
- **Generic ordering type vs. hardcoding `begin`.** `ScanSortColumn` (with a `descending` flag)
  keeps the mechanism reusable for future single-stream, preorder-built views without re-plumbing.

## Documentation

No user-facing behavior changes (same rows, same order, same SQL), so no doc pages require updates.
Optionally add a short note to any internal lakehouse/perfetto architecture notes explaining the
`get_scan_output_ordering` contract; not required for merge.

## Testing Strategy

- **Regression test — monotonic `begin` across multiple partitions.** Build a synthetic
  `thread_spans` scenario for one stream that produces **more than one partition**. `ThreadSpansView`
  hardcodes `JitPartitionConfig::default()`, so forcing a second partition means either >20M objects in
  one hour or, more cheaply, block insert-times that deliberately span more than one 1-hour JIT
  segment. This needs a DB-backed harness that ingests through the `MaterializedView`/`view_instance`
  path. `rust/analytics/tests/histo_view_test.rs` and `rust/analytics/tests/sql_view_test.rs` already
  provide this pattern in-crate — `#[tokio::test]` + `#[ignore]` tests that require a live DB (they
  read `MICROMEGAS_SQL_CONNECTION_STRING` via `env::var(...).with_context(...)`, erroring rather than
  skip-gating when it is unset, and don't run under a plain `cargo test`), calling
  `connect_to_data_lake` and constructing `LakehouseContext::new(...)`. Note `get_view_instance_id`
  appears in their materialize/retire helpers, not the data query, and they query a global
  `SqlBatchView` registered via `add_global_view`; the new test instead queries
  `view_instance('thread_spans', <stream_id>)` by SQL string. The new regression test should be
  modeled on their DB-backed setup (`analytics/tests/span_tests.rs`, by contrast, is a pure in-memory parse test with no
  `LakehouseContext`, no DB, no `view_instance` query, and is not a fit). With the harness, query
  `view_instance('thread_spans', <stream_id>)` **with the `ORDER BY` removed** and assert `begin` is
  non-decreasing across the full result — and that it spans a partition boundary. Note
  `PartitionedTableProvider::scan` always passes `&[]` (no declared ordering), so that path does *not*
  exercise the new ordering and is not a valid substitute. This DB-backed test requires a live SQL
  connection to run; the file-group sort unit test in `make_partitioned_execution_plan` (assert
  `min_event_time` ordering with `file_path` tiebreak) and the plan-shape assertion below provide
  additional coverage that doesn't depend on a live DB.
- **Plan-shape assertion.** For a `SELECT ... FROM view_instance('thread_spans', ...) ORDER BY begin`
  query, capture the physical plan (`df.create_physical_plan()` + `displayable(...).indent()`) and
  assert **no `SortExec`** node appears. Add a negative control on a view that does *not* opt in to
  confirm its `SortExec` is still present.
- **Manual verification.** Re-run the failing export (moderate thread count, wide time range, thread
  spans only) against a running flight-sql server and confirm it completes without exhausting
  `datafusion.runtime.memory_limit`.
- `cargo test`, `cargo fmt`, `cargo clippy --workspace -- -D warnings`.
