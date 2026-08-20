# Bounded-Memory Merge Scans Plan

[#1491](https://github.com/madesroches/micromegas/issues/1491) — the `measures` hourly merge dwarfs
every other view's and correlates with a ~700-800 MB daemon memory spike.

## Overview

**Change how partitions are merged during compaction: make every merge scan its source partitions
with one sequential reader instead of `target_partitions` concurrent ones.**

Today a merge's source scan is split into `target_partitions` byte-range file groups and executed by
that many concurrent Parquet readers, coalesced back to one stream — for a pipeline that can only
consume rows as fast as its single downstream writer. The scan-side working set is therefore
multiplied by the daemon host's core count for no throughput the writer can absorb. This is the
whole of the observed spike's scan component, and it is the same code path for every view;
`measures` is simply the one with 5-6× the data.

The fix is one session setting — `repartition_file_scans = false` — hoisted out of the two
ordering-aware merge paths that already set it and applied to **every** `QueryMerger` merge,
`Unordered` included. It needs no correctness contract from any view and it benefits every view on
the default merge route: `measures`, `log_entries`, `async_events`, `images`, `blocks`' plain-merger
fallback, and every `SqlBatchView` without a declared merge sort order.

Two things follow, and both are part of the change rather than side effects:

- **The merge dispatch collapses from three arms to two.** `Unordered` and `Concatenated` already
  build the identical scan (`build_unordered_or_concatenated_plan` puts every input partition in one
  file group for both) and after this change they also *execute* it identically — the only thing
  separating them is whether the resulting order is declared. The real taxonomy is **concatenate**
  (one sequential reader; output is the inputs back to back) and **sort-merge** (k readers collapsed
  by a `SortPreservingMergeExec` on a certified per-file order). See §2. The `ScanOrdering` enum
  keeps three variants — on the *query* path, "declare nothing" is a distinct and necessary state.
- **Merged partitions regain time-local row groups.** A sequential scan makes the merged file the
  concatenation of its inputs in insert-time order, restoring row-group pruning that today's
  interleave kills. Ordering by construction — no declared contract, no sort. See §3.

**The ordering question the issue actually asked — should `measures` declare a `ScanOrdering`, and
should these views be sorted? — is settled, and the answer to both is no.** That analysis, including
the query-audit evidence for `measures` and `log_entries`, lives in
`tasks/completed/1491_merge_scan_ordering_research.md`. It is not a prerequisite for anything here:
this plan's fix does not depend on any view's ordering and does not move any view toward one. The
one place it matters is calibration — §3's pruning win is real as a mechanism but, per that research,
neither view in the reporting deployment collects on it, which is why Phase 2 measures the mechanism
with a synthetic query rather than promising a latency win.

**Scope note on `log_entries`.** It is not an incidental beneficiary but the second-largest view on
the same path: 288 GB across 1468 global partitions with a largest single partition of ~1 GB, against
`measures`' 1.86 TB across 1324 with a largest of ~5 GB (measured with `list_partitions()` on the
reporting deployment). Same `BlockPartitionSpec`, same merger, same fan-out. Testing and measurement
below cover both views, which is what makes this a shared-path fix rather than a `measures` one.

## Current State

### The merge path

`create_merged_partition` (`rust/analytics/src/lakehouse/merge.rs:332`) logs
`merging {n} partitions sum_size={sum_size}` — the number quoted in the issue — then delegates to
`view.merge_partitions(...)`. `MetricsView` (`metrics_view.rs`) overrides neither `merge_partitions`
nor `get_scan_output_ordering`, so it takes the `View` trait default (`view.rs:99`): a `QueryMerger`
with query `SELECT * FROM source;` and `merge_scan_ordering: ScanOrdering::Unordered`
(`merge.rs:84`).

`QueryMerger::execute_merge_query` (`merge.rs:247`) branches on the ordering:

| Path | Optimizer settings | Execution |
|---|---|---|
| `Unordered` (`merge.rs:278`) | none | `df.execute_stream()` |
| `Concatenated` (`merge.rs:101`) | `repartition_file_scans = false` | build plan, check shape, `execute_stream(plan, ctx)` |
| `PerFile` (`merge.rs:157`) | `repartition_file_scans = false` + 4 more | build plan, 3 checks, `execute_stream(plan, ctx)` |

Only the `Unordered` path — the default every view above takes — leaves file-scan repartitioning
enabled.

### Why that makes the scan `target_partitions`-wide

The merge session context is built by `make_session_context` (`query.rs:256`) from a plain
`SessionConfig::default()`, so `target_partitions` is the host's core count and
`repartition_file_scans` is `true` (DataFusion default). Walking the plan for
`SELECT * FROM source;` over the `PartitionedTableProvider` registered at `merge.rs:264`:

1. `make_partitioned_execution_plan` with `ScanOrdering::Unordered` builds **one** file group holding
   all 60 partitions (`partitioned_execution_plan.rs:339`) — a single-partition `DataSourceExec`. No
   `Filter` survives: `source` is a bare table, not a `MaterializedView`, so `TableScanRewrite` skips
   it (`table_scan_rewrite.rs:36`), and `SELECT *` leaves no projection.
