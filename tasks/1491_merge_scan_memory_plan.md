# Bounded-Memory Merge Scans Plan

[#1491](https://github.com/madesroches/micromegas/issues/1491) — `measures` hourly merge dwarfs
every other view's and correlates with a ~700-800 MB daemon memory spike. The fix is not
`measures`-specific: it is one session setting on the shared `QueryMerger` path, and it applies
equally to `log_entries` and to every other view that merges through it.

## Overview

The issue asks two questions: is `measures`/`MetricsView` a good candidate for a
`ScanOrdering` declaration, and how much memory would that save?

**Answers: no, and none.** `measures` partitions satisfy neither precondition of
`ScanOrdering::Concatenated` (rows inside a partition are not time-ordered; partition event-time
ranges overlap heavily), so declaring it would be unsafe in a mixed way rather than a single clean
failure: `sort_and_check_non_overlapping` returns a hard error for any partition set that includes an
overlapping adjacent pair, but for a partition set with none — a single partition, or a query whose
time range selects a non-overlapping subset — the check passes, the declared ordering is attached,
and rows sorted within a file (the other precondition) still isn't true, so the query **silently
returns mis-ordered rows** instead of erroring. That silent branch is what most sampled `measures`
traffic would actually hit: JIT `view_instance` partitions are typically a single small file per
process (§3), which rarely has an adjacent pair to trip the error. `ScanOrdering::PerFile` would silently degrade to `Unordered`, because
`measures` partitions record no `sort_order`. (Two separate knobs read this declaration: only
`View::get_scan_output_ordering` — the user-query path — would break this way; `QueryMerger`'s
merge-side ordering is set independently via `with_merge_scan_ordering`, defaults to `Unordered`,
and `MetricsView` never calls it, so merges are unaffected either way.)

But the issue's underlying diagnosis is right, and it points at something more general than
`measures`. The memory win the issue attributes to `Concatenated` does not come from the ordering
at all — it comes from the one line `execute_concatenated_merge` runs before planning:
`repartition_file_scans = false`. With that setting left at its default (`true`), a merge's source
scan is split into `target_partitions` byte-range file groups and executed by
`target_partitions` **concurrent** Parquet readers, coalesced back to one stream by
`execute_stream`. The scan-side working set is therefore multiplied by the daemon host's core
count, for a merge that can only ever consume rows as fast as its single downstream writer.

The fix is to stop coupling "bounded merge scan" to "declared ordering": hoist
`repartition_file_scans = false` out of the two ordering-aware merge paths and apply it to every
`QueryMerger` merge, `Unordered` included. That is one small setting, it needs no correctness
contract from any view, and it benefits every view whose merge goes through `QueryMerger`'s default
`Unordered` route — `measures`, `log_entries`, `async_events`, `images`, `blocks`' plain-merger
fallback, and every `SqlBatchView` without a declared merge sort order. `BatchPartitionMerger` (the
other `PartitionMerger` implementation, currently unused in-repo) keeps its current scan shape;
Design §1 explains why applying the same setting there would trade its unbounded-memory scan for a
wall-clock regression rather than fixing it.

Two things follow from that, and this plan treats both as part of the fix rather than as side
effects.

**`log_entries` is not an incidental beneficiary — it is the second-largest view on the same
path.** Measured with `list_partitions()` on the deployment that reported the issue: `log_entries`
holds 288 GB across 1468 global partitions with a largest single partition of ~1 GB, against
`measures`' 1.86 TB across 1324 with a largest of ~5 GB. It is written by the same
`BlockPartitionSpec`, with the same `buffer_unordered` block ordering and the same insert-time
partition cuts, so every Current State finding about why `measures` can declare no ordering holds
for `log_entries` verbatim — same two failed preconditions, same mixed failure mode, same `PerFile`
no-op. Where the two views differ is in *who queries them*, and that difference reverses the
reasoning behind §2's and §3's conclusions without changing the verdicts (§3b).

**Once every merge scan is sequential there are only two merge strategies, not three.**
`Unordered` and `Concatenated` already build the identical scan today —
`build_unordered_or_concatenated_plan` puts every input partition in one file group for both — and
after this change they also execute it identically. The only thing separating them is whether the
resulting order is *declared*. The real taxonomy is **concatenate** (one sequential reader; output
is the inputs back to back) and **sort-merge** (k readers collapsed by a `SortPreservingMergeExec`,
on a certified per-file sort order), so `QueryMerger`'s three-arm dispatch collapses to two (§1b).
The `ScanOrdering` enum itself keeps three variants: on the *query* path "declare nothing" is a
distinct and necessary state. It is the merge dispatch, not the type, that has one strategy too
many.

Beyond memory, the sequential scan also buys a **query** win in mechanism, though a modest one in
practice. Today's interleaved
merge scatters each source minute across every row group of the merged hourly partition, so `time`
row-group statistics span the whole hour and a narrow-window query prunes nothing. A sequential scan
makes the merged file the concatenation of its inputs in insert-time order, which restores
time-local row groups — ordering by construction, with no declared contract and no sort. How much
that is worth depends on who queries the merged global partitions; Design §3 answers that for
`measures` and §3b for `log_entries`, both from 6-hour query-audit samples, and the same evidence
is why this plan stops short of a *recorded* sort rather than merely deferring one. The short
version: the mechanism is free, and in this deployment neither view collects on it — for opposite
reasons.

## Current State

### The merge path

`create_merged_partition` (`rust/analytics/src/lakehouse/merge.rs:332`) logs
`merging {n} partitions sum_size={sum_size}` — the number quoted in the issue — then delegates to
`view.merge_partitions(...)`. `MetricsView` (`rust/analytics/src/lakehouse/metrics_view.rs`)
overrides neither `merge_partitions` nor `get_scan_output_ordering`, so it takes the `View` trait
default (`view.rs:99`): a `QueryMerger` with query `SELECT * FROM source;` and
`merge_scan_ordering: ScanOrdering::Unordered` (`merge.rs:84`).

`QueryMerger::execute_merge_query` (`merge.rs:247`) branches on the ordering:

| Path | Optimizer settings | Execution |
|---|---|---|
| `Unordered` (`merge.rs:278`) | none | `df.execute_stream()` |
| `Concatenated` (`merge.rs:101`) | `repartition_file_scans = false` | build plan, check shape, `execute_stream(plan, ctx)` |
| `PerFile` (`merge.rs:157`) | `repartition_file_scans = false` + 4 more | build plan, 3 checks, `execute_stream(plan, ctx)` |

Only the `Unordered` path leaves file-scan repartitioning enabled.

### Why that makes the scan `target_partitions`-wide

The merge session context is built by `make_session_context` (`query.rs:256`) from a plain
`SessionConfig::default()`, so `target_partitions` is the host's core count and
`repartition_file_scans` is `true` (DataFusion default). Walking the plan for
`SELECT * FROM source;` over the `PartitionedTableProvider` registered at `merge.rs:264`:

1. `make_partitioned_execution_plan` with `ScanOrdering::Unordered` builds **one** file group
   holding all 60 partitions (`partitioned_execution_plan.rs:339`) — a single-partition
   `DataSourceExec`. No `Filter` survives: `source` is a bare table, not a `MaterializedView`, so
   `TableScanRewrite` skips it (`table_scan_rewrite.rs:36`), and `SELECT *` leaves no projection.
2. The physical optimizer's first rule, `OutputRequirements::new_add_mode`, wraps the root in an
   `OutputRequirementExec` with `Distribution::UnspecifiedDistribution`. **This is the part that
   matters**: it gives the scan a parent, so `ensure_distribution`'s
   `if dist_context.plan.children().is_empty() { return }` early-out does not fire.
3. `ensure_distribution` then hits the `repartition_file_scans && roundrobin_beneficial_stats`
   branch. `roundrobin_beneficial_stats` is `true` because the `Unordered` scan attaches no
   per-file statistics, so `num_rows` is `Precision::Absent`. It calls
   `DataSourceExec::repartitioned`, and `FileGroupPartitioner` splits the single file group into
   `target_partitions` byte-range groups (each partition file is ~33 MB, far above the 10 MB
   `repartition_file_min_size`).
4. `OutputRequirementExec` requires only `UnspecifiedDistribution` and returns
   `benefits_from_input_partitioning() == [false]`, so nothing collapses the fan-out. The
   ancillary node is removed by `OutputRequirements::new_remove_mode`, leaving a
   `target_partitions`-partition `DataSourceExec`.
5. `DataFrame::execute_stream` → `execute_stream(plan, ctx)` takes the `2.. =>` arm and wraps it in
   a `CoalescePartitionsExec`, which spawns **all** partitions concurrently into a
   `RecordBatchReceiverStream` of capacity `target_partitions`.

So the peak scan footprint is roughly `target_partitions × (row-group column chunks held by one
Parquet reader + decoded batches in flight)`, plus up to `target_partitions` queued batches in the
coalesce channel, plus whatever the L1 range cache holds behind those concurrent readers
(`L1_TOTAL_FETCH_PERMITS = 16` × `DEFAULT_MAX_COALESCED_GET_BYTES = 8 MB` in-flight, over a 200 MB
`BoundedMemoryBackend` budget). Every term except the L1 budget scales with core count, and all of
them scale with `sum_size` — which is exactly the shape the issue observed: same code path for
every view, 5-6× the data for `measures`, disproportionate memory.

This also explains why the existing `Concatenated` path needs its `assert_single_partition` check
with the message *"This likely means repartition_file_scans did not take effect"* — that check
exists precisely because this fan-out is what happens by default.

### Downstream of the scan, per merge

- `create_merged_partition` runs a full DataFusion `min`/`max` aggregate **per record batch**
  (`merge.rs:407` → `NamedColumnsTimeBounds::get_time_bounds`, `dataframe_time_bounds.rs:36`).
  Transient per batch, but thousands of plannings per hourly merge.
- The writer flushes when `arrow_writer.in_progress_size() > 100 MB`
  (`write_partition.rs:731`), with `max_row_group_row_count = 128 Ki`
  (`write_partition.rs:923`) and a `BufWriter` at `max_concurrency(2)`. So the write side
  contributes a bounded ~100 MB of in-progress row group plus part buffers — it does **not** scale
  with core count.

### Why `measures` — and `log_entries` — cannot declare an ordering

`View::get_scan_output_ordering`'s contract (`view.rs:150-209`) requires, for
`Concatenated { columns, bounds }`, that (a) rows within each partition file are already sorted by
`columns` and (b) partition ranges on the leading column do not overlap. `get_scan_output_ordering`
has exactly one consumer: `MaterializedView::scan` (`materialized_view.rs:94`), the user-query path.
`QueryMerger`'s merge-side ordering is a separate, independently-set field
(`with_merge_scan_ordering`, `merge.rs:87-90`, defaulting to `Unordered`, `merge.rs:84`), and
`MetricsView` never calls it — so everything below is about breaking `measures` **queries**, not
`measures` merges.

- **(a) fails.** `measures` partitions are written by `BlockPartitionSpec::write`, which streams
  per-block row sets through `.buffer_unordered(nb_tasks)`
  (`block_partition_spec.rs:144`) — its own rustdoc says it "processes blocks individually and out
  of order". Rows land in block-completion order. There is no sort anywhere in the write path and
  no write-time check like `thread_spans_view::ensure_begin_non_decreasing`.
- **(b) fails for `time`.** `measures` partitions are cut on *insert* time, while `time` is event
  time; a block's rows precede its insert time by the block's fill duration, which varies per
  stream. Consecutive one-minute partitions therefore overlap on `[min_event_time,
  max_event_time]`. `sort_and_check_non_overlapping` (`partitioned_execution_plan.rs:83`) walks
  `partitions.windows(2)` and returns `DataFusionError::Internal` on the first overlapping *adjacent
  pair* — but a partition set with no such pair (a single selected partition, or a query range that
  happens to pick a non-overlapping subset) produces no pair and passes with `Ok`. That is not safe:
  (a) still fails on its own, so the declared ordering gets attached anyway and the query silently
  returns mis-ordered rows instead of erroring. **Declaring `Concatenated` over `time` would
  therefore break `measures` queries in a mixed way — a hard error where selected partitions overlap,
  silent mis-ordering where they don't** — and the JIT `view_instance` population, where most sampled
  queries go (§3), lands mostly in the silent case: `MetricsView`'s JIT partitions use the default
  `BlockOrder::InsertTime` and are typically a single small file per process, so there's usually no
  adjacent pair to trip the check.
- **`insert_time` + `OrderingBounds::InsertTime`** satisfies (b) by construction but still fails
  (a), for the same `buffer_unordered` reason. This is what `blocks_view` gets right and `measures`
  cannot: `BlocksView`'s extract query ends in `ORDER BY blocks.insert_time, blocks.block_id`
  (`blocks_view.rs:68`), and its `merge_partitions` additionally requires every input to *carry*
  the recorded `sort_order` before taking the ordered path (`blocks_view.rs:198`).
- **`PerFile`** is gated by `Partition::certifies_sort_order` inside
  `make_partitioned_execution_plan` (`partitioned_execution_plan.rs:291`). `BlockPartitionSpec`
  passes `None` for `sort_order` (`block_partition_spec.rs:92`), so no `measures` partition
  certifies anything and the declaration would degrade to `Unordered` — a pure no-op.

**The same four bullets hold for `log_entries`, unchanged.** `LogView` takes the same `View` trait
defaults (`log_view.rs` overrides neither `merge_partitions` nor `get_scan_output_ordering`),
constructs the same `BlockPartitionSpec` (`log_view.rs:127`) with the same `buffer_unordered` block
ordering and the same `sort_order: None`, cuts partitions on insert time while `time` is event
time, and groups its JIT partitions under the same `BlockOrder::InsertTime` (`log_view.rs:192`).
Substitute the view name and every argument above reads the same, including the mixed failure
mode. `async_events` and `images` are the same shape again — `measures` and `log_entries` are
simply the two large enough to matter.

## Design

### 1. One setting, applied to every `QueryMerger` merge

There are two `PartitionMerger` implementations: `QueryMerger` (the default, ordering-aware path)
and `BatchPartitionMerger` (the bounded-memory fallback used when a merge's whole output can't be
held in memory at once — `sql_batch_view.rs:179`; no in-repo view constructs one today, but it's
one `session_configurator` call away).

This plan fixes `QueryMerger` only. `BatchPartitionMerger` keeps its current scan shape, unbounded
and all: `batch_partition_merger.rs:106-190` builds its own session via `make_session_context`,
registers an `Unordered` `PartitionedTableProvider` over the *whole* partition set being merged,
and re-executes the same `$begin`/`$end`-templated query once per batch — `nb_batches` times,
`try_buffered(2)` — with `repartition_file_scans` at its default `true`. Its output feeds the same
single writer task as `QueryMerger` (`create_merged_partition` drains whichever merger's stream
through the same `mpsc::channel(1)`), so the two are not asymmetric in *who* drains the scan — the
asymmetry is in how much is buffered ahead of that writer: the producer publishes into a 10-batch
`RecordBatchReceiverStreamBuilder` channel, and `try_buffered(2)` runs two of the `nb_batches`
re-executions concurrently, each independently fanning out to `target_partitions` readers. Per-batch
merge queries are also typically aggregating (batching only pays off when a batch's result is small
enough to hold, which usually means a `GROUP BY`), so the writer may not be the bottleneck the way
it is for `QueryMerger`'s single full-size scan. Forcing `repartition_file_scans = false` here would
therefore be an unmeasured trade, not a free one, on a shape this plan hasn't exercised. Since no
in-repo view constructs a `BatchPartitionMerger` today, and its `nb_batches`-times re-execution shape
needs its own measurement before changing its scan settings, this plan leaves its scan as-is rather
than guess, and records the gap explicitly instead: the first view that adopts `BatchPartitionMerger`
inherits its current unbounded-scan memory cost, unchanged by #1491. Bounding that scan without an
unmeasured wall-clock risk — e.g. a small pinned `target_partitions` for its session, rather than a
full serial scan repeated `nb_batches` times — is follow-up work for whoever takes that on, not part
of this plan.

Partition-*creation* extract queries are out of scope for the same reason `BatchPartitionMerger` is:
`SqlBatchView::make_batch_partition_spec` (`sql_batch_view.rs:256`) builds its session with the same
plain `make_session_context`, and `SqlPartitionSpec::write` streams that query into
`write_partition_from_rows` through a single-writer `mpsc::channel(1)` (`sql_partition_spec.rs:149`)
— structurally the same single-writer shape this plan's rationale is built on. They're left alone
because their queries are typically aggregating (a view's extract query usually does the `GROUP BY`
or windowing that shapes a partition, not a bare `SELECT *`), so the writer is less likely to be the
bottleneck and scan parallelism is less likely to be wasted the way it is for `QueryMerger`'s
non-aggregating default. Forcing `repartition_file_scans = false` there is an unmeasured trade for
the same reason as `BatchPartitionMerger`, not a free extension of this fix. This also matters for
Phase 2: these extract scans run in the same daemon whose memory the before/after sample measures,
so they're a potential confounder in that comparison, not just an unrelated code path.

`QueryMerger` gets the fix via one extracted helper, `make_merge_session_context(...)`, a thin
wrapper around `make_session_context` (`query.rs:256`) that applies the merge-only setting before
handing back the session, rather than each ordering-aware path building a session and then
separately remembering to set it. The plan-shape test in step 6 asserts against this same wrapper,
which guards the wrapper's own plan shape; it does not exercise `QueryMerger::execute_merge_query`
itself, so it would not catch a future regression where that method stops calling the wrapper.

Move `repartition_file_scans = false` out of the two ordering-aware paths and into the wrapper:

```rust
// merge.rs

// A merge is a bounded, streaming rewrite of a fixed set of files whose consumer is a single
// writer task (`create_merged_partition` -> `write_partition_from_rows`), not a query that
// benefits from scan parallelism. Left at DataFusion's default (`true`), `EnforceDistribution`
// splits the source scan into `target_partitions` byte-range file groups and `execute_stream`
// coalesces them, so the reader working set is multiplied by the host's core count for no
// throughput the writer can absorb. Forcing one sequential file group here fixes that for the
// `Unordered` and `Concatenated` shapes, which both start from a single file group spanning every
// input partition: left unsplit, one reader works through that group's files one at a time instead
// of `target_partitions` concurrent byte-range readers. `PerFile` already builds one file group per
// input partition, and this setting is load-bearing there too: left at `true`, `repartition_file_groups`
// (via `repartition_preserving_order`) splits each of those k per-partition groups into
// `target_partitions` byte-range groups whenever k < `target_partitions`, so this setting is what keeps
// `PerFile` at one reader per input file for its k-way ordered merge instead of `target_partitions`
// concurrent readers per file. Downstream parallelism is untouched: a merge query with a GROUP BY still
// gets its round-robin fan-out above the scan.
pub async fn make_merge_session_context(
    lakehouse: Arc<LakehouseContext>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    configurator: Arc<dyn SessionConfigurator>,
    caller: CallerContext,
) -> Result<SessionContext> {
    let ctx = make_session_context(
        lakehouse, part_provider, query_range, view_factory, configurator, caller,
    )
    .await?;
    ctx.state_ref()
        .write()
        .config_mut()
        .options_mut()
        .optimizer
        .repartition_file_scans = false;
    Ok(ctx)
}
```

Call it from `QueryMerger::execute_merge_query` in place of the direct `make_session_context` call.
Delete the now-redundant assignment from `execute_concatenated_merge` (`merge.rs:106-111`) and from
the block in `execute_per_file_merge` (`merge.rs:167`), leaving that path's other four settings where
they are.

Resulting plan for `SELECT * FROM source;`: a one-file-group `DataSourceExec`, one output
partition, `execute_stream`'s `1 => plan.execute(0, ctx)` arm, one Parquet reader at a time.

**Deliberately not added:** an `assert_single_partition` check on the `Unordered` path. Unlike the
other two paths, `Unordered` carries no ordering claim to protect, and it is the path a
`SqlBatchView` with no declared merge sort order uses with an arbitrary user-supplied
`merge_partitions_query` (`sql_batch_view.rs:194`). A `GROUP BY` in such a query legitimately plans
to multiple partitions, and coalescing it is correct. With `repartition_file_scans = false` those
queries keep their aggregation parallelism via round-robin repartition *above* a sequential scan —
which is the desirable shape, not something to bail on.

### 1b. Two merge strategies, not three

With `repartition_file_scans = false` applied to every `QueryMerger` merge, the three arms of
`execute_merge_query`'s `match &self.merge_scan_ordering` (`merge.rs:277-296`) produce two distinct
physical shapes, not three:

| `merge_scan_ordering` | Scan built by `make_partitioned_execution_plan` | Concurrent readers | Strategy |
|---|---|---|---|
| `Unordered` | `build_unordered_or_concatenated_plan`, `ordering: None` | 1 | concatenate |
| `Concatenated { .. }` | `build_unordered_or_concatenated_plan`, `ordering: Some(..)` | 1 | concatenate |
| `PerFile { columns }` | `build_per_file_plan` | k, one per input partition | sort-merge |

The first two call the same builder (`partitioned_execution_plan.rs:303-324`) and differ only in
bookkeeping *around* the same single file group: `Concatenated` first runs
`sort_and_check_non_overlapping`, attaches leading-column statistics per file
(`attach_ordering_statistics`), and declares a `LexOrdering`. The rows come out in the same order
either way — file by file, in the `begin_insert_time` order `create_merged_partition` sorted them
into (`merge.rs:371`). `Unordered` is not a third strategy; it is concatenation with the resulting
order left undeclared, which is exactly what §2 calls "ordering by construction". Before this plan
the two arms at least *executed* differently (`Unordered` kept the `target_partitions` fan-out);
after it, keeping them apart is bookkeeping masquerading as a strategy.

So collapse the dispatch to the two strategies that exist:

```rust
// merge.rs, QueryMerger::execute_merge_query
match &self.merge_scan_ordering {
    // Sort-merge: k ordered readers, collapsed by a SortPreservingMergeExec.
    ScanOrdering::PerFile { columns } => {
        self.execute_sorted_merge(&ctx, columns, insert_range).await
    }
    // Concatenate: one sequential reader over one file group. `Unordered` and `Concatenated`
    // are the same strategy -- they differ only in whether the resulting order is declared,
    // which is what gates the checks inside.
    ordering => {
        self.execute_concatenated_merge(&ctx, ordering.concatenated_columns(), insert_range)
            .await
    }
}
```

backed by a small accessor on the enum:

```rust
// partitioned_execution_plan.rs
impl ScanOrdering {
    /// The global ordering a *concatenating* scan of these partitions declares, if any.
    /// `PerFile` returns `None` because a per-file ordering only becomes a global one through a
    /// downstream merge -- it is the other strategy, not an undeclared version of this one.
    pub fn concatenated_columns(&self) -> Option<&[ScanSortColumn]> {
        match self {
            ScanOrdering::Concatenated { columns, .. } => Some(columns),
            ScanOrdering::Unordered | ScanOrdering::PerFile { .. } => None,
        }
    }
}
```

`execute_concatenated_merge` becomes the single concatenating path, taking
`declared: Option<&[ScanSortColumn]>`:

- **Always**: build the physical plan with `create_physical_plan()` and run it with
  `execute_stream(plan, task_ctx)`, replacing the `Unordered` arm's `df.execute_stream()`. Not a
  behavior change — `DataFrame::execute_stream` is exactly those two steps — but it puts both
  concatenating merges on one code path and makes the plan available to inspect and log on either.
- **Only when `declared.is_some()`**: `assert_single_partition` and the
  `SortExec`/`SortPreservingMergeExec` `ordering_honored` check, both unchanged. They exist to
  protect a declared ordering; with nothing declared there is nothing to protect, and §1's
  "Deliberately not added" reasoning still holds — an aggregating `Unordered` merge query
  legitimately plans to multiple partitions, and coalescing it is correct.
- `ordering_honored` stays `true` in the undeclared case, matching today's `Unordered` arm and the
  field's existing rustdoc ("Always `true` when no ordering was declared").

Rename `execute_per_file_merge` to `execute_sorted_merge` so the two method names are the two
strategies rather than one strategy and one scan shape. `ScanOrdering::PerFile` keeps its name — it
describes the scan shape, which is what that enum is for.

**The enum stays at three variants.** `ScanOrdering` has a second consumer,
`MaterializedView::scan` on the user-query path (`materialized_view.rs:94`), where `Unordered`
means "declare no ordering to DataFusion" and is neither redundant nor expressible as
`Concatenated`. The collapse is in the merge dispatch and in the vocabulary the docs use, not in
the type. `make_partitioned_execution_plan` already encodes the same two-shape split for both
consumers: its `PerFile`-that-does-not-certify arm degrades to `Unordered`
(`partitioned_execution_plan.rs:290-300`), which is precisely "fall back from sort-merge to
concatenate".

### 2. What this changes about the output — and what it recovers for queries

For a non-aggregating merge query — the default `SELECT * FROM source` shape most views use —
merged row order becomes deterministic: file concatenation in the order `create_merged_partition`
already sorts them (`filtered_partitions.sort_by_key(|p| p.begin_insert_time())`, `merge.rs:371`),
instead of today's nondeterministic byte-range interleave from `CoalescePartitionsExec`. This does
not extend to aggregating `Unordered` merges: a `GROUP BY` merge query (§5 — `processes`, `streams`,
and `log_stats`'s unordered fallback) still gets round-robin and hash repartitioning above the
sequential scan and is coalesced back nondeterministically, so those views' merged row order stays
as nondeterministic as it is today.

That is not a cosmetic difference — **it restores row-group pruning on the merged partition**, which
today is effectively dead:

- Today, ~`target_partitions` readers are positioned at byte offsets spread across the whole
  hour of source files, and `CoalescePartitionsExec` interleaves their batches in completion order.
  Every 128 Ki-row row group in the merged hourly partition therefore contains rows drawn from
  ~`target_partitions` different minutes spread across the hour, so its `time` min/max statistic
  spans nearly the full hour. A query filtering `time BETWEEN a AND b` over a narrow window pushes
  its predicate into the scan (`make_time_filter`, `metrics_view.rs:215` →
  `filters_to_predicate` → `ParquetSource::with_predicate`) and prunes **nothing** — it reads
  the entire hourly partition.
- After the change, the merged file is the concatenation of its 60 one-minute inputs in insert-time
  order, so row groups are time-local at roughly minute granularity and a narrow-window query
  touches only the row groups covering it.

The correlation is insert time, not event time, so this is clustering rather than a guarantee: a
stream with a long block-fill interval contributes events older than its insert window, which
widens some row groups' `time` range. Pruning improves substantially without becoming exact — for
an exact bound, see §3.

Three limits on how much this is worth, all quantified in §3 and §3b. Pruning can never select
less than one row group (128 Ki rows, `write_partition.rs:923`), so the benefit scales with row
groups per partition — large on merged hourly partitions, negligible on fresh one-minute ones.
Pruning also only pays when a query's window is materially narrower than the partition it lands in;
a query asking for the whole hour reads the whole hourly partition however well its row groups are
clustered. And a view collects on either of those only in proportion to how much of its traffic
reaches merged global partitions at all. In the deployment that reported this issue the global
`measures` view fails the third test (§3: ~2 queries in 6 hours) and the global `log_entries` view
fails the second (§3b: heavily queried, but no sampled query asks for a window narrower than the
partition it reads). The mechanism is real and free; the payoff is deployment-dependent, and in
this deployment it is close to zero for both of the views this plan is about.

No guarantee is *recorded*: `get_merged_partition_sort_order` still returns `None` for
`MetricsView`, so no query plan may assume it, and nothing certifies a `sort_order`. This is
ordering by construction — free, contract-free, and applying to every view on the default merge
path.

### 3. What an enforced sort would add, what it would cost, and what the queries actually ask for (`measures`)

Ordering by construction (§2) is clustering, not a declared order. A real, recorded sort on
`measures` would add three things §2 cannot: exact row-group pruning (bounded by the sort key, not
by insert/event-time skew), `ORDER BY` elision, and — via `ScanOrdering::PerFile` — a k-way
streaming merge. The machinery already exists: #1392 built `PerFile`,
`Partition::certifies_sort_order`, and the `SortPreservingMergeExec` merge path, with `log_stats`
as its in-repo adopter. So the question is not "can we?" but "what would it buy?" — and that is
answerable from data rather than argument.

**The query evidence.** A 6-hour sample of `flight-sql-srv`'s own query audit log
(`log_entries` where `target = 'flightsql_query_audit'`, written by
`rust/public/src/servers/query_audit.rs`) from a production deployment, covering 548 queries that
touch `measures`:

- **Every one of them filters `WHERE name = '<literal>'`.** Nothing else is ever filtered — not
  `target`, `unit`, `stream_id`, `computer`, `username`, or `exe`. About half additionally
  `ORDER BY time`; the rest are `min`/`max`/`avg` aggregates over the requested range.
- **546 of the 548 go to per-process JIT view instances** (`view_instance('measures', <process_id>)`).
  **The global `measures` view — the one whose hourly merge this issue is about — received 2
  queries in 6 hours.**
- Those queries are not scan-bound. A per-process partition is a single few-MB Parquet file of
  which roughly 1 MB is read; p50 latency (~2 s) is dominated by JIT freshness checking over a
  requested range far wider than the data the process actually holds, not by I/O.
- Sampling caveat: 544 of the 546 come from one auto-refreshing dashboard against a single process.
  This is a real workload but a narrow one, and it should not be treated as the shape of every
  deployment.

That evidence answers the sort-key question and then largely dissolves the case for asking it:

**a. The sort has to happen at write time, and that is where `measures` has no spare memory.**
Sorting at merge time is a blocking `SortExec` over the full ~2 GB — precisely the blowup #1392
exists to remove. So the sort must go in `BlockPartitionSpec::write`, which today never materializes
a partition at all: it streams per-block row sets straight into the Parquet writer
(`block_partition_spec.rs:146-161`). Sorting means buffering a whole one-minute partition (~33 MB
compressed, roughly 100-200 MB as Arrow) and sorting it, on the **every-minute** path, on top of the
~100 MB of concurrent block payloads that path already holds by design
(`block_partition_spec.rs:107`). That trades an hourly spike for a per-minute one. `BlockPartitionSpec`
is also shared with `log_entries`, `async_events`, and `images`, so it would have to become opt-in
per view. The JIT population is written by a different path again
(`jit_partitions::write_partition_from_blocks`), so covering both means sorting in two places.

**b. Sorting scatters the per-block clustering the schema compresses on.** Rows arrive grouped by
block, and within a block `process_id`, `stream_id`, `block_id`, `exe`, `username`, `computer`, and
`process_properties` are constant — long dictionary runs that RLE compresses to nearly nothing.
That is 7 of the 14 columns in `metrics_table_schema()`. Any sort key interleaves streams and turns
those runs into effectively random dictionary indices. A `name`-leading key at least pays some of
this back (`name` itself becomes one run per value), but the net on a 2 GB/hour view is still a
storage and scan-bytes regression that has to be measured, not assumed.

**c. It does not help the merge memory this issue is about — it may hurt.** `PerFile` gives each
input partition its own plan partition, so the hourly merge would open **k = 60** concurrent
Parquet readers (#1392 §4: "k open Parquet readers", with round-robin repartitioning disabled so
concurrency is k rather than `target_partitions`). That is more concurrent readers than today's
`target_partitions`, not fewer. #1392 accepts that trade because it removes a blocking aggregate
sort; `measures`' merge query is a bare `SELECT * FROM source`, so there is no aggregate to unblock
— only §1's sequential scan is strictly better on memory.

**d. If a sort ever happens, the key is `(name, time)` — not `time`.** The evidence is unanimous:
`name` is the only column ever filtered, and it is high-cardinality, so it is the only key that can
prune. This overrides the instinct (and #1392 §7's `log_stats` reasoning) to lead with time: leading
with `time` would prune nothing for a workload that always narrows by `name` first. A
non-temporal leading key rules out `Concatenated` and forces `PerFile` — which is precisely the
shape #1392 was built for.

**e. But neither `measures` population is a good target for it.** The two populations are disjoint,
and the sort would land on the wrong one:
- **Global partitions** are the ones this plan's merge touches, and after merging they carry enough
  row groups for a `name` key to bite — but they see ~2 queries per 6 hours. Almost no query value
  to capture.
- **JIT per-process partitions** absorb essentially all the query traffic, but they are single
  small files with few row groups, and their latency is dominated by JIT freshness checking rather
  than bytes scanned. A sort key cannot prune below one row group, so there is little to win.

**f. Row-group granularity floors the win regardless of key.** Pruning cannot select less than one
row group (128 Ki rows, `write_partition.rs:923`), so the benefit scales with row groups per
partition. Measured on a comparable high-volume metrics view in the same deployment: a `name`
matching 0.0008% of rows cut scanned bytes only 4.7× (101 MB → 21.6 MB) on fresh one-minute
partitions, because such a partition is only ~7 row groups and one value cannot occupy less than
one of them. The same measurement on merged hourly and daily partitions — hundreds to thousands of
row groups — is where a declared key actually pays. This cuts both ways for §2 as well: its
concatenation-order clustering is likewise worth most on merged partitions and nothing on fresh
ones.

**g. Rollout is self-healing but slow.** Existing partitions record `sort_order` NULL and will not
certify, so merges keep taking the unordered path until the whole retention window has been
re-materialized. No correctness risk (the certification gate at
`partitioned_execution_plan.rs:291` degrades silently), but the benefit arrives gradually.

**Conclusion: sorting `measures` is not currently justified, and the evidence — not the effort —
is why.** The population that would benefit is barely queried; the population that is queried is
bound by something else. That verdict is contingent on one deployment's 6-hour sample from one
dominant dashboard, so it should be revisited if global-view query volume grows or a second
deployment shows a different mix — and if it is revisited, `(name, time)` is the key and #1392's
`PerFile` is the mechanism. The larger finding from the same sample is worth its own issue and is
**not** about ordering at all: those queries spend their time in JIT freshness checking over
over-wide requested ranges, which no sort key or scan change can address.

### 3b. The same question for `log_entries` — opposite traffic, same answer

`log_entries` sits on the identical write and merge path as `measures` (§1's Overview), so §3's
*mechanics* transfer unchanged: the sort would have to happen in `BlockPartitionSpec::write` on the
every-minute path, it would scatter the same per-block dictionary runs, and `PerFile` would raise
merge reader concurrency to k. What does not transfer is the query evidence, which is close to
inverted — so it is worth checking separately rather than assuming the verdict carries.

**The query evidence.** Same source and same 6-hour window as §3 (`log_entries` where
`target = 'flightsql_query_audit'`, production deployment), for the 3178 audited queries whose SQL
references `log_entries`. The window is rolling, so counts drift by a few tens between runs; the
proportions below are what matter and they are stable.

- **3171 of 3178 go to the global view**; 7 use `view_instance('log_entries', ...)`. That is the
  exact opposite of `measures`, where 546 of 548 went to per-process JIT instances and the global
  view saw 2. So for `log_entries` the population this plan's merge touches *is* the population
  being queried.
- **Only 28 distinct SQL statements** account for all 3178 — this is dashboard traffic, refreshing.
- **What they filter:** 3169 filter on `level`; 3153 call
  `property_get(process_properties, '<key>')`; 3026 `GROUP BY`; 289 `ORDER BY`; 11 mention
  `process_id`. No high-cardinality scalar column is ever filtered.
- **All of it is on fresh data**: 3170 of 3178 have `query_time - range_end ≤ 1 hour`.
- **Window width vs. bytes scanned**, over the 3171 global-view queries, bucketing on the audit
  record's `range_end - range_begin`:

  | Requested window | Queries | Avg bytes scanned | Max | Total |
  |---|---|---|---|---|
  | ≤ 2 min | 2880 | 1.8 MB | 5 MB | 5.0 GB |
  | ≤ 1 hour | 272 | 175.8 MB | 315 MB | 46.7 GB |
  | ≤ 1 day | 19 | 856.9 MB | 1.6 GB | 15.9 GB |
  | > 1 day | 1 | 0 | 0 | 0 |

That evidence changes §3's reasoning and leaves its verdict intact:

**a. The pruning win (§2) still does not land — but for the opposite reason.** For `measures` it
was that nobody queries the merged global partitions. For `log_entries` everybody does, and it
still does not pay, because no sampled query asks for a window materially narrower than the
partition it reads. The ≤ 2-minute queries average 1.8 MB — two orders of magnitude below the wide
bucket — which is them landing on fresh one-minute partitions, where a partition already *is* the
window and there is nothing to prune. The queries that do reach merged partitions ask for the whole
hour or the whole day. Better row-group clustering cannot help a query that wants every row group.
This is the third limit §2 now lists, and `log_entries` is the view that demonstrates it.

**b. There is no candidate sort key.** For `measures`, §3d found one — `name`, unanimously filtered
and high-cardinality — and rejected the sort only because the population that would benefit was not
the population being queried. `log_entries` does not get that far: the only scalar column ever
filtered is `level`, which has six values (`micromegas_tracing::Level`) and cannot prune, and
everything else goes through
`property_get` over the `process_properties` map, which is not a sortable scalar column at all. A
`(time, ...)` key would prune nothing these dashboards ask for, since they already narrow by time
through the query range. So the answer here is not "right key, wrong population" — it is that no key
exists.

**c. The write-side cost is the same, and lands on a second high-volume view.** `BlockPartitionSpec`
is shared, so a sort would be opt-in per view (§3a); opting `log_entries` in buys the same
whole-partition write-time buffer on the every-minute path, for the view whose evidence is weaker
than `measures`'.

**d. Same sampling caveat, and one finding worth flagging.** Twenty-eight statements from one
dashboard family is a real workload but a narrow one, and it should no more be read as the shape of
every deployment than §3's sample should. The finding it does support is not about ordering: of the
67.6 GB scanned by global `log_entries` queries in 6 hours, 62.6 GB comes from the 292 queries
asking for a window wider than 2 minutes, and nothing in this plan — or in any sort key — addresses
that. It belongs with the follow-up in step 9.

**Conclusion: no sort for `log_entries` either, and the §2 pruning win does not materialize for it.
The memory fix in §1 applies to it in full and does not depend on any of the above** — it is 288 GB
of partitions merging through the same `target_partitions`-wide scan, with a largest partition of
~1 GB, and the fan-out costs the same multiple of the reader working set there as it does for
`measures`.

### 4. Expected improvement, and how to size it

The scan-side component of the spike is linear in `target_partitions`; this change takes it to
`1×`. On a 16-core daemon that is a ~16× reduction of that component. What remains is
core-count-independent:

- writer in-progress row group: ≤ ~100 MB (`write_partition.rs:731`)
- L1 range-cache budget: 200 MB (`MICROMEGAS_OBJECT_CACHE_L1_MB`, `l1_store.rs:26`)
- one Parquet reader's row group + in-flight coalesced GETs: tens of MB

so the predicted post-change hourly spike is in the low hundreds of MB rather than 700-800 MB.
That is a model, not a measurement — Phase 2 exists to confirm it against the same five-hour
sampling the issue used. The honest caveat: `used_memory` is a **host-wide** `sysinfo` gauge
(`telemetry-sink/src/system_monitor.rs:90`), not this process's footprint. The daemon already
enables `jemalloc-metrics` (`telemetry-maintenance-srv/Cargo.toml:17`), so
`process_resident_bytes`, `jemalloc_allocated_bytes`, and `jemalloc_resident_bytes` are available
and are what the before/after comparison should actually use. A gap between
`jemalloc_allocated_bytes` (flat) and `jemalloc_resident_bytes` (spiking) would say the residual is
allocator retention from streaming churn, not a live working set — a different problem with a
different fix, and worth knowing before tuning anything else.

### 5. Cost: merge wall-clock

Decode and decompression of the source files stop overlapping across cores. The pipeline's serial
bottleneck is already the single writer task (LZ4 encoding of the output on one thread, fed through
an `mpsc::channel(1)`), so the added cost is the scan's serial time no longer hiding behind it —
expect roughly 1.5-2× on the largest merges, not `target_partitions`×. The `blocks` view has run
its ordered merges on exactly this sequential shape in production since #1340, so the shape is not
novel. Phase 2 measures it; the hourly budget for one view's merge is generous, and the daemon
materializes views strictly sequentially (`materialize_all_views`, `public/src/servers/
maintenance.rs`), so a slower merge delays only later views in the same pass.

That 1.5-2× bound assumes the single writer stays the bottleneck, which needs the merge output to
be close in size to the scan input. It does not hold for aggregating `Unordered` merges, where a
`GROUP BY` shrinks the output by orders of magnitude: `processes` and `streams`
(`SqlBatchView`s whose merge query is a `GROUP BY` aggregate on the default `Unordered`
`QueryMerger` — `processes_view.rs:47-68`, `streams_view.rs:41-53`) and `log_stats`'s merge
whenever it falls back to the plain merger because an input hasn't yet certified its `sort_order`
(the gate at `partitioned_execution_plan.rs:291` — true of every existing partition until
re-materialization). There the writer is idle most of the time, so the now-serial scan is the
bottleneck: expect closer to `target_partitions`× on those merges, not 1.5-2×. In practice that's
bounded by `processes`/`streams`' small partition volumes, and by `log_stats` shrinking to its
ordered `PerFile` path as partitions re-materialize.

`BatchPartitionMerger` is untouched by this change (Design §1), so this estimate does not apply to
it either way: its scan shape, and whatever wall-clock cost that carries today, is unchanged by
this plan.

If the wall-clock regression turns out to matter, the escape hatch is the already-scoped backlog
item `tasks/backlog/datafusion_target_partitions_config.md` (`MICROMEGAS_DATAFUSION_TARGET_PARTITIONS`),
which lets an operator trade memory for parallelism globally. That knob is **not** part of this
plan: it does not fix the merge path (a merge wants *one* reader, not "fewer"), and it would
silently affect user queries too.

### 6. Optional, separable: don't populate L1 from merge reads (Phase 3)

A merge reads each source file exactly once, and those files are retired and deleted immediately
after. Caching them is pure cost: 2 GB streamed through a 200 MB LRU
(`BoundedMemoryBackend`) both holds 200 MB and evicts everything the daemon's own queries had
warm. `LakehouseContext` builds one `ReaderFactory` over an `l1_wrap`ped store
(`lakehouse_context.rs:88` and `:114`) and hands it to every consumer.

Shape: add a second, unwrapped `ReaderFactory` on `LakehouseContext` (reading
`lake.blob_storage.inner()` directly, sharing the same `MetadataCache` — footer metadata *is* worth
caching) and have `QueryMerger::execute_merge_query` use it for the `source` table. This is a
separate change with a separate measurement; it is not needed to close #1491 and should not be
folded into Phase 1's before/after. Same reasoning `mkdocs/docs/architecture/caching.md`'s "What is
intentionally not cached in L1" section already gives for raw telemetry blocks — read exactly once,
no reuse benefit — so Phase 3 adds a bullet there for merge reads (see Documentation).

## Implementation Steps

### Phase 1 — Bounded merge scan (the fix)

1. `rust/analytics/src/lakehouse/merge.rs`: extract `pub async fn make_merge_session_context(...)
   -> Result<SessionContext>` with the comment from Design §1 — same parameters as
   `make_session_context`, calling it internally and applying `repartition_file_scans = false`
   before returning. Change `QueryMerger::execute_merge_query` to call this wrapper instead of
   `make_session_context` directly, before the `match self.merge_scan_ordering`.
   `BatchPartitionMerger::execute_merge_query` (`batch_partition_merger.rs`) is **not** changed —
   it keeps calling `make_session_context` as today (Design §1).
2. Same file (`merge.rs`): remove the assignment from `execute_concatenated_merge` and from
   `execute_per_file_merge`'s optimizer block; update both methods' rustdoc, which currently
   describes setting it as part of their own path (`merge.rs:97-100`, `merge.rs:150-156`), to
   reference the shared setting instead.
3. `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`: add
   `ScanOrdering::concatenated_columns(&self) -> Option<&[ScanSortColumn]>` with the rustdoc from
   Design §1b, and extend the `ScanOrdering` enum's own rustdoc to name the two scan shapes (single
   sequential file group vs. one group per file) and to say that `Unordered` and `Concatenated` are
   the same shape differing only in what they declare.
4. Same file (`merge.rs`): collapse `execute_merge_query`'s three-arm `match` to the two arms in
   Design §1b. Change `execute_concatenated_merge`'s signature to take
   `declared: Option<&[ScanSortColumn]>`, gate `assert_single_partition` and the `ordering_honored`
   plan-string check on `declared.is_some()` (returning `ordering_honored: true` when it is `None`),
   and delete the `ScanOrdering::Unordered` arm's inline `df.execute_stream()` body — the
   concatenating path now builds the plan explicitly for both. Rename `execute_per_file_merge` to
   `execute_sorted_merge`. Update `MergeQueryResult::ordering_honored`'s rustdoc
   (`merge.rs:38-46`), which describes the field per-variant, to describe it per-strategy.
5. `rust/analytics/src/lakehouse/view.rs`: extend the `get_scan_output_ordering` rustdoc with a
   short note that a bounded merge scan is *not* a reason to declare an ordering — every merge is
   sequential regardless — so the next view author facing a big merge does not repeat #1491's
   reasoning.
6. `rust/analytics/tests/merge_scan_partitioning_tests.rs` (new): offline plan-shape test, modelled
   on `log_stats_ordering_tests.rs`'s `make_offline_lakehouse_context` helper (in-memory object
   store, no DB, lazily-connected) with fabricated `Partition`s (`file_size` above the 10 MB
   `repartition_file_min_size`) registered as the source table. Build two sessions — one from
   `make_merge_session_context(...)`, one from plain `make_session_context(...)` (control) — pinning
   `target_partitions` to 8 on each returned `SessionContext`, matching the precedent in
   `sql_batch_view_merge_ordering_tests.rs` and `log_stats_ordering_tests.rs`, so the control
   assertion doesn't silently no-op on a low-core-count CI runner (DataFusion otherwise defaults
   `target_partitions` to the host's core count). `create_physical_plan()` for `SELECT * FROM
   source` against each and assert:
   - with `make_merge_session_context`: `partition_count() == 1`
   - with plain `make_session_context` (control, guarding that the test is meaningful):
     `partition_count() > 1`
   Asserting against the wrapper itself, rather than a bare config-mutating helper the test drives
   by hand, guards the wrapper's own plan shape. It does not exercise
   `QueryMerger::execute_merge_query` — step 7 covers that.
7. Same test file: cover the step-4 collapse by driving `QueryMerger::execute_merge_query` itself
   over the offline lakehouse context with the default `Unordered` ordering and a
   `SELECT * FROM source` query, and assert the drained stream's rows are the inputs concatenated in
   `begin_insert_time` order. Two fabricated single-row partitions with distinguishable values are
   enough. This is the coverage the plan-shape test above does not give: it is what fails if a
   future refactor reverts the undeclared path to `df.execute_stream()` without the wrapper, or
   drops the wrapper call from `execute_merge_query`.

### Phase 2 — Measure and answer the issue

8. `rust/analytics/src/lakehouse/merge.rs`: extend `create_merged_partition` to log a completion
   line next to the existing `sum_size` line — elapsed wall-clock and output `file_size` — so
   before/after is queryable from `log_entries` without new instrumentation. (Deliberately a log
   line, not a metric: it pairs with the `sum_size` line the issue already quotes.)
9. Collect a five-hour sample matching the issue's, using `process_resident_bytes` /
   `jemalloc_allocated_bytes` / `jemalloc_resident_bytes` rather than host `used_memory`, plus the
   new duration line.
10. Measure row-group pruning before/after: run the same narrow-window query — projecting
   representative payload columns, not just `time`, so `bytes_scanned` is comparable to partition
   size (see Testing Strategy) — against a pre-change and a post-change merged hourly partition and
   compare `bytes_scanned` from `log_entries WHERE target = 'flightsql_query_audit'`. Do this for
   both `measures` and `log_entries`: it is the mechanism check, and it should show the improvement
   for a *synthetic* narrow-window query on both views even though §3/§3b predict that no real
   query in this deployment's sampled traffic collects on it. If the synthetic query shows no
   improvement, the §2 mechanism claim is wrong and should be struck rather than explained away.
   This is the query-side half of the result, and the baseline for step 12.
11. Post the answer on #1491: `Concatenated` is unsafe for `measures` (both preconditions fail, and
   the failure is mixed — a hard `sort_and_check_non_overlapping` error where selected partitions
   overlap, silent mis-ordered rows where they don't — landing on the query path, via
   `get_scan_output_ordering`, not the merge path), `PerFile` would be a no-op, the win was never in
   the ordering, and here is the measured before/after — memory and pruning. Note that the same
   holds for `log_entries` (§3b) and that the fix is shared, so the issue is closed for every view
   on the default merge path rather than for `measures` alone.
12. File the follow-up issues Design §3 and §3b's evidence actually point at, neither of which is
   about ordering:
   - `measures` queries through per-process JIT view instances spend their time in JIT freshness
     checking over requested ranges far wider than the process's data, not in scanning (§3).
   - Global `log_entries` dashboard queries scanned 67.6 GB over 6 hours, 62.6 GB of it from the
     292 refreshes asking for a window wider than 2 minutes, over a 288 GB view (§3b). Nothing in
     this plan, and no sort key, touches that.
   Both are larger wins in the sampled workload than anything here.

### Phase 3 — Optional, only if Phase 2 leaves the L1 component significant

13. `rust/analytics/src/lakehouse/lakehouse_context.rs`: add an uncached `ReaderFactory` sharing
    the existing `MetadataCache`.
14. `rust/analytics/src/lakehouse/merge.rs`: use it for the `source` table.
15. Re-measure independently.

## Files to Modify

- `rust/analytics/src/lakehouse/merge.rs` — the setting, the `make_merge_session_context`
  extraction, the two-arm dispatch collapse (`execute_concatenated_merge` taking
  `Option<&[ScanSortColumn]>`, `execute_per_file_merge` → `execute_sorted_merge`), rustdoc, the
  completion log line
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` — `ScanOrdering::concatenated_columns`
  and the enum's two-scan-shape rustdoc
- `rust/analytics/src/lakehouse/view.rs` — `get_scan_output_ordering` rustdoc note
- `rust/analytics/tests/merge_scan_partitioning_tests.rs` — new plan-shape regression test plus the
  `execute_merge_query` concatenation-order test
- `rust/analytics/src/lakehouse/lakehouse_context.rs` — Phase 3 only
- `CHANGELOG.md` — Unreleased / Analytics entry
- `mkdocs/docs/admin/maintenance.md` — see Documentation
- `mkdocs/docs/admin/monolith.md` — see Documentation
- `mkdocs/docs/architecture/caching.md` — Phase 3 only, see Documentation

Not modified: `metrics_view.rs` (the conclusion of this plan is that `measures` needs no
view-level change), `rust/analytics/src/lakehouse/batch_partition_merger.rs` (Design §1: it
keeps its current scan shape), and `rust/analytics/src/lakehouse/sql_batch_view.rs` /
`sql_partition_spec.rs` (Design §1: partition-creation extract queries are out of scope for the
same reason).

## Trade-offs

**Sequential merge scan vs. bounded merge memory.** Chosen: bounded memory. A merge's consumer is
one writer task behind an `mpsc::channel(1)`; scan parallelism buys throughput the pipeline cannot
absorb while multiplying the reader working set by the core count. The cost is quantified in
Design §4 and measured in Phase 2.

**Fix the shared merge path vs. patch `measures`.** Chosen: the shared path — `QueryMerger`, not a
`measures`-specific change. Every view whose merge takes `QueryMerger`'s default `Unordered` route
has the same fan-out; `measures` is merely the one large enough to make it visible. A
`measures`-only change would leave `log_entries`, `async_events`, `images`, and every unsorted
`SqlBatchView` with the same behavior — and, per Current State, there is no correct
`measures`-only change to make. `log_entries` is the concrete case for this: same write path, same
merger, 288 GB and a ~1 GB largest partition, and — per §3b — an entirely different query workload
that reaches the same conclusion about ordering. A fix scoped to `measures` would have had to be
re-derived for it.

**Collapse the merge dispatch to two strategies vs. leave three arms.** Chosen: two (Design §1b).
Once `repartition_file_scans = false` is unconditional, `Unordered` and `Concatenated` execute the
same scan; keeping separate arms leaves a distinction that no longer corresponds to anything the
merge does, and it is the distinction that made this fix look like it needed a declared ordering in
the first place — which is exactly the reasoning #1491 asked us to repeat. The cost is that the
undeclared path now builds its physical plan explicitly instead of calling `df.execute_stream()`;
that is the same two steps, so the cost is a slightly longer method, not a behavior change.

**Rejected: collapse `ScanOrdering` to two variants.** The tempting follow-through — delete
`Unordered`, or fold it into `Concatenated { columns: vec![] }` — breaks the query path.
`get_scan_output_ordering` is consumed by `MaterializedView::scan`, where "declare no ordering to
DataFusion" is a real state that a `Concatenated` with an empty column list does not express: that
variant would still run `sort_and_check_non_overlapping` (`partitioned_execution_plan.rs:83`),
imposing a non-overlap requirement on every view that today declares nothing. Three variants
describe the scan; two strategies describe the merge; those are different counts of different
things, and only the second one is over-counted today.

`BatchPartitionMerger`, the other `PartitionMerger` implementation, is deliberately left out of this
fix rather than folded in alongside `QueryMerger`. It drains into the same single writer task as
`QueryMerger` (Design §1), so the reason to leave it out isn't a difference in who serializes the
pipeline — it's that its scan is buffered ahead of that writer by a 10-slot channel plus
`try_buffered(2)` running two of its `nb_batches` re-executions concurrently (each independently
fanning out to `target_partitions` readers — worse than `QueryMerger`'s single scan today), and its
per-batch queries are typically aggregating, so the writer may not be the bottleneck the way it is
here. Forcing the same setting there is therefore an unmeasured trade, not a free one: since no
in-repo view constructs a `BatchPartitionMerger` today, and its `nb_batches`-times re-execution shape
needs its own measurement, this plan leaves the gap explicit — documented as the first adopter's
problem to solve, with a bounded-but-not-fully-serial scan — rather than guess at a wall-clock
regression along with the memory fix.

**Rejected: declare `ScanOrdering::Concatenated` on `MetricsView`** (the issue's proposal). Both
preconditions fail, and the failure mode is mixed rather than uniformly loud:
`sort_and_check_non_overlapping` (`partitioned_execution_plan.rs:83`), on the query path
(`get_scan_output_ordering`, consumed only by `MaterializedView::scan`), hard-errors whenever the
selected partitions include an overlapping adjacent pair — but where they don't (a single partition,
or a non-overlapping subset), the check passes, the declared ordering is attached, and the other
precondition (rows sorted within a file) still isn't true, so the query silently returns mis-ordered
rows instead of erroring. Either outcome is a reason to reject, and the silent one is the worse of
the two: it would land unnoticed on exactly the JIT `view_instance` queries that dominate `measures`
traffic (Design §3). (The merge path is unaffected either way: `QueryMerger`'s ordering is the
separate `with_merge_scan_ordering` field, which `MetricsView` never sets.) Rejected on correctness,
not on cost/benefit.

**Rejected: `MICROMEGAS_DATAFUSION_TARGET_PARTITIONS` as the fix.** It is a real backlog item for
user queries, but for merges the right number of readers is one, not "fewer", and a global knob
would drag user-query parallelism along with it.

**Rejected: lower the writer's 100 MB `in_progress_size` flush threshold.** It would shrink a
core-count-independent term at the cost of smaller row groups — worse compression and worse
row-group pruning, working against the 128 Ki row-group sizing chosen in #1392 §6. Revisit only if
Phase 2 shows the writer, not the scan, dominates.

**Ordering by construction vs. an enforced sort.** Chosen: ordering by construction, because it
is free (it falls out of the sequential scan) and needs no contract from any view. An enforced,
recorded sort would go further — exact pruning, `ORDER BY` elision, `PerFile` k-way merges — and
#1392 already built the machinery, so this is not a capability gap. It is rejected on **evidence**
(Design §3): a 6-hour query-audit sample shows the global `measures` view receiving ~2 queries while
per-process JIT instances take ~546, and the JIT partitions are too small (few row groups) and too
JIT-freshness-bound for a sort key to help. The costs are real too — a write-time full-partition
buffer on the every-minute path, scattered per-block dictionary runs across 7 of 14 columns, and
merge reader concurrency rising from `target_partitions` to k = 60 — but the decisive point is that
the population which would benefit is not the population being queried. Revisit if that ratio
changes; the key would be `(name, time)`, not `time`.

## Documentation

- `CHANGELOG.md` — Unreleased / Analytics: merges now scan their source partitions sequentially
  regardless of declared ordering; note that for a non-aggregating merge query this changes
  merged-partition row order (previously nondeterministic, now source-partition concatenation
  order) and restores row-group pruning for time-filtered queries via the resulting time-local row
  groups, that no sort guarantee is claimed, and that an aggregating (`GROUP BY`) merge query's
  output order stays nondeterministic as before. No SQL-surface change: no schema, view name, or
  UDF signature moves, and no `SCHEMA_VERSION` bump is needed (the file schema is untouched). No
  **Minor breaking change** clause either: the dispatch collapse touches only private methods on
  `QueryMerger`, and `ScanOrdering::concatenated_columns` is additive.
- `mkdocs/docs/admin/maintenance.md` — the `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` row already
  explains that daemon merges run on the shared unscoped pool; add a short note that merge scans
  are single-reader by design, so merge memory does not scale with host core count. Relevant to
  anyone sizing a daemon host.
- `mkdocs/docs/admin/monolith.md` — the same `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` row appears
  here too, and the monolith runs the maintenance role (merges included) under `--roles all`; add
  the same one-line note so anyone sizing a monolith host sees it.
- `mkdocs/docs/architecture/caching.md` (Phase 3 only) — add a bullet to "What is intentionally not
  cached in L1" for merge reads, mirroring the existing raw-telemetry-blocks entry: read exactly
  once during a merge, so caching them in L1 is pure cost.
- Rustdoc as listed in Implementation Steps 2-5 — the `view.rs` note is the one that
  prevents the "should this view declare an ordering to bound its merge?" question from being asked
  again, and the `ScanOrdering` note (step 3) is the one that keeps the two-strategy/three-variant
  distinction from being re-litigated the next time someone notices the enum has an arm the merge
  no longer branches on.

## Testing Strategy

- **New offline plan-shape test** (step 6) — the regression guard for the wrapper's own plan shape.
  Builds one session via `make_merge_session_context` and one via plain `make_session_context`
  (control), asserting one output partition with the former and more than one with the latter, so a
  future DataFusion upgrade that re-introduces the fan-out fails CI rather than production memory.
  This test does not call `QueryMerger::execute_merge_query` — that gap is what the next bullet
  covers. Both sessions pin `target_partitions` to 8 so the control assertion is meaningful on any
  CI runner regardless of core count.
- **New `execute_merge_query` concatenation-order test** (step 7) — drives the collapsed dispatch
  (Design §1b) end to end on the default `Unordered` ordering with `SELECT * FROM source`, over two
  fabricated single-row partitions, and asserts the drained rows come out in `begin_insert_time`
  order. This is the regression guard for both halves of the refactor that the plan-shape test
  cannot give: a future change that drops the `make_merge_session_context` call from
  `execute_merge_query`, and one that re-splits the undeclared path back onto
  `df.execute_stream()`.
- **Existing merge-path tests must stay green** — `blocks_view_merge_ordering_tests.rs`,
  `sql_batch_view_merge_ordering_tests.rs`, `per_file_scan_ordering_tests.rs`,
  `log_stats_ordering_tests.rs`, `sql_partition_spec_sort_order_tests.rs`. These cover the
  `Concatenated` and `PerFile` paths whose duplicated setting is being hoisted and whose dispatch
  arms are being merged; they are the proof the refactor is behavior-preserving for them. In
  particular `blocks_view_merge_ordering_tests.rs` is what proves the declared-ordering checks still
  run after `execute_concatenated_merge` starts taking them conditionally.
- **Full `cargo test` in `rust/`**, plus `cargo clippy --all-targets` and `cargo fmt --check`.
- **Local end-to-end**: `python3 local_test_env/ai_scripts/start_services.py`, generate enough
  telemetry for at least two one-minute partitions each of `measures` **and** `log_entries`, let the
  hourly task merge them, and confirm from `/tmp/daemon.log` that both merges complete and each
  merged partition's `num_rows` equals the sum of its inputs'
  (`micromegas-query "SELECT ... FROM list_partitions()"`). Covering both is what exercises the
  claim that this is a shared-path fix rather than a `measures` one.
- **Overlap claim, verifiable against real data** — the evidence that `Concatenated` was never
  available to `measures`:
  ```sql
  SELECT count(*) AS overlapping_adjacent_pairs
  FROM (
    SELECT max_event_time,
           lead(min_event_time) OVER (ORDER BY min_event_time, file_path) AS next_min
    FROM list_partitions()
    WHERE view_set_name = 'measures' AND view_instance_id = 'global'
  ) t
  WHERE next_min < max_event_time;
  ```
  Run it for `view_set_name = 'log_entries'` as well — §3b's claim is that the same two
  preconditions fail there, and this is the half of it that real data can settle.
  Ordered by `min_event_time` (tiebreak `file_path`) to match the adjacency
  `sort_and_check_non_overlapping` actually checks (`partitioned_execution_plan.rs:83-90` sorts by
  the leading-column `begin` bound, not by `begin_insert_time`). A non-zero count is the count of
  partition pairs the check would have errored on. Worth attaching to the issue reply.
- **Row-group pruning, before/after** (Design §2) — the query-side half of the change, and the
  baseline any future sort project should be judged against. A partition's internal row order is
  fixed at write (merge) time, so this cannot be measured on one partition before and after the
  change — it's a comparison across two *different* merged hourly partitions of the same view: one
  merged before this change (old interleaved clustering) and one merged after (new
  insert-time-concatenated clustering), matched for comparable volume and window width. Do it for
  `measures` and for `log_entries`; the latter is the one with real narrow-window traffic, even
  though §3b finds that traffic never lands on a merged partition. Run the same narrow-window query
  against each and compare bytes actually scanned:
  ```
  micromegas-query "SELECT count(*), min(value), max(name) FROM measures WHERE time BETWEEN ... AND ..." --begin 1h
  ```
  with a 1-minute window inside a 1-hour partition. Project representative payload columns, not just
  `time`: `bytes_scanned` is summed from bytes actually fetched from object storage
  (`reader_factory.rs:100-118`), and a bare `SELECT count(*) ... WHERE time BETWEEN` only projects
  `time`, so even with zero row-group pruning it fetches just the footer plus the `time` column
  chunks — a small fraction of a 14-column `metrics_table_schema()` partition, not a stand-in for
  "scanned the whole partition." With payload columns projected, expect the pre-change partition to
  scan bytes comparable to the whole partition; the post-change one, roughly the fraction of row
  groups covering the window.
  Note for the issue reply: existing merged partitions keep the old clustering until they age out of
  retention or are retired and rebuilt — this comparison only becomes moot once that has happened.
  Read scanned bytes from `log_entries WHERE target = 'flightsql_query_audit'` (the `bytes_scanned`
  field `query_audit.rs` stamps on every `QueryAuditRecord`), which is always on in a deployment; the
  per-file `parquet_read ... bytes=` line at `reader_factory.rs:115` is `debug!`-level and only a
  fallback when a per-file breakdown is needed.
- **Production validation** — Phase 2's five-hour sample, on the process-level gauges.

## Open Questions

1. **Core count of the affected daemon host.** The predicted improvement is linear in
   `target_partitions`; the actual factor cannot be stated without it. Not blocking — Phase 2
   measures the outcome directly.
2. **Is the residual allocator retention?** If Phase 2 shows `jemalloc_resident_bytes` spiking
   while `jemalloc_allocated_bytes` stays flat, the remaining footprint is jemalloc retention from
   streaming churn (consistent with the issue's "back to baseline within ~5 minutes"), and the next
   step is decay tuning, not further scan work. Worth knowing before opening Phase 3.
3. **Does anything outside this repo depend on merged-partition row order?** Nothing in-repo does
   (no `sort_order` is recorded for these views, and it was nondeterministic before), but a
   downstream consumer relying on the incidental order would see a change. Flagged for the issue
   reply rather than blocking.
