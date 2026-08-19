# Bounded-Memory Merge Scans Plan

[#1491](https://github.com/madesroches/micromegas/issues/1491) — `measures` hourly merge dwarfs
every other view's and correlates with a ~700-800 MB daemon memory spike.

## Overview

The issue asks two questions: is `measures`/`MetricsView` a good candidate for a
`ScanOrdering` declaration, and how much memory would that save?

**Answers: no, and none.** `measures` partitions satisfy neither precondition of
`ScanOrdering::Concatenated` (rows inside a partition are not time-ordered; partition event-time
ranges overlap heavily), so declaring it would not be a mis-optimization — it would make
`sort_and_check_non_overlapping` return a hard error and **fail every `measures` query** (global and
every JIT `view_instance`). `ScanOrdering::PerFile` would silently degrade to `Unordered`, because
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
`QueryMerger` merge, `Unordered` included, and to `BatchPartitionMerger` (the other
`PartitionMerger` implementation) alongside it. That is one small setting, it needs no correctness
contract from any view, and it benefits every view whose merge goes through either path —
`measures`, `log_entries`, `async_events`, `images`, `blocks`' plain-merger fallback, every
`SqlBatchView` without a declared merge sort order, and `BatchPartitionMerger`'s bounded-memory
fallback (currently unused in-repo, but documented as the escape hatch for a view whose merge
output doesn't fit in memory in one pass).

It also buys a **query** win in mechanism, though a modest one in practice. Today's interleaved
merge scatters each source minute across every row group of the merged hourly partition, so `time`
row-group statistics span the whole hour and a narrow-window query prunes nothing. A sequential scan
makes the merged file the concatenation of its inputs in insert-time order, which restores
time-local row groups — ordering by construction, with no declared contract and no sort. How much
that is worth depends on who queries the global `measures` view; Design §3 answers that from a
6-hour query-audit sample, and the same evidence is why this plan stops short of a *recorded* sort
rather than merely deferring one.

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
  (`write_partition.rs:730`), with `max_row_group_row_count = 128 Ki`
  (`write_partition.rs:923`) and a `BufWriter` at `max_concurrency(2)`. So the write side
  contributes a bounded ~100 MB of in-progress row group plus part buffers — it does **not** scale
  with core count.

### Why `measures` cannot declare an ordering

`View::get_scan_output_ordering`'s contract (`view.rs:150-209`) requires, for
`Concatenated { columns, bounds }`, that (a) rows within each partition file are already sorted by
`columns` and (b) partition ranges on the leading column do not overlap. `get_scan_output_ordering`
has exactly one consumer: `MaterializedView::scan` (`materialized_view.rs:94`), the user-query path.
`QueryMerger`'s merge-side ordering is a separate, independently-set field
(`with_merge_scan_ordering`, `merge.rs:87-90`, defaulting to `Unordered`, `merge.rs:80`), and
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
  max_event_time]`. `sort_and_check_non_overlapping` (`partitioned_execution_plan.rs:83`) returns
  `DataFusionError::Internal` on the first overlapping adjacent pair — it does not degrade
  gracefully. **Declaring `Concatenated` over `time` would break every `measures` query outright.**
- **`insert_time` + `OrderingBounds::InsertTime`** satisfies (b) by construction but still fails
  (a), for the same `buffer_unordered` reason. This is what `blocks_view` gets right and `measures`
  cannot: `BlocksView`'s extract query ends in `ORDER BY blocks.insert_time, blocks.block_id`
  (`blocks_view.rs:68`), and its `merge_partitions` additionally requires every input to *carry*
  the recorded `sort_order` before taking the ordered path (`blocks_view.rs:198`).
- **`PerFile`** is gated by `Partition::certifies_sort_order` inside
  `make_partitioned_execution_plan` (`partitioned_execution_plan.rs:291`). `BlockPartitionSpec`
  passes `None` for `sort_order` (`block_partition_spec.rs:92`), so no `measures` partition
  certifies anything and the declaration would degrade to `Unordered` — a pure no-op.

## Design

### 1. One setting, applied to every merge

There are two `PartitionMerger` implementations: `QueryMerger` (the default, ordering-aware path)
and `BatchPartitionMerger` (the bounded-memory fallback used when a merge's whole output can't be
held in memory at once — `sql_batch_view.rs:179`; no in-repo view constructs one today, but it's
one `session_configurator` call away, and its scan is currently unbounded too:
`batch_partition_merger.rs:106-190` builds its own session via `make_session_context`, registers an
`Unordered` `PartitionedTableProvider`, and calls `execute_stream()` with `repartition_file_scans`
at its default `true` — and it runs two batches concurrently (`try_buffered(2)`), so its scan side
is up to `2 × target_partitions` readers, worse than `QueryMerger`'s). Both get the fix by sharing
one extracted helper, `configure_merge_session(&ctx)` (also needed for the plan-shape test in step
5), rather than hoisting the setting into `QueryMerger::execute_merge_query` alone, which would not
reach `BatchPartitionMerger` at all.

Move `repartition_file_scans = false` from the two ordering-aware paths into
`QueryMerger::execute_merge_query`, before the `match` on `merge_scan_ordering`, and call the same
helper from `BatchPartitionMerger::execute_merge_query` right after its session is built:

```rust
// merge.rs

// A merge is a bounded, streaming rewrite of a fixed set of files whose consumer is a single
// writer task (`create_merged_partition` -> `write_partition_from_rows`), not a query that
// benefits from scan parallelism. Left at DataFusion's default (`true`), `EnforceDistribution`
// splits the source scan into `target_partitions` byte-range file groups and `execute_stream`
// coalesces them, so the reader working set is multiplied by the host's core count for no
// throughput the writer can absorb. Forcing one sequential file group here makes every merge
// path -- ordered or not, and for both PartitionMerger implementations -- read one file at a
// time. Downstream parallelism is untouched: a merge query with a GROUP BY still gets its
// round-robin fan-out above the scan.
pub fn configure_merge_session(ctx: &SessionContext) {
    ctx.state_ref()
        .write()
        .config_mut()
        .options_mut()
        .optimizer
        .repartition_file_scans = false;
}
```

called from `QueryMerger::execute_merge_query` before the `match` on `merge_scan_ordering`, and from
`BatchPartitionMerger::execute_merge_query` right after `make_session_context` returns. Delete the
now-redundant assignment from `execute_concatenated_merge` (`merge.rs:106-111`) and from the block
in `execute_per_file_merge` (`merge.rs:167`), leaving that path's other four settings where they
are.

Resulting plan for `SELECT * FROM source;`: a one-file-group `DataSourceExec`, one output
partition, `execute_stream`'s `1 => plan.execute(0, ctx)` arm, one Parquet reader at a time.

**Deliberately not added:** an `assert_single_partition` check on the `Unordered` path. Unlike the
other two paths, `Unordered` carries no ordering claim to protect, and it is the path a
`SqlBatchView` with no declared merge sort order uses with an arbitrary user-supplied
`merge_partitions_query` (`sql_batch_view.rs:194`). A `GROUP BY` in such a query legitimately plans
to multiple partitions, and coalescing it is correct. With `repartition_file_scans = false` those
queries keep their aggregation parallelism via round-robin repartition *above* a sequential scan —
which is the desirable shape, not something to bail on.

### 2. What this changes about the output — and what it recovers for queries

Merged row order becomes deterministic: file concatenation in the order
`create_merged_partition` already sorts them (`filtered_partitions.sort_by_key(|p|
p.begin_insert_time())`, `merge.rs:371`), instead of today's nondeterministic byte-range interleave
from `CoalescePartitionsExec`.

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

Two limits on how much this is worth, both quantified in §3: pruning can never select less than
one row group (128 Ki rows, `write_partition.rs:923`), so the benefit scales with row groups per
partition — large on merged hourly partitions, negligible on fresh one-minute ones — and the global
`measures` view turns out to receive very little query traffic in the deployment that reported this
issue. The mechanism is real and free; the payoff is deployment-dependent.

No guarantee is *recorded*: `get_merged_partition_sort_order` still returns `None` for
`MetricsView`, so no query plan may assume it, and nothing certifies a `sort_order`. This is
ordering by construction — free, contract-free, and applying to every view on the default merge
path.

### 3. What an enforced sort would add, what it would cost, and what the queries actually ask for

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

### 4. Expected improvement, and how to size it

The scan-side component of the spike is linear in `target_partitions`; this change takes it to
`1×`. On a 16-core daemon that is a ~16× reduction of that component. What remains is
core-count-independent:

- writer in-progress row group: ≤ ~100 MB (`write_partition.rs:730`)
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
folded into Phase 1's before/after.

## Implementation Steps

### Phase 1 — Bounded merge scan (the fix)

1. `rust/analytics/src/lakehouse/merge.rs`: extract `pub fn configure_merge_session(ctx:
   &SessionContext)` with the comment from Design §1, and call it from `execute_merge_query` before
   the `match self.merge_scan_ordering`. Also in `rust/analytics/src/lakehouse/batch_partition_merger.rs`:
   call `configure_merge_session(&ctx)` in `BatchPartitionMerger::execute_merge_query` right after
   `make_session_context` returns, so the bounded-memory fallback merger gets the same sequential
   scan.
2. Same file (`merge.rs`): remove the assignment from `execute_concatenated_merge` and from
   `execute_per_file_merge`'s optimizer block; update both methods' rustdoc, which currently
   describes setting it as part of their own path (`merge.rs:97-100`, `merge.rs:150-156`), to
   reference the shared setting instead.
3. Same file: update `MergeQueryResult::ordering_honored`'s rustdoc if it references the per-path
   setting.
4. `rust/analytics/src/lakehouse/view.rs`: extend the `get_scan_output_ordering` rustdoc with a
   short note that a bounded merge scan is *not* a reason to declare an ordering — every merge is
   sequential regardless — so the next view author facing a big merge does not repeat #1491's
   reasoning.
5. `rust/analytics/tests/merge_scan_partitioning_tests.rs` (new): offline plan-shape test, modelled
   on `per_file_scan_ordering_tests.rs` (no DB, `object_store::memory::InMemory`, fabricated
   `Partition`s with `file_size` above the 10 MB `repartition_file_min_size`). Register a
   `PartitionedTableProvider` with `ScanOrdering::Unordered`, `create_physical_plan()` for
   `SELECT * FROM source`, and assert:
   - with the merge setting applied: `partition_count() == 1`
   - without it (control, guarding that the test is meaningful): `partition_count() > 1`
   Build both the control and the configured session from
   `SessionConfig::new().with_target_partitions(8)`, matching the precedent in
   `sql_batch_view_merge_ordering_tests.rs` and `log_stats_ordering_tests.rs`, so the control
   assertion doesn't silently no-op on a low-core-count CI runner (DataFusion otherwise defaults
   `target_partitions` to the host's core count). Calls the `configure_merge_session` helper from
   Design §1 for the configured session.

### Phase 2 — Measure and answer the issue

6. `rust/analytics/src/lakehouse/merge.rs`: extend `create_merged_partition` to log a completion
   line next to the existing `sum_size` line — elapsed wall-clock and output `file_size` — so
   before/after is queryable from `log_entries` without new instrumentation. (Deliberately a log
   line, not a metric: it pairs with the `sum_size` line the issue already quotes.)
7. Collect a five-hour sample matching the issue's, using `process_resident_bytes` /
   `jemalloc_allocated_bytes` / `jemalloc_resident_bytes` rather than host `used_memory`, plus the
   new duration line.
8. Measure row-group pruning before/after: run the same narrow-window `measures` query against a
   pre-change and a post-change merged hourly partition and compare `bytes_scanned` from
   `log_entries WHERE target = 'flightsql_query_audit'` (see Testing Strategy) — the query-side
   half of the result, and the baseline for step 10.
9. Post the answer on #1491: `Concatenated` is unsafe for `measures` (with the two failing
   preconditions and the hard `sort_and_check_non_overlapping` error — landing on the query path,
   via `get_scan_output_ordering`, not the merge path), `PerFile` would be a no-op, the win was
   never in the ordering, and here is the measured before/after — memory and pruning.
10. File the follow-up issue Design §3's evidence actually points at, which is **not** about
    ordering: `measures` queries through per-process JIT view instances spend their time in JIT
    freshness checking over requested ranges far wider than the process's data, not in scanning.
    That is the larger win in the sampled workload and nothing in this plan addresses it.

### Phase 3 — Optional, only if Phase 2 leaves the L1 component significant

11. `rust/analytics/src/lakehouse/lakehouse_context.rs`: add an uncached `ReaderFactory` sharing
    the existing `MetadataCache`.
12. `rust/analytics/src/lakehouse/merge.rs`: use it for the `source` table.
13. Re-measure independently.

## Files to Modify

- `rust/analytics/src/lakehouse/merge.rs` — the setting, the `configure_merge_session` extraction,
  rustdoc, the completion log line
- `rust/analytics/src/lakehouse/batch_partition_merger.rs` — call `configure_merge_session`
- `rust/analytics/src/lakehouse/view.rs` — `get_scan_output_ordering` rustdoc note
- `rust/analytics/tests/merge_scan_partitioning_tests.rs` — new plan-shape regression test
- `rust/analytics/src/lakehouse/lakehouse_context.rs` — Phase 3 only
- `CHANGELOG.md` — Unreleased / Analytics entry
- `mkdocs/docs/admin/maintenance.md` — see Documentation

Not modified: `metrics_view.rs`. The conclusion of this plan is that `measures` needs no view-level
change.

## Trade-offs

**Sequential merge scan vs. bounded merge memory.** Chosen: bounded memory. A merge's consumer is
one writer task behind an `mpsc::channel(1)`; scan parallelism buys throughput the pipeline cannot
absorb while multiplying the reader working set by the core count. The cost is quantified in
Design §4 and measured in Phase 2.

**Fix the shared merge path vs. patch `measures`.** Chosen: the shared path — both
`PartitionMerger` implementations. Every view whose merge takes `QueryMerger`'s default `Unordered`
route has the same fan-out; `measures` is merely the one large enough to make it visible.
`BatchPartitionMerger`, the bounded-memory fallback, has the same unbounded scan (and a worse one:
`2 × target_partitions` concurrent readers from its `try_buffered(2)`) and gets the same fix via the
shared `configure_merge_session` helper, even though no in-repo view constructs one today — it's
one line to include and leaves no latent gap for the next view that opts into it. A `measures`-only
change would leave `log_entries`, `async_events`, `images`, and every unsorted `SqlBatchView` with
the same behavior — and, per Current State, there is no correct `measures`-only change to make.

**Rejected: declare `ScanOrdering::Concatenated` on `MetricsView`** (the issue's proposal). Both
preconditions fail, and the failure mode is loud and total: `sort_and_check_non_overlapping` errors
out on the query path (`get_scan_output_ordering`, consumed only by `MaterializedView::scan`), so
every `measures` query stops — global and every JIT `view_instance` — an even stronger reason to
reject than a merge failure would be. (The merge path is unaffected either way: `QueryMerger`'s
ordering is the separate `with_merge_scan_ordering` field, which `MetricsView` never sets.) Rejected
on correctness, not on cost/benefit.

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
  regardless of declared ordering; note it changes merged-partition row order (previously
  nondeterministic, now source-partition concatenation order), that no sort guarantee is claimed,
  and that the resulting time-local row groups restore row-group pruning for time-filtered queries
  on merged partitions. No SQL-surface change: no schema, view name, or UDF signature moves, and no
  `SCHEMA_VERSION` bump is needed (the file schema is untouched).
- `mkdocs/docs/admin/maintenance.md` — the `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` row already
  explains that daemon merges run on the shared unscoped pool; add a short note that merge scans
  are single-reader by design, so merge memory does not scale with host core count. Relevant to
  anyone sizing a daemon host.
- Rustdoc as listed in Implementation Steps 2-4 — the `view.rs` note is the one that prevents this
  question from being asked again.

## Testing Strategy

- **New offline plan-shape test** (step 5) — the regression guard. Asserts one output partition
  with the merge configuration and more than one without it, so a future DataFusion upgrade that
  re-introduces the fan-out fails CI rather than production memory. Both sessions pin
  `target_partitions` to 8 via `SessionConfig::new().with_target_partitions(8)` so the control
  assertion is meaningful on any CI runner regardless of core count.
- **Existing merge-path tests must stay green** — `blocks_view_merge_ordering_tests.rs`,
  `sql_batch_view_merge_ordering_tests.rs`, `per_file_scan_ordering_tests.rs`,
  `log_stats_ordering_tests.rs`, `sql_partition_spec_sort_order_tests.rs`. These cover the
  `Concatenated` and `PerFile` paths whose duplicated setting is being hoisted; they are the proof
  the refactor is behavior-preserving for them.
- **Full `cargo test` in `rust/`**, plus `cargo clippy --all-targets` and `cargo fmt --check`.
- **Local end-to-end**: `python3 local_test_env/ai_scripts/start_services.py`, generate enough
  telemetry for at least two one-minute `measures` partitions, let the hourly task merge them, and
  confirm from `/tmp/daemon.log` that the merge completes and the merged partition's `num_rows`
  equals the sum of its inputs' (`micromegas-query "SELECT ... FROM list_partitions()"`).
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
  Ordered by `min_event_time` (tiebreak `file_path`) to match the adjacency
  `sort_and_check_non_overlapping` actually checks (`partitioned_execution_plan.rs:83-90` sorts by
  the leading-column `begin` bound, not by `begin_insert_time`). A non-zero count is the count of
  partition pairs the check would have errored on. Worth attaching to the issue reply.
- **Row-group pruning, before/after** (Design §2) — the query-side half of the change, and the
  baseline any future sort project should be judged against. A partition's internal row order is
  fixed at write (merge) time, so this cannot be measured on one partition before and after the
  change — it's a comparison across two *different* merged hourly `measures` partitions: one merged
  before this change (old interleaved clustering) and one merged after (new insert-time-concatenated
  clustering), matched for comparable volume and window width. Run the same narrow-window query
  against each and compare bytes actually scanned:
  ```
  micromegas-query "SELECT count(*) FROM measures WHERE time BETWEEN ... AND ..." --begin 1h
  ```
  with a 1-minute window inside a 1-hour partition. Expect the pre-change partition to scan bytes ≈
  the whole partition; the post-change one, roughly the fraction of row groups covering the window.
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
3. ~~**Is concatenation-order pruning (§2) enough, or is a recorded sort worth its costs?**~~
   **Answered** by the 6-hour query-audit sample in Design §3: a recorded sort is not currently
   justified, because the global `measures` view — the only population this plan's merge produces —
   sees ~2 queries per 6 hours, while the ~546 that matter go to per-process JIT partitions that
   are too small and too JIT-freshness-bound to prune. Contingent on one deployment and one
   dominant dashboard; re-ask if global-view traffic grows.
4. ~~**`time` or `(name, time)` as the sort key?**~~ **Answered**: `(name, time)`. Every sampled
   `measures` query filters `name = '<literal>'` and nothing else, so a time-leading key would
   prune nothing for the observed workload. Recorded for whenever question 3 is re-opened; it also
   forces `PerFile` rather than `Concatenated`.
5. **Does anything outside this repo depend on merged-partition row order?** Nothing in-repo does
   (no `sort_order` is recorded for these views, and it was nondeterministic before), but a
   downstream consumer relying on the incidental order would see a change. Flagged for the issue
   reply rather than blocking.