2. The physical optimizer's first rule, `OutputRequirements::new_add_mode`, wraps the root in an
   `OutputRequirementExec` with `Distribution::UnspecifiedDistribution`. **This is the part that
   matters**: it gives the scan a parent, so `ensure_distribution`'s
   `if dist_context.plan.children().is_empty() { return }` early-out does not fire.
3. `ensure_distribution` then hits the `repartition_file_scans && roundrobin_beneficial_stats`
   branch. `roundrobin_beneficial_stats` is `true` because the `Unordered` scan attaches no per-file
   statistics, so `num_rows` is `Precision::Absent`. It calls `DataSourceExec::repartitioned`, and
   `FileGroupPartitioner` splits the single file group into `target_partitions` byte-range groups
   (each partition file is ~33 MB, far above the 10 MB `repartition_file_min_size`).
4. `OutputRequirementExec` requires only `UnspecifiedDistribution` and returns
   `benefits_from_input_partitioning() == [false]`, so nothing collapses the fan-out. The ancillary
   node is removed by `OutputRequirements::new_remove_mode`, leaving a `target_partitions`-partition
   `DataSourceExec`.
5. `DataFrame::execute_stream` → `execute_stream(plan, ctx)` takes the `2.. =>` arm and wraps it in a
   `CoalescePartitionsExec`, which spawns **all** partitions concurrently into a
   `RecordBatchReceiverStream` of capacity `target_partitions`.

So the peak scan footprint is roughly `target_partitions × (row-group column chunks held by one
Parquet reader + decoded batches in flight)`, plus up to `target_partitions` queued batches in the
coalesce channel, plus whatever the L1 range cache holds behind those concurrent readers
(`L1_TOTAL_FETCH_PERMITS = 16` × `DEFAULT_MAX_COALESCED_GET_BYTES = 8 MB` in-flight, over a 200 MB
`BoundedMemoryBackend` budget). Every term except the L1 budget scales with core count, and all of
them scale with `sum_size` — exactly the shape the issue observed.

This also explains why the existing `Concatenated` path needs its `assert_single_partition` check
with the message *"This likely means repartition_file_scans did not take effect"* — that check exists
precisely because this fan-out is the default.

### Downstream of the scan, per merge

- `create_merged_partition` runs a full DataFusion `min`/`max` aggregate **per record batch**
  (`merge.rs:407` → `NamedColumnsTimeBounds::get_time_bounds`, `dataframe_time_bounds.rs:36`).
  Transient per batch, but thousands of plannings per hourly merge.
- The writer flushes when `arrow_writer.in_progress_size() > 100 MB` (`write_partition.rs:731`), with
  `max_row_group_row_count = 128 Ki` (`write_partition.rs:923`) and a `BufWriter` at
  `max_concurrency(2)`. So the write side contributes a bounded ~100 MB of in-progress row group plus
  part buffers — it does **not** scale with core count.

## Design

### 1. One setting, applied to every `QueryMerger` merge

`QueryMerger` gets the fix through one extracted helper, `make_merge_session_context(...)` — a thin
wrapper around `make_session_context` (`query.rs:256`) that applies the merge-only setting before
handing back the session, rather than each path building a session and then separately remembering to
set it.

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
// input partition, and this setting is load-bearing there too: left at `true`,
// `repartition_file_groups` (via `repartition_preserving_order`) splits each of those k
// per-partition groups into `target_partitions` byte-range groups whenever k < `target_partitions`,
// so this setting is what keeps `PerFile` at one reader per input file for its k-way ordered merge.
// Downstream parallelism is untouched: a merge query with a GROUP BY still gets its round-robin
// fan-out above the scan.
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

Resulting plan for `SELECT * FROM source;`: a one-file-group `DataSourceExec`, one output partition,
`execute_stream`'s `1 => plan.execute(0, ctx)` arm, one Parquet reader at a time.

**Deliberately not added:** an `assert_single_partition` check on the undeclared path. Unlike the
other two, it carries no ordering claim to protect, and it is the path a `SqlBatchView` with no
declared merge sort order uses with an arbitrary user-supplied `merge_partitions_query`
(`sql_batch_view.rs:194`). A `GROUP BY` in such a query legitimately plans to multiple partitions,
and coalescing it is correct. With `repartition_file_scans = false` those queries keep their
aggregation parallelism via round-robin repartition *above* a sequential scan — the desirable shape,
not something to bail on.

**Not changed: `BatchPartitionMerger`.** It is the other `PartitionMerger` implementation — the
bounded-memory fallback for merges whose whole output can't be held at once (`sql_batch_view.rs:179`);
no in-repo view constructs one today, though it's one `session_configurator` call away. It keeps its
current scan shape, unbounded and all: `batch_partition_merger.rs:106-190` builds its own session via
`make_session_context`, registers an `Unordered` provider over the *whole* partition set, and
re-executes the same `$begin`/`$end`-templated query once per batch — `nb_batches` times,
`try_buffered(2)` — with `repartition_file_scans` at its default. Its output feeds the same single
writer task as `QueryMerger` (`create_merged_partition` drains either merger's stream through the
same `mpsc::channel(1)`), so the two are not asymmetric in *who* drains the scan. The asymmetry is in
how much is buffered ahead of that writer: a 10-batch `RecordBatchReceiverStreamBuilder` channel plus
`try_buffered(2)` running two re-executions concurrently, each independently fanning out to
`target_partitions` readers. Per-batch merge queries are also typically aggregating (batching only
pays when a batch's result is small enough to hold, which usually means a `GROUP BY`), so the writer
may not be the bottleneck the way it is for `QueryMerger`'s single full-size scan. Forcing the same
setting there is an unmeasured trade on a shape this plan hasn't exercised, so the gap is recorded
rather than guessed at: the first view to adopt `BatchPartitionMerger` inherits its current
unbounded-scan cost, and bounding it without a wall-clock regression — e.g. a small pinned
`target_partitions` rather than a full serial scan repeated `nb_batches` times — is that adopter's
work.

**Also not changed: partition-*creation* extract queries**, for the same reason.
`SqlBatchView::make_batch_partition_spec` (`sql_batch_view.rs:256`) builds its session with the same
plain `make_session_context`, and `SqlPartitionSpec::write` streams that query into
`write_partition_from_rows` through a single-writer `mpsc::channel(1)` (`sql_partition_spec.rs:149`)
— structurally the same shape this plan's rationale rests on. But a view's extract query is typically
aggregating (it does the `GROUP BY` or windowing that shapes a partition, not a bare `SELECT *`), so
the writer is less likely to be the bottleneck and scan parallelism less likely to be wasted. This
matters for Phase 2: these extract scans run in the same daemon whose memory the before/after sample
measures, so they are a potential confounder in that comparison.

### 2. Two merge strategies, not three

With `repartition_file_scans = false` applied to every `QueryMerger` merge, the three arms of
`execute_merge_query`'s `match &self.merge_scan_ordering` (`merge.rs:277-296`) produce two distinct
physical shapes:

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
order left undeclared. Before this change the two arms at least *executed* differently; after it,
keeping them apart is bookkeeping masquerading as a strategy — and it is exactly the distinction
that made this fix look like it needed a declared ordering in the first place.

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
  "deliberately not added" reasoning still holds.
- `ordering_honored` stays `true` in the undeclared case, matching today's `Unordered` arm and the
  field's existing rustdoc ("Always `true` when no ordering was declared").

Rename `execute_per_file_merge` to `execute_sorted_merge` so the two method names are the two
strategies rather than one strategy and one scan shape. `ScanOrdering::PerFile` keeps its name — it
describes the scan shape, which is what that enum is for.

**The enum stays at three variants.** `ScanOrdering` has a second consumer,
`MaterializedView::scan` on the user-query path (`materialized_view.rs:94`), where `Unordered` means
"declare no ordering to DataFusion" and is neither redundant nor expressible as `Concatenated`. The
collapse is in the merge dispatch and in the vocabulary the docs use, not in the type.
`make_partitioned_execution_plan` already encodes the same two-shape split for both consumers: its
`PerFile`-that-does-not-certify arm degrades to `Unordered` (`partitioned_execution_plan.rs:290-300`),
which is precisely "fall back from sort-merge to concatenate".

### 3. What this changes about merged partitions

For a non-aggregating merge query — the default `SELECT * FROM source` shape most views use — merged
row order becomes deterministic: file concatenation in the order `create_merged_partition` already
sorts them (`filtered_partitions.sort_by_key(|p| p.begin_insert_time())`, `merge.rs:371`), instead of
today's nondeterministic byte-range interleave from `CoalescePartitionsExec`. This does not extend to
aggregating merges: a `GROUP BY` merge query (§5 — `processes`, `streams`, and `log_stats`'s
unordered fallback) still gets round-robin and hash repartitioning above the sequential scan and is
coalesced back nondeterministically, so those views' merged row order stays as nondeterministic as it
is today.

For the non-aggregating case that is not cosmetic — **it restores row-group pruning on the merged
partition**, which today is effectively dead:

- Today, ~`target_partitions` readers sit at byte offsets spread across the whole hour of source
  files, and `CoalescePartitionsExec` interleaves their batches in completion order. Every 128 Ki-row
  row group in the merged hourly partition therefore holds rows from ~`target_partitions` different
  minutes spread across the hour, so its `time` min/max spans nearly the full hour. A query filtering
  `time BETWEEN a AND b` over a narrow window pushes its predicate into the scan (`make_time_filter`,
  `metrics_view.rs:215` → `filters_to_predicate` → `ParquetSource::with_predicate`) and prunes
  **nothing** — it reads the entire hourly partition.
- After the change, the merged file is the concatenation of its 60 one-minute inputs in insert-time
  order, so row groups are time-local at roughly minute granularity and a narrow-window query touches
  only the row groups covering it.

The correlation is insert time, not event time, so this is clustering rather than a guarantee: a
stream with a long block-fill interval contributes events older than its insert window, which widens
some row groups' `time` range. Pruning improves substantially without becoming exact.

**How much this is worth is deployment-dependent, and in the reporting deployment it is close to
zero for both views.** Three limits, all quantified in
`tasks/completed/1491_merge_scan_ordering_research.md`: pruning can never select less than one row
group (128 Ki rows, `write_partition.rs:923`), so the benefit scales with row groups per partition —
large on merged hourly partitions, negligible on fresh one-minute ones; it only pays when a query's
window is materially narrower than the partition it lands in; and a view collects at all only in
proportion to how much of its traffic reaches merged global partitions. Global `measures` fails the
third test (~2 queries in 6 hours) and global `log_entries` fails the second (heavily queried, but no
sampled query asks for a window narrower than the partition it reads). The mechanism is real and
free; treat it as such rather than as a promised latency win, and see Phase 2 step 10 for how it is
measured.

No guarantee is *recorded*: `get_merged_partition_sort_order` still returns `None` for these views, so
no query plan may assume it, and nothing certifies a `sort_order`. This is ordering by construction —
free, contract-free, and applying to every view on the default merge path.

### 4. Expected improvement, and how to size it

The scan-side component of the spike is linear in `target_partitions`; this change takes it to `1×`.
On a 16-core daemon that is a ~16× reduction of that component. What remains is
core-count-independent:

- writer in-progress row group: ≤ ~100 MB (`write_partition.rs:731`)
- L1 range-cache budget: 200 MB (`MICROMEGAS_OBJECT_CACHE_L1_MB`, `l1_store.rs:26`)
- one Parquet reader's row group + in-flight coalesced GETs: tens of MB

so the predicted post-change hourly spike is in the low hundreds of MB rather than 700-800 MB. That
is a model, not a measurement — Phase 2 confirms it against the same five-hour sampling the issue
used. The honest caveat: `used_memory` is a **host-wide** `sysinfo` gauge
(`telemetry-sink/src/system_monitor.rs:90`), not this process's footprint. The daemon already enables
`jemalloc-metrics` (`telemetry-maintenance-srv/Cargo.toml:17`), so `process_resident_bytes`,
`jemalloc_allocated_bytes`, and `jemalloc_resident_bytes` are available and are what the before/after
comparison should use. A gap between `jemalloc_allocated_bytes` (flat) and `jemalloc_resident_bytes`
(spiking) would say the residual is allocator retention from streaming churn, not a live working set
— a different problem with a different fix.

### 5. Cost: merge wall-clock

Decode and decompression of the source files stop overlapping across cores. The pipeline's serial
bottleneck is already the single writer task (LZ4 encoding of the output on one thread, fed through
an `mpsc::channel(1)`), so the added cost is the scan's serial time no longer hiding behind it —
expect roughly 1.5-2× on the largest merges, not `target_partitions`×. The `blocks` view has run its
ordered merges on exactly this sequential shape in production since #1340, so the shape is not novel.
Phase 2 measures it; the hourly budget for one view's merge is generous, and the daemon materializes
views strictly sequentially (`materialize_all_views`, `public/src/servers/maintenance.rs`), so a
slower merge delays only later views in the same pass.

That 1.5-2× bound assumes the single writer stays the bottleneck, which needs the merge output to be
close in size to the scan input. It does **not** hold for aggregating merges, where a `GROUP BY`
shrinks the output by orders of magnitude: `processes` and `streams` (`SqlBatchView`s whose merge
query is a `GROUP BY` aggregate on the default undeclared route — `processes_view.rs:47-68`,
`streams_view.rs:41-53`) and `log_stats`'s merge whenever it falls back to the plain merger because an
input hasn't yet certified its `sort_order` (the gate at `partitioned_execution_plan.rs:291` — true of
every existing partition until re-materialization). There the writer is idle most of the time, so the
now-serial scan is the bottleneck: expect closer to `target_partitions`× on those merges. In practice
that's bounded by `processes`/`streams`' small partition volumes, and by `log_stats` shrinking to its
ordered `PerFile` path as partitions re-materialize.

`BatchPartitionMerger` is untouched (§1), so this estimate does not apply to it either way.

If the wall-clock regression turns out to matter, the escape hatch is the already-scoped backlog item
`tasks/backlog/datafusion_target_partitions_config.md`
(`MICROMEGAS_DATAFUSION_TARGET_PARTITIONS`), which lets an operator trade memory for parallelism
globally. That knob is **not** part of this plan: it does not fix the merge path (a merge wants *one*
reader, not "fewer"), and it would silently affect user queries too.

### 6. Optional, separable: don't populate L1 from merge reads (Phase 3)

A merge reads each source file exactly once, and those files are retired and deleted immediately
after. Caching them is pure cost: 2 GB streamed through a 200 MB LRU (`BoundedMemoryBackend`) both
holds 200 MB and evicts everything the daemon's own queries had warm. `LakehouseContext` builds one
`ReaderFactory` over an `l1_wrap`ped store (`lakehouse_context.rs:88` and `:114`) and hands it to
every consumer.

Shape: add a second, unwrapped `ReaderFactory` on `LakehouseContext` (reading
`lake.blob_storage.inner()` directly, sharing the same `MetadataCache` — footer metadata *is* worth
caching) and have `QueryMerger::execute_merge_query` use it for the `source` table. This is a separate
change with a separate measurement; it is not needed to close #1491 and should not be folded into
Phase 1's before/after. Same reasoning `mkdocs/docs/architecture/caching.md`'s "What is intentionally
not cached in L1" section already gives for raw telemetry blocks.

## Implementation Steps

### Phase 1 — Bounded merge scan (the fix)

1. `rust/analytics/src/lakehouse/merge.rs`: extract `pub async fn make_merge_session_context(...) ->
   Result<SessionContext>` with the comment from §1 — same parameters as `make_session_context`,
   calling it internally and applying `repartition_file_scans = false` before returning. Change
   `QueryMerger::execute_merge_query` to call this wrapper instead of `make_session_context` directly,
   before the `match self.merge_scan_ordering`. `BatchPartitionMerger::execute_merge_query`
   (`batch_partition_merger.rs`) is **not** changed (§1).
2. Same file: remove the assignment from `execute_concatenated_merge` and from
   `execute_per_file_merge`'s optimizer block; update both methods' rustdoc, which currently describes
   setting it as part of their own path (`merge.rs:97-100`, `merge.rs:150-156`), to reference the
   shared setting instead.
3. `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`: add
   `ScanOrdering::concatenated_columns(&self) -> Option<&[ScanSortColumn]>` with the rustdoc from §2,
   and extend the `ScanOrdering` enum's own rustdoc to name the two scan shapes (single sequential
   file group vs. one group per file) and to say that `Unordered` and `Concatenated` are the same
   shape differing only in what they declare.
4. `merge.rs`: collapse `execute_merge_query`'s three-arm `match` to the two arms in §2. Change
   `execute_concatenated_merge`'s signature to take `declared: Option<&[ScanSortColumn]>`, gate
   `assert_single_partition` and the `ordering_honored` plan-string check on `declared.is_some()`
   (returning `ordering_honored: true` when it is `None`), and delete the `ScanOrdering::Unordered`
   arm's inline `df.execute_stream()` body — the concatenating path now builds the plan explicitly for
   both. Rename `execute_per_file_merge` to `execute_sorted_merge`. Update
   `MergeQueryResult::ordering_honored`'s rustdoc (`merge.rs:38-46`), which describes the field
   per-variant, to describe it per-strategy.
5. `rust/analytics/src/lakehouse/view.rs`: extend the `get_scan_output_ordering` rustdoc with a short
   note that a bounded merge scan is *not* a reason to declare an ordering — every merge is sequential
   regardless — so the next view author facing a big merge does not repeat #1491's reasoning.
6. `rust/analytics/tests/merge_scan_partitioning_tests.rs` (new): offline plan-shape test, modelled on
   `log_stats_ordering_tests.rs`'s `make_offline_lakehouse_context` helper (in-memory object store, no
   DB, lazily-connected) with fabricated `Partition`s (`file_size` above the 10 MB
   `repartition_file_min_size`) registered as the source table. Build two sessions — one from
   `make_merge_session_context(...)`, one from plain `make_session_context(...)` (control) — pinning
   `target_partitions` to 8 on each returned `SessionContext`, matching the precedent in
   `sql_batch_view_merge_ordering_tests.rs` and `log_stats_ordering_tests.rs`, so the control assertion
   doesn't silently no-op on a low-core-count CI runner. `create_physical_plan()` for
   `SELECT * FROM source` against each and assert:
   - with `make_merge_session_context`: `partition_count() == 1`
   - with plain `make_session_context` (control, guarding that the test is meaningful):
     `partition_count() > 1`

   Asserting against the wrapper itself, rather than a bare config-mutating helper the test drives by
   hand, guards the wrapper's own plan shape. It does not exercise
   `QueryMerger::execute_merge_query` — step 7 covers that.
7. Same test file: cover the step-4 collapse by driving `QueryMerger::execute_merge_query` itself over
   the offline lakehouse context with the default `Unordered` ordering and a `SELECT * FROM source`
   query, and assert the drained stream's rows are the two input partitions concatenated in the order
   they were passed to `execute_merge_query` (this drives the merger directly, below
   `create_merged_partition`'s `begin_insert_time` sort at `merge.rs:371` — production
   `begin_insert_time` ordering comes from that caller, not from anything this test asserts). Unlike
   the plan-shape-only tests in this file and in `blocks_view_merge_ordering_tests.rs`, draining the
   stream means the fabricated `file_path`s must resolve to real objects: write two real single-row
   Parquet files into the offline context's `BlobStorage` (the `AsyncArrowWriter` +
   `object_store::buffered::BufWriter` pattern from `write_partition_tests.rs`'s `make_arrow_writer`,
   one row each with distinguishable values), and set each fabricated `Partition::file_size` to the
   written object's actual byte size — an inflated `file_size` makes `ParquetOpener`'s footer read run
   past the end of the object. This pins the collapsed dispatch's concatenation semantics; step 6's
   plan-shape test is what actually guards the wrapper regression (a fabricated `file_size` large
   enough to trigger `repartition_file_scans` would also fan the file-scan step out to multiple
   partitions, so a two-row fixture at this scale cannot detect either a reverted `df.execute_stream()`
   path or a dropped wrapper call — see step 6).

### Phase 2 — Measure and answer the issue

8. `merge.rs`: extend `create_merged_partition` to log a completion line next to the existing
   `sum_size` line — elapsed wall-clock only, no output `file_size` (`write_partition_from_rows`
   returns `Result<()>`; the size is consumed internally by `insert_partition` and never returned, and
   plumbing it out would mean changing that function's return type across its 6 production call sites,
   which is out of scope here) — so before/after is queryable from `log_entries` without new
   instrumentation. (Deliberately a log line, not a metric: it pairs with the `sum_size` line the issue
   already quotes.) Output size for the before/after comparison comes from `list_partitions()` instead.
9. Collect a five-hour sample matching the issue's, using `process_resident_bytes` /
   `jemalloc_allocated_bytes` / `jemalloc_resident_bytes` rather than host `used_memory`, plus the new
   duration line.
10. Measure row-group pruning before/after: run the same narrow-window query — projecting
    representative payload columns, not just `time`, so `bytes_scanned` is comparable to partition
    size (see Testing Strategy) — against a pre-change and a post-change merged hourly partition and
    compare `bytes_scanned` from `log_entries WHERE target = 'flightsql_query_audit'`. Do this for both
    `measures` and `log_entries`. This is a *mechanism* check: it should show the improvement for a
    **synthetic** narrow-window query on both views even though no real query in this deployment's
    sampled traffic collects on it (§3). If the synthetic query shows no improvement, §3's mechanism
    claim is wrong and should be struck rather than explained away.
11. Post the answer on #1491: the win was never in the ordering — `Concatenated` is unsafe for
    `measures` and `PerFile` would be a no-op (link
    `tasks/completed/1491_merge_scan_ordering_research.md` for the full argument and the same verdict
    for `log_entries`) — and here is the measured before/after, memory and pruning. Note that the fix
    is shared, so the issue closes for every view on the default merge path rather than for `measures`
    alone.
12. File the follow-up issues the research's evidence points at, neither about ordering:
    - `measures` queries through per-process JIT view instances spend their time in JIT freshness
      checking over requested ranges far wider than the process's data, not in scanning.
    - Global `log_entries` dashboard queries scanned 67.6 GB over 6 hours, 62.6 GB of it from the 292
      refreshes asking for a window wider than 2 minutes, over a 288 GB view.

    Both are larger wins in the sampled workload than anything here.

### Phase 3 — Optional, only if Phase 2 leaves the L1 component significant

13. `rust/analytics/src/lakehouse/lakehouse_context.rs`: add an uncached `ReaderFactory` sharing the
    existing `MetadataCache`.
14. `merge.rs`: use it for the `source` table.
15. Re-measure independently.

## Files to Modify

- `rust/analytics/src/lakehouse/merge.rs` — the setting, the `make_merge_session_context` extraction,
  the two-arm dispatch collapse (`execute_concatenated_merge` taking `Option<&[ScanSortColumn]>`,
  `execute_per_file_merge` → `execute_sorted_merge`), rustdoc, the completion log line
- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` — `ScanOrdering::concatenated_columns`
  and the enum's two-scan-shape rustdoc
- `rust/analytics/src/lakehouse/view.rs` — `get_scan_output_ordering` rustdoc note
- `rust/analytics/tests/merge_scan_partitioning_tests.rs` — new plan-shape regression test plus the
  `execute_merge_query` concatenation-order test
- `rust/analytics/src/lakehouse/lakehouse_context.rs` — Phase 3 only
- `CHANGELOG.md` — Unreleased / Analytics entry
- `mkdocs/docs/admin/maintenance.md`, `mkdocs/docs/admin/monolith.md` — see Documentation
- `mkdocs/docs/architecture/caching.md` — Phase 3 only

Not modified: `metrics_view.rs` (no view-level change is needed or correct — see the research doc),
`batch_partition_merger.rs`, and `sql_batch_view.rs` / `sql_partition_spec.rs` (§1).

## Trade-offs

**Sequential merge scan vs. bounded merge memory.** Chosen: bounded memory. A merge's consumer is one
writer task behind an `mpsc::channel(1)`; scan parallelism buys throughput the pipeline cannot absorb
while multiplying the reader working set by the core count. The cost is quantified in §5 and measured
in Phase 2.

**Fix the shared merge path vs. patch `measures`.** Chosen: the shared path — `QueryMerger`, not a
`measures`-specific change. Every view on the default undeclared route has the same fan-out;
`measures` is merely the one large enough to make it visible. A `measures`-only change would leave
`log_entries`, `async_events`, `images`, and every unsorted `SqlBatchView` with the same behavior —
and there is no correct `measures`-only change to make (see the research doc). `log_entries` is the
concrete case: same write path, same merger, 288 GB and a ~1 GB largest partition.

**Collapse the merge dispatch to two strategies vs. leave three arms.** Chosen: two (§2). Once
`repartition_file_scans = false` is unconditional, the two concatenating arms execute the same scan;
keeping them apart leaves a distinction that no longer corresponds to anything the merge does — and it
is the distinction that made this fix look like it needed a declared ordering in the first place. The
cost is that the undeclared path now builds its physical plan explicitly instead of calling
`df.execute_stream()`; that is the same two steps, so the cost is a slightly longer method, not a
behavior change.

**Rejected: collapse `ScanOrdering` to two variants.** The tempting follow-through — delete
`Unordered`, or fold it into `Concatenated { columns: vec![] }` — breaks the query path.
`get_scan_output_ordering` is consumed by `MaterializedView::scan`, where "declare no ordering to
DataFusion" is a real state that a `Concatenated` with an empty column list does not express: that
variant would still run `sort_and_check_non_overlapping` (`partitioned_execution_plan.rs:83`),
imposing a non-overlap requirement on every view that today declares nothing. Three variants describe
the scan; two strategies describe the merge; those are different counts of different things, and only
the second is over-counted today.

**Rejected: bounding `BatchPartitionMerger`'s scan in the same change.** It drains into the same
single writer task, so the reason isn't a difference in who serializes the pipeline — it's that its
scan is buffered ahead of that writer by a 10-slot channel plus `try_buffered(2)` running two of its
`nb_batches` re-executions concurrently (each independently fanning out to `target_partitions`
readers), and its per-batch queries are typically aggregating, so the writer may not be the
bottleneck. Forcing the same setting there is an unmeasured trade. No in-repo view constructs one
today, so the gap is left explicit and documented as the first adopter's problem (§1).

**Rejected: `MICROMEGAS_DATAFUSION_TARGET_PARTITIONS` as the fix.** A real backlog item for user
queries, but for merges the right number of readers is one, not "fewer", and a global knob would drag
user-query parallelism along with it.

**Rejected: lower the writer's 100 MB `in_progress_size` flush threshold.** It would shrink a
core-count-independent term at the cost of smaller row groups — worse compression and worse row-group
pruning, working against the 128 Ki row-group sizing chosen in #1392 §6. Revisit only if Phase 2 shows
the writer, not the scan, dominates.

**Ordering by construction vs. an enforced sort.** Chosen: ordering by construction, because it falls
out of the sequential scan for free and needs no contract from any view. An enforced, recorded sort
would go further — exact pruning, `ORDER BY` elision, `PerFile` k-way merges — and #1392 already built
the machinery, so this is not a capability gap. It is rejected on evidence, recorded in full in
`tasks/completed/1491_merge_scan_ordering_research.md`.

## Documentation

- `CHANGELOG.md` — Unreleased / Analytics: merges now scan their source partitions sequentially
  regardless of declared ordering; note that for a non-aggregating merge query this changes
  merged-partition row order (previously nondeterministic, now source-partition concatenation order)
  and restores row-group pruning for time-filtered queries via the resulting time-local row groups,
  that no sort guarantee is claimed, and that an aggregating (`GROUP BY`) merge query's output order
  stays nondeterministic as before. No SQL-surface change: no schema, view name, or UDF signature
  moves, and no `SCHEMA_VERSION` bump is needed (the file schema is untouched). No **Minor breaking
  change** clause either: the dispatch collapse touches only private methods on `QueryMerger`, and
  `ScanOrdering::concatenated_columns` is additive.
- `mkdocs/docs/admin/maintenance.md` — the `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` row already
  explains that daemon merges run on the shared unscoped pool; add a short note that merge scans are
  single-reader by design, so merge memory does not scale with host core count. Relevant to anyone
  sizing a daemon host.
- `mkdocs/docs/admin/monolith.md` — the same row appears here too, and the monolith runs the
  maintenance role (merges included) under `--roles all`; add the same one-line note.
- `mkdocs/docs/architecture/caching.md` (Phase 3 only) — add a bullet to "What is intentionally not
  cached in L1" for merge reads, mirroring the existing raw-telemetry-blocks entry.
- Rustdoc as listed in steps 2-5 — the `view.rs` note is the one that prevents the "should this view
  declare an ordering to bound its merge?" question from being asked again, and the `ScanOrdering`
  note (step 3) keeps the two-strategy/three-variant distinction from being re-litigated the next time
  someone notices the enum has an arm the merge no longer branches on.

## Testing Strategy

- **New offline plan-shape test** (step 6) — the regression guard for the wrapper's own plan shape.
  One session via `make_merge_session_context` and one via plain `make_session_context` (control),
  asserting one output partition with the former and more than one with the latter, so a future
  DataFusion upgrade that re-introduces the fan-out fails CI rather than production memory. Both
  sessions pin `target_partitions` to 8 so the control assertion is meaningful on any CI runner.
- **New `execute_merge_query` concatenation-order test** (step 7) — drives the collapsed dispatch (§2)
  end to end on the default `Unordered` ordering with `SELECT * FROM source`, over two real single-row
  Parquet files written into the offline context's `BlobStorage` (with `Partition::file_size` set to
  each file's actual byte size, since the stream is actually drained here), asserting the drained rows
  come out in the order the partitions were passed to `execute_merge_query` — `begin_insert_time`
  ordering itself is applied by `create_merged_partition`'s caller-side sort, not by anything under
  test here. At this fixture size the file group is far below `repartition_file_min_size`, so it does
  not exercise file-scan repartitioning either way; it pins the collapsed dispatch's concatenation
  semantics, while the plan-shape test (step 6) is the actual regression guard for the wrapper.
- **Existing merge-path tests must stay green** — `blocks_view_merge_ordering_tests.rs`,
  `sql_batch_view_merge_ordering_tests.rs`, `per_file_scan_ordering_tests.rs`,
  `log_stats_ordering_tests.rs`, `sql_partition_spec_sort_order_tests.rs`. These cover the
  `Concatenated` and `PerFile` paths whose duplicated setting is being hoisted and whose dispatch arms
  are being merged; they are the proof the refactor is behavior-preserving. In particular
  `blocks_view_merge_ordering_tests.rs` is what proves the declared-ordering checks still run after
  `execute_concatenated_merge` starts taking them conditionally.
- **Full `cargo test` in `rust/`**, plus `cargo clippy --all-targets` and `cargo fmt --check`.
- **Local end-to-end**: `python3 local_test_env/ai_scripts/start_services.py`, generate enough
  telemetry for at least two one-minute partitions each of `measures` **and** `log_entries`, let the
  hourly task merge them, and confirm from `/tmp/daemon.log` that both merges complete and each merged
  partition's `num_rows` equals the sum of its inputs'
  (`micromegas-query "SELECT ... FROM list_partitions()"`). Covering both is what exercises the claim
  that this is a shared-path fix rather than a `measures` one.
- **Row-group pruning, before/after** (§3) — the query-side half of the change. A partition's internal
  row order is fixed at write (merge) time, so this cannot be measured on one partition before and
  after; it is a comparison across two *different* merged hourly partitions of the same view — one
  merged before the change (old interleaved clustering) and one after (new insert-time concatenation),
  matched for comparable volume and window width. Do it for `measures` and `log_entries`. Run the same
  narrow-window query against each and compare bytes actually scanned:
  ```
  micromegas-query "SELECT count(*), min(value), max(name) FROM measures WHERE time BETWEEN ... AND ..." --begin 1h
  ```
  with a 1-minute window inside a 1-hour partition. Project representative payload columns, not just
  `time`: `bytes_scanned` is summed from bytes actually fetched from object storage
  (`reader_factory.rs:100-118`), and a bare `SELECT count(*) ... WHERE time BETWEEN` only projects
  `time`, so even with zero pruning it fetches just the footer plus the `time` column chunks — a small
  fraction of a 14-column `metrics_table_schema()` partition, not a stand-in for "scanned the whole
  partition." With payload columns projected, expect the pre-change partition to scan bytes comparable
  to the whole partition; the post-change one, roughly the fraction of row groups covering the window.
  Note for the issue reply: existing merged partitions keep the old clustering until they age out of
  retention or are retired and rebuilt. Read scanned bytes from
  `log_entries WHERE target = 'flightsql_query_audit'` (the `bytes_scanned` field `query_audit.rs`
  stamps on every `QueryAuditRecord`), which is always on in a deployment; the per-file
  `parquet_read ... bytes=` line at `reader_factory.rs:115` is `debug!`-level and only a fallback when
  a per-file breakdown is needed.
- **Production validation** — Phase 2's five-hour sample, on the process-level gauges.

## Open Questions

1. **Resolved: core count of the affected daemon host.** The predicted improvement is linear in
   `target_partitions`, so the exact factor cannot be stated in advance — and it does not need to be.
   Phase 2 measures the outcome directly on the host that matters, which is the number the change is
   judged on.
2. **Is the residual allocator retention?** If Phase 2 shows `jemalloc_resident_bytes` spiking while
   `jemalloc_allocated_bytes` stays flat, the remaining footprint is jemalloc retention from streaming
   churn (consistent with the issue's "back to baseline within ~5 minutes"), and the next step is decay
   tuning, not further scan work. Worth knowing before opening Phase 3.
3. **Does anything outside this repo depend on merged-partition row order?** Nothing in-repo does (no
   `sort_order` is recorded for these views, and it was nondeterministic before), but a downstream
   consumer relying on the incidental order would see a change. Flagged for the issue reply rather than
   blocking.
