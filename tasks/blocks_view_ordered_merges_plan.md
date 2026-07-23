# Blocks-View Ordered Merges + Bounded-Memory Regeneration Plan

## Overview

[#1336](https://github.com/madesroches/micromegas/issues/1336): make blocks-view partition
merges order-preserving so consumers (starting with JIT partition generation) can eventually
trust a declared `(insert_time, block_id)` scan ordering and drop a redundant `SortExec`. This
plan also closes a related OOM hazard: the same Postgres-backed materialization path that would
regenerate a merged partition currently loads the *entire* insert-time range's block rows into
one `Vec<PgRow>` and one `RecordBatch` before writing anything — for a busy day that is an
unbounded amount of memory. Regeneration must never buffer more than one bounded chunk of blocks
at a time.

Four coupled changes:
1. Make blocks-view merges order-preserving (declared scan ordering + explicit `ORDER BY`), so
   merged partitions stay internally sorted by `(insert_time, block_id)` going forward.
2. Make the Postgres-source partition write path (`MetadataPartitionSpec::write`, used by
   `BlocksView::make_batch_partition_spec`) stream in bounded chunks instead of
   `fetch_all`-ing a whole insert range into memory.
3. Add an admin table function to force-regenerate existing (already-merged, potentially
   unsorted) partitions online from the Postgres source tables, reusing the now-streaming write
   path — no downtime, no schema change.
4. Record each partition's actual sort guarantee as a first-class, SQL-queryable
   `lakehouse_partitions.sort_order` column (schema v6→v7), since Parquet footers — and any sort
   order recoverable from them — are no longer stored in Postgres (`partition_metadata` was
   dropped in v5→v6, `migration.rs:418-426`). This makes "is this partition safely ordered"
   queryable during the §3 regeneration rollout and cheaply available at planning time via the
   partition cache, without a footer fetch.

Declaring `(insert_time, block_id)` as a *trusted* scan ordering for blocks-view consumers (the
JIT partition generators, per `tasks/jit_single_query_plan.md`) is explicitly **out of scope**
here — it only becomes safe after every active merged partition has been regenerated under (1),
which is an operational rollout step, not a code change. See Open Questions.

## Current State

### Merges are not order-preserving today

`BlocksView` (`rust/analytics/src/lakehouse/blocks_view.rs`) writes fresh partitions sorted —
`data_sql` ends with `ORDER BY blocks.insert_time, blocks.block_id` (`blocks_view.rs:46`). But
`BlocksView` does not override `merge_partitions`, so it gets `View::merge_partitions`'s default
(`view.rs:101-124`): a `QueryMerger` running `SELECT * FROM source;` — no `ORDER BY` — over a
`PartitionedTableProvider` source table built with **no declared scan ordering**
(`merge.rs:81-86` always constructs `PartitionedTableProvider::new(...)`, whose `scan()` always
passes `&[]` for `output_ordering`, `partitioned_table_provider.rs:63-72`). DataFusion is free to
interleave file reads from the source partitions in any order, so a merged daily blocks
partition is not guaranteed sorted, even though its inputs are.

This matters because `partitioned_execution_plan.rs`'s declared-ordering machinery — used today
by `MaterializedView` (`materialized_view.rs:94`, via `View::get_scan_output_ordering`) for
per-view consumer queries (e.g. `ThreadSpansView`, `thread_spans_view.rs:357`) — is entirely
event-time-based: `sort_and_check_non_overlapping` and `attach_ordering_statistics`
(`partitioned_execution_plan.rs:29,61`) read `Partition::min_event_time()`/`max_event_time()`.
Blocks-view ordering is insert-time-based (`Partition::begin_insert_time()`/`end_insert_time()`,
`partition.rs:44-51` — always `Some`, unlike the `Option` event-time bounds), and
`PartitionedTableProvider` never plumbs any ordering into its source scan at all. Neither piece
of existing infrastructure covers the merge source table today.

### The Postgres-source write path buffers a full insert range

`BlocksView::make_batch_partition_spec` (`blocks_view.rs:67-93`) delegates to
`fetch_metadata_partition_spec` (`metadata_partition_spec.rs:29-56`), which runs a `COUNT(*)`
query up front (cheap, one row) and returns a `MetadataPartitionSpec` holding that count and the
sorted `data_sql`. The actual data fetch happens later, in
`MetadataPartitionSpec::write` (`metadata_partition_spec.rs:68-118`):

```rust
let rows = sqlx::query(&self.data_sql)
    .bind(self.insert_range.begin)
    .bind(self.insert_range.end)
    .fetch_all(&lake.db_pool)          // <- entire range's rows, in one Vec<PgRow>
    .await?;
...
let record_batch = rows_to_record_batch(&rows)?;   // <- entire range, in one RecordBatch
```

`fetch_all` materializes every matching `blocks ⋈ streams ⋈ processes` row for the whole
`insert_range` — which defaults to a full day (`View::get_max_partition_time_delta`'s default is
`TimeDelta::days(1)`, `view.rs:130-132`) — as one `Vec<PgRow>`, then `rows_to_record_batch`
converts all of it into one `RecordBatch` before a single row reaches
`write_partition_from_rows`. Each row carries the joined `streams`/`processes` columns
(`tags`, `properties`, `dependencies_metadata`, `objects_metadata` — all JSONB/array columns), so
a busy day's block count times per-row payload size can be large enough to OOM the process doing
the materialization. This is the only `PartitionSpec::write` implementation in the codebase that
still works this way — every sibling implementation already streams:

- `BlockPartitionSpec::write` (`block_partition_spec.rs:60-162`) consumes
  `PartitionBlocksSource::get_blocks_stream()` and sends one `PartitionRowSet` per processed
  block.
- `SqlPartitionSpec::write` (`sql_partition_spec.rs:77-114`) runs its extract query via
  `df.execute_stream()` and sends one `PartitionRowSet` per `RecordBatch`.
- `create_merged_partition` (`merge.rs:132-232`) consumes the merge's `SendableRecordBatchStream`
  the same way.

All three write to `write_partition_from_rows` (`write_partition.rs:560-670`) through a
`tokio::sync::mpsc::channel(1)` — a one-item buffer that backpressures the producer until the
Parquet writer (which itself flushes every 100MB, `write_partition.rs:437-442`) drains the
previous batch. `MetadataPartitionSpec` is the only one that defeats this backpressure by
building its one giant `RecordBatch` before the channel exists in any meaningful sense.

`MetadataPartitionSpec` is only used by `BlocksView` (verified by grep) — this is a self-contained
fix, not a widely shared abstraction.

### Regeneration has no forcing path today

The existing `materialize_partitions` admin UDF (`materialize_partitions_table_function.rs`,
registered in `query.rs:131-137`) calls `batch_update::materialize_partition_range`
(`batch_update.rs:195-215`), which for each `partition_time_delta`-sized bucket calls
`materialize_partition` (`batch_update.rs:102-191`). That function always runs
`verify_overlapping_partitions` (`batch_update.rs:23-100`) first, which compares the *source data
hash* (an object count) against existing partitions' stored hashes and returns
`PartitionCreationStrategy::Abort` when they already match (`batch_update.rs:94-99`, "already up
to date"). An existing merged blocks partition whose row order is wrong but whose content
(hence hash) is unchanged is exactly the "already up to date" case — `materialize_partitions`
cannot force it to rebuild.

`insert_partition` (`write_partition.rs:265-400`) already performs the atomic swap this plan
needs for regeneration: inside one Postgres transaction, guarded by a per-partition advisory
lock, it calls `retire_partitions` (delete old row + queue its file for cleanup) then `INSERT`s
the new partition row (`write_partition.rs:318-359`), commit-releasing the lock
(`write_partition.rs:389`). `create_merged_partition` and every `PartitionSpec::write` already
go through this same path — regeneration reuses it for free once it reaches
`write_partition_from_rows`.

## Design

### 1. Order-preserving merges

Generalize the event-time-only ordering machinery in `partitioned_execution_plan.rs` to also
support insert-time bounds, then wire that into the merge source table and give `BlocksView` an
`ORDER BY`-based merge.

```rust
/// Which pair of bounds on `Partition` a declared ordering's leading column is checked against.
#[derive(Clone, Copy, Debug)]
pub enum OrderingBounds {
    /// `min_event_time()` / `max_event_time()` — `Option`, absent for empty partitions.
    EventTime,
    /// `begin_insert_time()` / `end_insert_time()` — always present.
    InsertTime,
}
```

- `sort_and_check_non_overlapping` and `attach_ordering_statistics` take an `OrderingBounds` and
  read bounds through one small helper (`fn partition_bounds(p: &Partition, bounds:
  OrderingBounds) -> Option<(DateTime<Utc>, DateTime<Utc>)>`) instead of calling
  `min_event_time()`/`max_event_time()` directly. Behavior for `OrderingBounds::EventTime` is
  byte-for-byte unchanged.
- `make_partitioned_execution_plan` gains an `ordering_bounds: OrderingBounds` parameter. It has
  two production callers: through `MaterializedView` (`materialized_view.rs:86-96`), and through
  `PartitionedTableProvider::scan` (`partitioned_table_provider.rs:63`, the live user-query path
  registered by `query.rs:64` and also used by `batch_partition_merger.rs:136`). Both pass
  `OrderingBounds::EventTime` — the `MaterializedView` site explicitly, the
  `PartitionedTableProvider` site via its `new(...)` constructor's `EventTime` default (see below)
  — so there is no behavior change for any existing consumer-side view. The
  function is also called directly (bypassing `MaterializedView`) from 3 sites in
  `rust/analytics/tests/thread_spans_ordering_tests.rs` (lines 77, 105, 147); those must be
  updated to pass `OrderingBounds::EventTime` too, or the crate won't compile against the new
  signature — see Files to Modify.
- `PartitionedTableProvider` gains an `output_ordering: Vec<ScanSortColumn>` +
  `ordering_bounds: OrderingBounds` pair of fields, defaulted to `vec![]` /
  `OrderingBounds::EventTime` by the existing `PartitionedTableProvider::new(...)` constructor
  (used unchanged by `query.rs` and `batch_partition_merger.rs`), plus a new
  `PartitionedTableProvider::with_ordering(schema, reader_factory, partitions, output_ordering,
  ordering_bounds)` constructor that `scan()` threads through to
  `make_partitioned_execution_plan`.
- `QueryMerger` (`merge.rs`) gains a builder method (e.g. `with_merge_scan_ordering(self,
  ordering: Vec<ScanSortColumn>) -> Self`, default empty) and uses
  `PartitionedTableProvider::with_ordering(..., ordering, OrderingBounds::InsertTime)` to build
  its `"source"` table instead of the unconditional `PartitionedTableProvider::new(...)`. With an
  empty ordering (every existing `View::merge_partitions` caller, and `sql_batch_view.rs`'s
  merger), this is a no-op — identical plan to today.
- `BlocksView` overrides `merge_partitions` (mirroring the pattern already used by
  `SqlBatchView::merge_partitions`, `sql_batch_view.rs:250-269`, which just delegates to a
  pre-built merger) to build and reuse a `QueryMerger` configured with:
  - query: `"SELECT * FROM source ORDER BY insert_time;"`
  - ordering: `[ScanSortColumn { column: Arc::new(String::from("insert_time")), descending: false }]`
    (`column` is `Arc<String>`, not `&str` — see `view.rs`'s `ScanSortColumn` struct and the existing
    usage pattern in `thread_spans_view.rs`'s `get_scan_output_ordering()`)

  Only `insert_time` is declared (not `block_id`), and the query's `ORDER BY` matches it exactly:
  `attach_ordering_statistics` can only attach min/max stats for the leading declared column, and
  `Partition` metadata has no `block_id` bounds to attach — a two-column declared ordering would
  never validate (DataFusion 54 requires present min/max stats for *every* declared sort column)
  and DataFusion would fall back to a full buffering `SortExec`. A single-column `insert_time`
  ordering is still sufficient for full `(insert_time, block_id)` correctness: source partitions
  are non-overlapping in insert_time by construction and each is already sorted by
  `(insert_time, block_id)` (`blocks_view.rs:46`), so once the source scan's single sequential
  file group (see below) has a validated `insert_time` ordering, its file-by-file concatenation is
  already exactly the order the merge needs — no second declared column is required to reach that
  result.

  `create_merged_partition` already calls `filtered_partitions.sort_by_key(|p|
  p.begin_insert_time())` before invoking `view.merge_partitions` (`merge.rs:171`), and
  time-sliced partitions are non-overlapping in insert_time by construction, so
  `sort_and_check_non_overlapping` under `OrderingBounds::InsertTime` should always pass for
  well-formed input — a failure here would indicate a genuine partitioning bug, matching the
  existing "fail loudly" philosophy for the event-time case.

  Why the merged output ends up fully `(insert_time, block_id)`-sorted, not just
  `insert_time`-sorted — **conditionally**, gated on the inputs actually being trustworthy:
  1. Each input file is internally `(insert_time, block_id)`-sorted *if and only if* its own
     recorded `sort_order` (Design §4) already equals `['insert_time', 'block_id']`. This is true
     for partitions written fresh via `data_sql`'s `ORDER BY blocks.insert_time, blocks.block_id`
     (`blocks_view.rs:46`) and for partitions produced by a prior run of this same ordered merge —
     but it is **not** true for a partition merged before this change shipped: the maintenance
     daemon (`rust/public/src/servers/maintenance.rs:68-174`) creates 1-second `CreateFromSource`
     blocks partitions, then rolls them up through minutely, hourly, and daily merges, and every
     pre-fix merge (at any granularity) ran `View::merge_partitions`'s unordered `SELECT * FROM source;` default
     (`view.rs:101-124`) with no declared scan ordering — its output is not internally sorted, and
     (before Design §4 exists) it has no `sort_order` to say so. `BlocksView::merge_partitions`
     therefore only takes the ordered path below — declaring the `[insert_time]` scan ordering and
     later recording `['insert_time', 'block_id']` — when *every* partition in `partitions_to_merge`
     already has `sort_order == Some(['insert_time', 'block_id'])`; if even one input's `sort_order`
     is `NULL` (every pre-fix partition, merged or not), the merge instead runs today's plain
     unordered query and records `NULL` itself, rather than trusting a per-file order it cannot
     verify (see Design §4 and Open Questions for the resulting rollout property). Points 2 and 3
     below, and the elision they enable, are therefore only ever relied on once this gate has
     already confirmed point 1 for the specific inputs at hand.
  2. Input partitions have disjoint, half-open `[begin, end)` insert-time ranges by construction,
     so no two files can contain equal-`insert_time` rows — ties never need to be broken *across*
     files, only within one (already-sorted) file.
  3. `make_partitioned_execution_plan` puts every source file into one sequential file group, and
     DataFusion validates the declared `[insert_time]` ordering against each file's min/max
     insert-time stats; validation only passes if the files are non-overlapping and already in
     ascending order. `QueryMerger::execute_merge_query` (`merge.rs`) sets
     `datafusion.optimizer.repartition_file_scans = false` on its session, but **only when the
     merger's declared scan ordering is non-empty** — i.e. only for `BlocksView`'s ordered merger,
     never for the plain unordered merger (empty ordering, every other `View::merge_partitions`
     caller and `sql_batch_view.rs`'s aggregation merger; see Design §4/Trade-offs for why scoping
     this matters). For an ordering-declared merge, preserving global order forces the source scan
     into one sequential file group regardless — a merge under this path always executes as a
     single output stream, so disabling scan-file repartitioning costs nothing there. This matters
     for one specific shape: a merge source can end up with exactly one non-empty input file — empty partitions still
     count toward `create_merged_partition`'s "≥2 partitions" merge trigger but are then dropped by
     `make_partitioned_execution_plan` (e.g. one busy hour plus otherwise-empty hourlies) — and
     without the setting, DataFusion 54's `FileGroupPartitioner` falls through to
     `repartition_evenly_by_size` for a single-file group, byte-range-splitting it across
     `target_partitions` streams and requiring a `SortPreservingMergeExec` to reassemble the
     declared order, rather than eliding the sort as a no-op. With repartitioning off, one file
     group is always exactly one stream — for any number of non-empty input files, including
     exactly one — so once validated, `EnforceSorting` proves the `ORDER BY insert_time` is already
     satisfied and elides the sort node entirely: the plan becomes the plain in-order concatenation
     of the files, with no `SortExec` and no `SortPreservingMergeExec`.

  Point 3's elision is what turns points 1 and 2 into a guarantee on the actual output: an elided
  sort is a no-op concatenation, so the already-`(insert_time, block_id)`-sorted files stay in that
  order end to end. **This is a correctness dependency, not just a memory optimization, and it
  rests on two independent guards, not one:**
  - Point 1 is only true because the merge only takes this ordered path when every input's own
    recorded `sort_order` already equals `['insert_time', 'block_id']` (see point 1 above and
    Design §4). If that gate is wrong or skipped, elision would faithfully concatenate
    internally-unsorted files in validated insert-time order and produce a merged file that is
    *not* `block_id`-sorted within a shared `insert_time`, no matter how well the plan-shape check
    below is doing its job — the check can only confirm that the sort was elided, not that eliding
    it was safe for these particular inputs.
  - Given that guard holds, elision must also actually happen at the plan level. If elision failed
    instead (e.g. missing/absent min-max stats on some file, or the `repartition_file_scans`
    setting from point 3 not taking effect), the fallback is a real `SortExec` ordering by
    `insert_time` alone — and a plain (non-stable-guaranteed) sort on one column does not preserve
    the `block_id` tie-break order from point 1, so the merged output could come out **not** sorted
    by `block_id` within an `insert_time` value, silently breaking the ordering guarantee this
    design exists to provide — while also reintroducing the full buffering `SortExec` this plan is
    meant to avoid. The offline test below asserting sort elision is therefore load-bearing for
    both the ordering guarantee's correctness and its memory bound, not a performance-only check.
    Because DataFusion's `validate_orderings` silently drops an unprovable declared ordering rather
    than erroring — so this elision failure mode produces no error on its own — the merge path also
    performs a runtime physical-plan-shape check before execution, but only on the ordered path
    (Design §4; scoping rationale in Trade-offs): it builds the optimized physical plan and fails
    the merge loudly unless it is a single-partition plan containing **neither** a `SortExec`
    **nor** a `SortPreservingMergeExec` node — matching the offline test's own assertion, since
    either operator means elision did not fully happen (a bare `SortExec` means the declared
    ordering never validated; a `SortPreservingMergeExec` with no `SortExec` means
    `repartition_file_scans` didn't stop a single-file group from being byte-range-split, per point
    3 — the exact failure mode a check that only looked for `SortExec` would miss, since the string
    `"SortPreservingMergeExec"` does not contain the substring `"SortExec"`).

  Together, the input-`sort_order` gate and the plan-shape check are what keep a false `sort_order`
  guarantee from ever being persisted: the gate stops the merge from trusting inputs it can't
  verify, and the check stops the merge from trusting a plan shape it can't verify. Neither one
  alone is sufficient — see Design §4's value semantics for how the recorded `sort_order` reflects
  both.

### 2. Bounded-memory Postgres-source writes

Rewrite `MetadataPartitionSpec::write` to stream rows from Postgres in bounded chunks instead of
`fetch_all`-ing the whole range, following the same "spawn the writer, stream `PartitionRowSet`s
into its channel" shape already used by `create_merged_partition`, `SqlPartitionSpec::write`, and
`BlockPartitionSpec::write`:

```rust
/// Flush threshold on the estimated byte size of the pending chunk — bounds peak memory to one
/// ~8 MB chunk, not one day. Byte-based like the Parquet writer's own 100 MB flush
/// (`write_partition.rs:437-442`), because a row-count threshold bounds nothing when a few rows
/// carry MB-sized properties/objects_metadata payloads. Deliberately the only flush metric.
const SOURCE_BYTES_PER_BATCH: usize = 8 * 1024 * 1024;

async fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>) -> Result<()> {
    ...
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let join_handle = spawn_with_context(write_partition_from_rows(
        lake.clone(), self.view_metadata.clone(), self.schema.clone(),
        self.insert_range, self.get_source_data_hash(), rx, logger.clone(),
    ));

    let stream_result: Result<()> = async {
        if self.record_count > 0 {
            let mut rows = sqlx::query(&self.data_sql)
                .bind(self.insert_range.begin)
                .bind(self.insert_range.end)
                .fetch(&lake.db_pool);           // streaming cursor, not fetch_all
            let ctx = SessionContext::new();
            let mut chunk = Vec::new();
            let mut chunk_bytes = 0usize;
            while let Some(row) = rows.try_next().await? {
                chunk_bytes += estimate_row_bytes(&row);
                chunk.push(row);
                if chunk_bytes >= SOURCE_BYTES_PER_BATCH {
                    flush_chunk(&mut chunk, &ctx, &self.compute_time_bounds, &tx).await?;
                    chunk_bytes = 0;
                }
            }
            if !chunk.is_empty() {
                flush_chunk(&mut chunk, &ctx, &self.compute_time_bounds, &tx).await?;
            }
        }
        Ok(())
    }.await;

    match stream_result {
        Ok(()) => {
            drop(tx);
            join_handle.await??;
            Ok(())
        }
        Err(e) => {
            // mirror create_merged_partition's error path: send the abort through the
            // channel before dropping it, so write_partition_from_rows sees an Err item
            // instead of a plain closed-channel end-of-stream and does not commit a
            // partial partition.
            let _ = tx.send(Err(anyhow::anyhow!("metadata partition stream aborted"))).await;
            drop(tx);
            let _ = join_handle.await;
            Err(e)
        }
    }
}
```

`flush_chunk` converts the accumulated `Vec<PgRow>` via the existing `rows_to_record_batch`
(unchanged — it already operates on a row slice, so it works identically on a partial chunk),
computes the chunk's event-time bounds via `compute_time_bounds.get_time_bounds(...)` (same call
already made once for the whole range today), sends a `PartitionRowSet`, and clears the `Vec` via
`mem::take`/`clear()` for reuse.

`estimate_row_bytes` sums the row's raw column value lengths — `row.try_get_raw(i)` →
`PgValueRef::as_bytes()` (an inherent accessor in sqlx 0.8, `sqlx-postgres/src/value.rs:64`; note
it returns `Result<&[u8], BoxDynError>`, not a bare slice), counting `NULL` and any
`Err`/non-byte-backed values as 0. This estimates payload bytes, not allocator-exact
footprint — it deliberately tracks the JSONB/binary columns (`properties`, `objects_metadata`,
`dependencies_metadata`) that dominate blocks-view row width, which is all the flush decision
needs.

`source_data_hash` switches from `rows.len()` (recomputed from the fully-fetched `Vec<PgRow>`) to
`self.get_source_data_hash()` — the `record_count` already fetched by the earlier `COUNT(*)`
query in `fetch_metadata_partition_spec`. This is required because
`write_partition_from_rows` needs the hash *before* streaming starts (it's a spawn-time
parameter, not something derivable after the fact), and it's a simplification: `write()` no
longer computes a second, independently-arrived-at row count that could disagree with
`get_source_data_hash()`'s.

Peak memory becomes one ~`SOURCE_BYTES_PER_BATCH` chunk of `PgRow`s plus its `RecordBatch`
conversion plus the one in-flight channel item — bounded regardless of how many blocks exist in
the requested insert range and, unlike a row-count threshold, regardless of per-row payload
width. The Parquet writer's own up-to-100 MB in-progress buffer dominates the total either way,
which is why 8 MB needs no upward tuning: past the per-flush-overhead knee (one Arrow conversion,
one time-bounds pass, one channel send per flush), bigger chunks buy nothing — the writer
accumulates chunks into identical row groups regardless of chunk size, so the output file is
byte-identical too.

### 3. Forced online regeneration

Add a `force: bool` parameter to `batch_update::materialize_partition` (`batch_update.rs:103`,
private, with only one caller: the loop inside `materialize_partition_range`).
`materialize_partition_range` itself (`batch_update.rs:195`) is `pub` with 9 existing callers —
the production maintenance daemon (`rust/public/src/servers/maintenance.rs:55`), the
`materialize_partitions` UDF (`materialize_partitions_table_function.rs:56`), and 7 test call
sites (`histo_view_test.rs` ×3, `sql_view_test.rs` ×3, `thread_spans_ordering_db_test.rs` ×1) —
so its signature must not change. Instead, factor its loop body into a new private
`materialize_partition_range_impl(..., force: bool)`: `materialize_partition_range` becomes a
thin wrapper calling it with `force: false` (unchanged behavior, unchanged signature, zero call
sites touched), and a new `regenerate_partition_range` (same signature otherwise) calls it with
`force: true`. When `force` is true, `materialize_partition` skips *only* the source-data-hash
freshness comparison inside `verify_overlapping_partitions` — the "already up to date" check this
plan needs to bypass — and takes `PartitionCreationStrategy::CreateFromSource` unconditionally,
*and* also skips the `get_max_partition_time_delta`-driven subdivision check
(`batch_update.rs:138-155`) so the entire requested `insert_range` is written as a single
partition instead of being split into buckets.

**Invariant `regenerate_partitions` callers must uphold:** the requested `(begin, end, delta)`
must exactly cover the boundaries of the partition(s) being regenerated.
`retire_partitions`'s cleanup only deletes partitions fully contained within the *new*
partition's own range (`begin_insert_time >= $3 AND end_insert_time <= $4`,
`write_partition.rs:226-246`); a misaligned range/delta (e.g. regenerating a daily partition with
`delta=3600`, or a sub-range that starts or ends mid-partition) means the new partition's range
doesn't fully contain the old one, so the old partition is never retired — it survives untouched
while the new one is inserted alongside it, producing silent duplicate rows.

This invariant is enforced, not just documented: `regenerate_partition_range` validates, before
entering `materialize_partition_range_impl`'s loop, that `delta` exactly tiles `(begin, end)` —
`(end - begin)` must be an exact, non-zero multiple of `delta`. `chrono::TimeDelta` implements no
`Rem`/`%` operator, so the check is integer arithmetic on nanoseconds: with
`span = (end - begin).num_nanoseconds()` and `step = delta.num_nanoseconds()` (both
`Option<i64>`-unwrapped with `expect`, well within range for any real time range), require
`step > 0 && span >= step && span % step == 0` — and returns a loud `Err` otherwise. This is a distinct failure
mode from the one `verify_force_regeneration_alignment` (below) catches: the loop
(`batch_update.rs:203-220`, reused by `_impl`) runs `while end_part <= insert_range.end`, so a
`delta` that doesn't tile the range makes it execute a partial or zero number of iterations and
return `Ok` without ever reaching `materialize_partition` (and hence
`verify_force_regeneration_alignment`) for the untiled remainder — a silent partial or total no-op
that the alignment guard, which only runs per-bucket inside the loop, would never see.

Because `force` bypasses `verify_overlapping_partitions` entirely, it also loses that function's
*only* other job: rejecting a partial overlap (`batch_update.rs:57-67`,
`begin < insert_range.begin || end > insert_range.end` → `Abort`). To keep that protection
without reintroducing the freshness check, add a small new function,
`verify_force_regeneration_alignment`, called only on the `force` path, that runs the same
partial-overlap test against the same filtered existing-partitions set but with no source-hash
comparison — so it can never produce the "already up to date" `Abort` that forcing is meant to
bypass, only a loud `Err` on a misaligned request:

```rust
fn verify_force_regeneration_alignment(
    existing_partitions_all_views: &PartitionCache,
    insert_range: TimeRange,
    view_set_name: &str,
    view_instance_id: &str,
    file_schema_hash: &[u8],
) -> Result<()> {
    let filtered = existing_partitions_all_views.filter(
        view_set_name,
        view_instance_id,
        file_schema_hash,
        insert_range,
    );
    for part in &filtered.partitions {
        let begin = part.begin_insert_time();
        let end = part.end_insert_time();
        if begin < insert_range.begin || end > insert_range.end {
            anyhow::bail!(
                "regenerate_partitions: requested range [{}, {}] does not fully contain \
                 existing partition [{}, {}] for {view_set_name}/{view_instance_id} — \
                 range/delta must exactly cover the partition(s) being regenerated",
                insert_range.begin.to_rfc3339(), insert_range.end.to_rfc3339(),
                begin.to_rfc3339(), end.to_rfc3339(),
            );
        }
    }
    Ok(())
}

let strategy = if force {
    verify_force_regeneration_alignment(
        &existing_partitions_all_views,
        insert_range,
        &view_set_name,
        &view_instance_id,
        &view.get_file_schema_hash(),
    )?;
    PartitionCreationStrategy::CreateFromSource
} else {
    verify_overlapping_partitions(...).await?
};
let new_delta = if force {
    insert_range.end - insert_range.begin
} else {
    view.get_max_partition_time_delta(&strategy)
};
if new_delta < (insert_range.end - insert_range.begin) {
    ... // subdivision branch, unreachable when force is true
}
```

This guard is meant to fail the call loudly instead of silently leaving both the stale and the new
partition in place — but that requires `regenerate_partitions` to surface the `Err` as a
query-level failure, not as a log row `TaskLogExecPlan` streams back as if the query succeeded (see
below for why the existing `materialize_partitions`/`TaskLogExecPlan` pattern cannot do this, and
the mechanism `regenerate_partitions` uses instead).

Skipping the subdivision is required, not optional: `BlocksView::get_max_partition_time_delta`
returns `TimeDelta::hours(1)` for `CreateFromSource` but `TimeDelta::days(1)` for
`MergeExisting` (`blocks_view.rs:140-147`). A forced regeneration over a day range that honored
the 1-hour delta would recurse into `materialize_partition_range` and write 24 hourly
partitions instead of one daily partition. `retire_partitions`'s range-containment delete
(`begin_insert_time >= $3 AND end_insert_time <= $4`, `write_partition.rs:226-246`) only retires
partitions fully contained within *each new* partition's own range, so none of those 24 hourly
`[h, h+1h)` ranges would contain the pre-existing daily `[day_begin, day_end)` partition — it
would never be retired, leaving duplicate rows until a manual `retire_partitions` call. Writing
the whole range as one `CreateFromSource` partition makes the new partition's range exactly
`[day_begin, day_end)`, so `retire_partitions` cleanly deletes the old daily partition in the
same transaction as the insert.

`force` is threaded unchanged through the `materialize_partition_range_impl` →
`materialize_partition` loop call (`batch_update.rs:209`); only the subdivision decision inside
`materialize_partition` itself is affected. The subdivision branch's recursive call
(`batch_update.rs:156`) is only reachable when `force` is false — a forced call's `new_delta`
always spans the whole range, so the `new_delta < range` check never recurses — and keeps calling
the plain, forceless `materialize_partition_range`; no recursion needs to carry `force` through.

Via `partition_spec.write()`, this still uses the now-streaming Postgres fetch from part 2 (so a
whole-day `CreateFromSource` write stays memory-bounded even without subdivision) and the
retire-then-insert atomic swap in `insert_partition` (part of the "Current State" analysis
above), so regeneration is online (no downtime) and memory-bounded from day one.

**Guarding against a concurrent daemon write into the same range.** `verify_force_regeneration_alignment`
only checks the `PartitionCache` snapshot fetched at the start of the call — it cannot see a
partition the maintenance daemon commits *after* that snapshot but *before* the forced write
commits. This matters specifically when `regenerate_partitions` targets a range the daemon is
still actively materializing at finer granularity (e.g. the current or a very recent day, whose
hours/minutes the daemon has not yet rolled up into a merged daily; already-merged past dailies are
not exposed to this race). `insert_partition`'s advisory lock (`write_partition.rs:251-263`) is
keyed on the exact `(view_set_name, view_instance_id, begin_insert_time, end_insert_time)` tuple,
so a regen write and a concurrent daemon write into an overlapping range take *different* locks and
can commit in either order, with the overlap running in either direction of containment:
- A regen daily `[d, d+1)` write, and a concurrent daemon hourly `[h, h+1)` write (contained
  *inside* `[d, d+1)`) that commits after the regen daily. Nothing retires it: `retire_partitions`
  only deletes rows fully contained within the *new* partition's own range, and every later daemon
  pass hits `verify_overlapping_partitions`'s partial-overlap `Abort` (`batch_update.rs:57-67`) on
  both rows.
- The reverse: following the rollout's "regenerate finer partitions first" ordering (Open Questions), a
  regen hourly `[h, h+1)` write is in flight when the daemon's daily merge commits `[d, d+1)`
  first — `d` spans `h`, and the daemon's merge retires the old hourlies. The regen's own
  `retire_partitions([h, h+1))` cannot delete that daily row (the daily is not contained *inside*
  `[h, h+1)` — it *contains* `[h, h+1)`, the reverse relationship), so the regen's new hourly and
  the daemon's new daily both persist.

Either shape leaves both rows in place forever, and queries double-count the overlapping hour's
blocks.

To close this without depending on daemon-scheduling internals (there is no clean, stable
definition of "the daemon's active horizon" to check against from `batch_update.rs`), the forced
path re-checks for a new overlap from *inside* `insert_partition`'s own transaction, immediately
before it would otherwise commit — where it can see anything any other transaction has already
committed, not a stale snapshot. `PartitionSpec::write` (`view.rs:32`) gains a `force: bool`
parameter (`false` at every existing call site — `BlockPartitionSpec::write`,
`SqlPartitionSpec::write`, and `MetadataPartitionSpec::write`'s normal, non-forced callers — so
this is a no-op everywhere except forced regeneration), threaded through `write_partition_from_rows`
into `insert_partition`. There are two production call sites of `PartitionSpec::write` itself that
need updating for the new parameter: `materialize_partition`'s `partition_spec.write(...)`
(`batch_update.rs:171`), which supplies the real `force` value driving this guard, and
`write_partition_from_blocks`'s `block_spec.write(...)` (`jit_partitions.rs:632`, JIT partition
generation at query time), which passes `force: false` and never triggers this guard. When `force` is `true`, `insert_partition`, inside its existing transaction,
*after* `retire_partitions` has removed the partition being replaced and after the new partition row
has been inserted — immediately before it would otherwise commit — runs one more `SELECT` against
`lakehouse_partitions` for the same `(view_set_name, view_instance_id)`, applying a general
*interval-overlap* predicate — `begin_insert_time < insert_range.end AND end_insert_time >
insert_range.begin` (the same predicate `PartitionCache::filter` already uses at snapshot time,
`partition_cache.rs:209-231`) — excluding the row just inserted (its own exact
`begin_insert_time`/`end_insert_time`) so the new partition doesn't match its own recheck. This
must be the general overlap test, not a containment-only predicate
(`begin_insert_time >= insert_range.begin AND end_insert_time <= insert_range.end`): containment
only asks whether the *existing* row sits *inside* `insert_range`, which catches the first shape
above (daemon hourly inside regen daily) but misses the second (daemon daily containing regen
hourly) — there the existing daily row's bounds are not contained within the regen hourly's
`insert_range` at all; the containment relationship runs the other way. The
`begin < insert_range.end AND end > insert_range.begin` form is direction-agnostic and catches
both shapes with one check. `bail!`s — rolling back both the retire and the not-yet-committed
insert, so neither the old nor a duplicate new partition is left behind — if any row comes back.
This rollback undoes the retire and the insert but not the Parquet file `finalize_partition_write`
already wrote and closed before this recheck runs — a guard-triggered `bail!` leaves that file
orphaned in object storage, neither deleted nor registered via `add_file_for_cleanup`. This matches
`insert_partition`'s existing INSERT-error path, which leaks the same way, so it is an acknowledged
pre-existing pattern rather than a new gap; a follow-up could delete the file or register it for
cleanup before returning the error.
Because Postgres's default `READ COMMITTED` isolation lets this in-transaction `SELECT` see any row
another transaction has already committed, this shrinks the race window from "the whole
forced-regeneration call" down to "the gap between this `SELECT` and this transaction's `COMMIT`" —
the same order of magnitude every other `insert_partition` caller already lives with, rather than a
window wide enough for a daemon pass to land in. `regenerate_partition_range` sets `force: true` on
the `PartitionSpec::write()` call it reaches; `materialize_partition_range`'s existing non-forced
path always passes `false`.

This in-transaction recheck is keyed on the advisory lock's exact `(view_set_name, view_instance_id,
begin_insert_time, end_insert_time)` tuple, so it only serializes writers targeting the *identical*
range; it does not fully close the race between two concurrent *forced* regenerations of
overlapping-but-different ranges (e.g. an hourly and a daily forced call whose ranges overlap) —
those take different locks, so under `READ COMMITTED` each transaction's recheck can pass before the
other commits, and both can commit, leaving duplicate/overlapping partitions. `regenerate_partitions`
is a temporary/admin rollout tool, not a steady-state path, so the mitigation here is operational,
not a new locking mechanism: operators must run `regenerate_partitions` calls serially, never with
overlapping ranges in flight concurrently.

`regenerate_partitions` reads from the Postgres ingestion tables, which retain rows for a shorter
window than merged lakehouse partitions. If a forced regeneration runs on a partition whose source
rows have already aged out, it will deliberately produce a smaller partition than the one it
replaces — accepted, since that older data is already past ingestion retention and headed for
deletion by lakehouse retention regardless.

`materialize_partitions_table_function.rs`'s existing call (`:56`) needs no change at all —
`materialize_partition_range`'s signature is untouched, so the existing `materialize_partitions`
UDF's behavior is unchanged automatically.

Add a new `regenerate_partitions` table function (new file
`regenerate_partitions_table_function.rs`, registered in `query.rs` next to
`materialize_partitions`), reusing `MaterializePartitionsTableFunction`'s argument parsing
(`view_set_name`, `begin`, `end`, `delta_seconds`) but **not** its log-only error handling,
because that shape cannot fail a query: today `TaskLogExecPlan`/`AsyncLogStream`
(`rust/analytics/src/dfext/task_log_exec_plan.rs`,
`rust/analytics/src/dfext/async_log_stream.rs`) stream a plain `mpsc::Receiver<(DateTime<Utc>,
String)>` into `(time, msg)` rows with no error channel at all, and
`materialize_partitions_table_function.rs`'s spawner (`:91-110`) only does `if let Err(e) = ... {
logger.write_log_entry(msg); error!(...) }` and returns — an `Err` becomes one more log row, and
`SELECT * FROM materialize_partitions(...)` (and, if it copied this shape, `regenerate_partitions`)
returns a *successful* result set that merely contains an error string. A misaligned or
non-tiling `regenerate_partitions(...)` call must not have this property: a scripted rollout
iterating the Open Questions' rollout query would otherwise see success for a partition that was
never actually regenerated.

`regenerate_partitions` therefore needs this plumbing to be able to carry an error. Rather than
duplicating the pair into a parallel "fallible" copy (~220 lines mirrored across two new types,
kept in sync with the originals forever), generalize the existing pair in place — the change is
small and behavior-preserving because the plain tuple type appears in exactly three places and
every existing producer funnels through one method:
- The channel item type becomes `Result<(DateTime<Utc>, String), String>` in the `TaskSpawner`
  alias (`task_log_exec_plan.rs:25-26`), `AsyncLogStream::rx` (`async_log_stream.rs:20,26`), and
  `LogSender::sender` (`response_writer.rs:52-59`).
- `LogSender::write_log_entry` (`response_writer.rs:64-70`) wraps its message in `Ok(...)`. That
  is the only way the existing spawners (`materialize_partitions_table_function.rs:91-110`,
  `retire_partitions_table_function.rs`) touch the channel, so both compile and behave unchanged —
  they never send an `Err` item, and their log-only error handling is deliberately left as-is.
- `AsyncLogStream::poll_next` batches `Ok` items into `(time, msg)` rows exactly as today, but on
  encountering an `Err(msg)` item yields
  `Poll::Ready(Some(Err(DataFusionError::Execution(msg))))` and ends the stream — a
  `RecordBatchStream` `Err` propagates through `execute_stream()`/`collect()` as a query execution
  error, which the FlightSQL layer surfaces to the client as a failed query, not a successful,
  possibly-empty result set.
- `LogStreamTableProvider` (`log_stream_table_provider.rs`) holds an opaque
  `Arc<TaskLogExecPlan>` with no coupling to the item type — no change.

`regenerate_partitions_table_function.rs`'s spawner keeps a clone of the raw channel sender
alongside the `LogSender` it wraps as its logger: ordinary progress lines flow through the logger
as `Ok((time, msg))` (matching `materialize_partitions`'s behavior) and, when
`regenerate_partition_range(...)` (including `verify_force_regeneration_alignment` and the
tiling check) returns `Err(e)`, it sends a single `Err(format!("{e:?}"))` item through the raw
sender before closing the channel, instead of only logging it.

Usage for this issue: `SELECT * FROM regenerate_partitions('blocks', <day_begin>, <day_end>,
86400);` for each active merged daily blocks partition — `<day_begin>`/`<day_end>`/`86400` must
exactly match that partition's existing boundaries (see the alignment invariant above); a range or
delta that doesn't will now fail loudly instead of creating a duplicate partition.

### 4. Recording per-partition sort guarantees in Postgres metadata

Pre-fix merged blocks partitions are not guaranteed sorted; new writes (part 2) and order-merged
partitions (part 1) are. Parquet footers are no longer stored in Postgres to recover this from —
`upgrade_v5_to_v6` (`migration.rs:418-426`) dropped the `partition_metadata` table entirely — so
the guarantee needs to be a first-class column on `lakehouse_partitions`, queryable in SQL for the
§3 rollout and readable from the partition cache at planning time with no footer fetch.

**Schema (v6→v7)**: bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` from 6 to 7 (`migration.rs:8`) and add
an `upgrade_v6_to_v7` function, wired into `execute_lakehouse_migration`'s chain the same way
`upgrade_v5_to_v6` is (`migration.rs:90-96`), that runs
`ALTER TABLE lakehouse_partitions ADD COLUMN sort_order TEXT[];` (nullable, no backfill needed)
and bumps `lakehouse_migration.version` to 7. `NULL` — the value every existing row reads back as
— means "no ordering guarantee", which is automatically correct for every partition written
before this change. A non-null value lists the guaranteed sort columns in order, ascending
implied, e.g. `{insert_time, block_id}`.

Deployment note: all partition-writing binaries must roll forward to the v7 build together with
this migration — a pre-upgrade binary's `insert_partition` builds a positional 13-value `INSERT`,
which fails against the new 14-column table, and a v6 binary refuses to even start against an
already-migrated v7 database (the `assert_eq!(current_version,
LATEST_LAKEHOUSE_SCHEMA_VERSION)` guards in `migrate_lakehouse` and `execute_lakehouse_migration`,
`migration.rs:48,97`) — so old and new binaries are not
cross-compatible and must not run concurrently against the same database during rollout.

**Value semantics** — the recorded guarantee is what the written file actually satisfies, not what
was declared to DataFusion for sort elision:
- Blocks partitions written fresh via `MetadataPartitionSpec::write` (part 2): `data_sql` ends
  `ORDER BY blocks.insert_time, blocks.block_id` (`blocks_view.rs:46`), so record
  `['insert_time', 'block_id']`.
- Blocks partitions written by the part-1 ordered merge: `['insert_time', 'block_id']` **only when
  every partition being merged already carries that exact `sort_order`** — `BlocksView` (Design §1)
  declares `[insert_time]` to DataFusion and takes the ordered path at all only when every entry in
  `partitions_to_merge` already has `sort_order == Some(['insert_time', 'block_id'])`; if even one
  input's `sort_order` is `NULL` (every pre-fix partition, including pre-fix *merged* partitions —
  see Design §1), the merge instead runs the plain unordered query with no declared ordering and
  records `NULL`, exactly like today. When every input does carry the guarantee, the elision
  argument in §1 establishes the output is fully `(insert_time, block_id)`-sorted — but this is not
  taken on faith at write time: because DataFusion's `validate_orderings` can silently drop an
  unprovable declared ordering and fall back to a real `SortExec` with no error, `merge.rs`'s
  ordered path (only — never the plain merger, see Design §1/Trade-offs on scoping) builds the
  optimized physical plan before executing it (mirroring the offline elision test) and fails the
  merge loudly with a descriptive error unless the plan is a single-partition plan containing
  **neither** a `SortExec` **nor** a `SortPreservingMergeExec` node — i.e. if elision did not fully
  happen, for whatever reason (config drift, a DataFusion upgrade, or `repartition_file_scans` not
  taking effect). This runtime check is the production enforcement of §1's correctness dependency;
  the offline test remains the fast regression signal. Only once both the input-`sort_order` gate
  and the plan-shape check pass does `create_merged_partition` record `['insert_time', 'block_id']`
  for that merge — otherwise it records `NULL` — so a false guarantee can never be persisted, and a
  transition-era merge that can't yet prove its inputs self-reports as unguaranteed instead of
  silently inheriting a guarantee it can't back up.
- Forced regeneration (part 3) goes through the same fresh-write path via `partition_spec.write()`,
  so it records `['insert_time', 'block_id']` automatically — no extra code needed at the
  regeneration call site.
- Every other view, and any unordered merge (every `View::merge_partitions` caller besides
  `BlocksView`, plus a `BlocksView` merge whose inputs aren't all yet guaranteed): `NULL`, unchanged.

The value must be threaded from the writer that already knows its own ordering — `MetadataPartitionSpec`
for fresh writes, `BlocksView` for merges — not hardcoded inside the shared `insert_partition`/
`write_partition_from_rows` plumbing, so a future view can opt in without those shared functions
knowing about blocks-view specifics.

**Plumbing**:
- `Partition` (`partition.rs:8-25`) gains `pub sort_order: Option<Vec<String>>`.
- `write_partition_from_rows` (`write_partition.rs:560-568`) gains a `sort_order:
  Option<Vec<String>>` parameter, threaded into the `Partition` literal it builds
  (`write_partition.rs:647-656`). `insert_partition`'s `INSERT` (`write_partition.rs:340-356`)
  gains a 14th value bound to it: `sort_order` is physically the last column (columns are
  appended in `ALTER TABLE` order, and `partition_format_version` was the v5 addition,
  `migration.rs:394-416`), so the statement becomes
  `INSERT INTO lakehouse_partitions VALUES($1, ..., $12, 2, $13);` with `$13` bound to
  `&partition.sort_order` — the pre-existing literal `2` (`partition_format_version`) stays
  ahead of it in the VALUES list, matching physical column order.
  - The 6 existing call sites of `write_partition_from_rows` all gain the new argument:
    `net_spans_view.rs:135-143`, `thread_spans_view.rs:138-146`, `sql_partition_spec.rs:92-100`,
    and `block_partition_spec.rs:86-94` pass `None` (no behavior change for these views).
    `metadata_partition_spec.rs:91-99` and `merge.rs:184-192` pass a real value, sourced as below.
  - `MetadataPartitionSpec` (`metadata_partition_spec.rs:19-27`) gains a `pub sort_order:
    Option<Vec<String>>` field, set via a new parameter on `fetch_metadata_partition_spec`
    (`metadata_partition_spec.rs:29-56`). `BlocksView::make_batch_partition_spec`
    (`blocks_view.rs:67-99`) passes `Some(vec!["insert_time".to_string(), "block_id".to_string()])`
    — exactly the ordering `data_sql`'s `ORDER BY` already guarantees (`blocks_view.rs:46`).
  - `View` (`view.rs:52-153`) gains a new method `fn get_merged_partition_sort_order(&self,
    _partitions_to_merge: &[Partition]) -> Option<Vec<String>> { None }` (default `None`, ignoring
    the argument). It takes the same partitions the merge is about to run over — the recorded value
    is a function of those specific inputs (Design §1), not a static per-view constant. This is a
    distinct concept from the existing `get_scan_output_ordering()` (`view.rs:150-152`): that one is
    a *trusted scan-ordering declaration for consumers*, deliberately left empty for blocks-view in
    this plan (see Design §1 and the Open Questions/Trade-offs on JIT trust); `get_merged_partition_sort_order()` is a
    *record of what this specific merge actually produced*, independent of what's declared to
    DataFusion for elision. `create_merged_partition` (`merge.rs:132-232`) calls
    `view.get_merged_partition_sort_order(&filtered_partitions)` (before or after `merge_partitions`
    — it's a pure function of the same input slice) and passes the result into its
    `write_partition_from_rows` call (`merge.rs:184-192`). `BlocksView` overrides the method to
    return `Some(vec!["insert_time".to_string(), "block_id".to_string()])` only when every partition
    in the given slice already has that exact `sort_order`, `None` otherwise — sharing the same
    predicate `BlocksView::merge_partitions` uses to pick its merger (see Design §1), so the two
    decisions can't diverge; every other view keeps the default `None`.
- `PartitionCache`'s 3 read paths in `partition_cache.rs` add `sort_order` to their `SELECT`
  column lists and `Partition` literals: `fetch_overlapping_insert_range` (query
  `partition_cache.rs:56-73`, construction `:105-114`),
  `fetch_overlapping_insert_range_for_view` (query `:136-152`, construction `:187-196`), and
  `LivePartitionProvider::fetch` (two query branches, `:344-364` and `:378-396`, one shared
  construction at `:430-439`). `sqlx` maps Postgres `TEXT[]` to `Option<Vec<String>>` directly
  (`r.try_get("sort_order")?`) — the same mapping already relied on for the `streams`/`processes`
  `tags` columns (`sql_arrow_bridge.rs:208-210`).
- `ListPartitionsTableProvider` (`list_partitions_table_function.rs`) adds `sort_order` to both
  `SELECT` query strings (`:107-124`, `:126-141`) and a matching field to `schema()`
  (`:52-88`): `Field::new("sort_order", DataType::List(Arc::new(Field::new("tag", DataType::Utf8,
  false))), true)`. The inner field name `"tag"` looks unrelated to sort columns but must match
  exactly what the generic `TEXT[]` column reader always produces regardless of the source
  column's name (`make_column_reader`'s `"TEXT[]"` arm, `sql_arrow_bridge.rs:350-357` — the same
  reader already used for the unrelated `tags` column, `blocks_view.rs:178-182`): `scan()` builds
  its `RecordBatch` via `rows_to_record_batch` (`sql_arrow_bridge.rs:371-396`), which derives field
  shapes from that reader rather than from `schema()`, and `MemorySourceConfig::try_new`
  (`list_partitions_table_function.rs:151-155`) requires the two to match field-for-field.

With this in place, the Open Question about which merged blocks partitions still need
regeneration becomes a SQL query rather than tribal knowledge (see Open Questions), and the
deferred JIT-consumer-trust follow-up (`tasks/jit_single_query_plan.md`) has a concrete,
footer-free way to check per-partition sort status before it declares
`(insert_time, block_id)` trusted (see Trade-offs).

## Implementation Steps

1. `rust/analytics/src/lakehouse/partitioned_execution_plan.rs`:
   - Add `OrderingBounds` enum and `partition_bounds` helper.
   - Thread `bounds: OrderingBounds` through `sort_and_check_non_overlapping`,
     `attach_ordering_statistics`, and `make_partitioned_execution_plan`.
2. `rust/analytics/tests/thread_spans_ordering_tests.rs`: update its 3 direct
   `make_partitioned_execution_plan` call sites (lines 77, 105, 147) to pass
   `OrderingBounds::EventTime` — required for the crate to compile against the new signature, not
   optional test-only cleanup.
3. `rust/analytics/src/lakehouse/materialized_view.rs`: pass `OrderingBounds::EventTime` at its
   `make_partitioned_execution_plan` call site.
4. `rust/analytics/src/lakehouse/partitioned_table_provider.rs`: add `output_ordering` +
   `ordering_bounds` fields, keep `new(...)` defaulting both to empty/`EventTime`, add
   `with_ordering(...)` constructor, thread both through `scan()`.
5. `rust/analytics/src/lakehouse/merge.rs`: add `with_merge_scan_ordering` builder method to
   `QueryMerger`; use `PartitionedTableProvider::with_ordering` in `execute_merge_query`. When the
   merger's declared scan ordering is non-empty, `execute_merge_query` must also: set
   `datafusion.optimizer.repartition_file_scans = false` on the session context (Design §1 point 3);
   and, before calling `execute_stream()`, build the optimized physical plan (`create_physical_plan`),
   inspect it (e.g. via `displayable(plan.as_ref()).indent(true)`) and return a descriptive `Err`
   instead of executing if it contains a `SortExec` or a `SortPreservingMergeExec` node anywhere
   (Design §1/§4's plan-shape check). Both of these are no-ops when the declared ordering is empty
   (every existing `View::merge_partitions` caller, plus `sql_batch_view.rs`'s aggregation merger),
   matching the scoping rationale in Trade-offs.
6. `rust/analytics/src/lakehouse/blocks_view.rs`: store two pre-built `QueryMerger`s — an ordered
   one (ordering = `[insert_time]`, i.e. the `Arc<String>`-wrapped `ScanSortColumn` from Design §1,
   query = `"SELECT * FROM source ORDER BY insert_time;"`) and the
   plain unordered one (empty ordering, query = `"SELECT * FROM source;"`, matching
   `View::merge_partitions`'s default); add a helper predicate over `partitions_to_merge` (every
   input already has `sort_order == Some(['insert_time', 'block_id'])`); override `merge_partitions`
   to delegate to the ordered merger when the predicate holds, otherwise to the plain merger
   (mirror `SqlBatchView::merge_partitions`'s delegation pattern). Reuse the same predicate in step
   18's `get_merged_partition_sort_order` override so the two decisions can't diverge.
7. `rust/analytics/src/lakehouse/metadata_partition_spec.rs`: rewrite `write()` to stream via
   `sqlx::query(...).fetch(...)`, flushing whenever the pending chunk's estimated size reaches
   `SOURCE_BYTES_PER_BATCH` (8 MB); add the `flush_chunk` and `estimate_row_bytes` helpers;
   switch `source_data_hash` to `self.get_source_data_hash()`.
8. `rust/analytics/src/lakehouse/batch_update.rs`: add `force: bool` to (private)
   `materialize_partition`; factor `materialize_partition_range`'s loop body into a new private
   `materialize_partition_range_impl(..., force: bool)`. `materialize_partition_range` keeps its
   existing signature, delegating to `materialize_partition_range_impl(..., force: false)`, so its
   9 existing callers (`rust/public/src/servers/maintenance.rs`, the `materialize_partitions` UDF
   in `materialize_partitions_table_function.rs`, plus 7 test call sites in `histo_view_test.rs`,
   `sql_view_test.rs`, `thread_spans_ordering_db_test.rs`) need no changes.
   Add a new `regenerate_partition_range(...)` (same signature) that calls
   `materialize_partition_range_impl(..., force: true)`. When `force`, `materialize_partition`
   skips only `verify_overlapping_partitions`'s source-hash freshness check, and skips the
   `get_max_partition_time_delta` subdivision check, so the whole requested range is written as
   one `CreateFromSource` partition (letting `retire_partitions` cleanly retire the existing
   partition it replaces). Add a new `verify_force_regeneration_alignment` function, called only
   on the `force` path in place of `verify_overlapping_partitions`, that re-checks the same
   partial-overlap condition (`begin < insert_range.begin || end > insert_range.end`) against
   existing partitions and returns a loud `Err` on a misaligned `insert_range`/delta instead of
   silently leaving a duplicate partition behind. `materialize_partition`'s existing
   `partition_spec.write(lakehouse.lake().clone(), logger)` call (`batch_update.rs`,
   `CreateFromSource` arm) gains the `force` argument it already has in scope, becoming
   `partition_spec.write(lakehouse.lake().clone(), logger, force)` — this is what actually reaches
   the in-transaction concurrency guard added in step 15/18 below.
9. Generalize the log-stream plumbing to carry an error (Design §3): change the channel item type
   to `Result<(DateTime<Utc>, String), String>` in `rust/analytics/src/dfext/task_log_exec_plan.rs`
   (the `TaskSpawner` alias), `rust/analytics/src/dfext/async_log_stream.rs` (`AsyncLogStream::rx`;
   `poll_next` batches `Ok` items into rows as today and turns an `Err(msg)` item into
   `Poll::Ready(Some(Err(DataFusionError::Execution(msg))))`, ending the stream), and
   `rust/analytics/src/response_writer.rs` (`LogSender::sender`; `write_log_entry` wraps its
   message in `Ok(...)`). The existing spawners in `materialize_partitions_table_function.rs` and
   `retire_partitions_table_function.rs` only touch the channel through
   `LogSender::write_log_entry`, so they need no changes. New
   `rust/analytics/src/lakehouse/regenerate_partitions_table_function.rs`: reuse
   `MaterializePartitionsTableFunction`'s argument parsing, spawn
   `regenerate_partition_range(...)`, send progress through a `LogSender`, and on failure send a
   single `Err(format!("{e:?}"))` item through a retained clone of the raw sender instead of only
   logging it. Declare the new module in `rust/analytics/src/lakehouse/mod.rs` (`pub mod
   regenerate_partitions_table_function;`) — required for it to compile into the crate.
10. `rust/analytics/src/lakehouse/query.rs`: register `regenerate_partitions` UDTF next to
    `materialize_partitions`.
11. `python/micromegas/micromegas/flightsql/client.py`: add a `regenerate_partitions(...)` method
    mirroring `materialize_partitions(...)` (same argument shape), issuing
    `SELECT * FROM regenerate_partitions(...)` instead.
12. Documentation: add `regenerate_partitions` alongside `materialize_partitions` in
    `mkdocs/docs/query-guide/functions-reference.md`, `mkdocs/docs/admin/maintenance.md`, and
    `mkdocs/docs/query-guide/python-api.md` (documenting the `client.regenerate_partitions(...)`
    method added in step 11).
13. `rust/analytics/src/lakehouse/migration.rs`: bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 7; add
    `upgrade_v6_to_v7` (`ALTER TABLE lakehouse_partitions ADD COLUMN sort_order TEXT[];` + bump
    `lakehouse_migration.version`), wired into `execute_lakehouse_migration`'s chain next to
    `upgrade_v5_to_v6`.
14. `rust/analytics/src/lakehouse/partition.rs`: add `pub sort_order: Option<Vec<String>>` to
    `Partition`.
15. `rust/analytics/src/lakehouse/write_partition.rs`: add a `sort_order: Option<Vec<String>>`
    parameter to `write_partition_from_rows`; thread it into the `Partition` literal and into
    `insert_partition`'s `INSERT` (new `$13` bind, physically after the existing literal `2`).
    Separately (Design §3's concurrency guard), add a `force: bool` parameter to both
    `write_partition_from_rows` and `insert_partition`, threaded from the former into the latter.
    Inside `insert_partition`, when `force` is `true`, after `retire_partitions` has run and the new
    partition row has been inserted (still inside the same transaction, before `commit`), run the
    one more `SELECT` against `lakehouse_partitions` described in Design §3 — general overlap
    predicate `begin_insert_time < insert_range.end AND end_insert_time > insert_range.begin`
    (matching `PartitionCache::filter`), excluding the just-inserted row's own exact bounds — and
    `bail!` if any row comes back, aborting the transaction. When `force` is `false` this recheck is
    skipped entirely (no behavior change for any existing caller).
16. Update all 6 existing `write_partition_from_rows` call sites for the new `sort_order` parameter:
    `net_spans_view.rs`, `thread_spans_view.rs`, `sql_partition_spec.rs`, and
    `block_partition_spec.rs` pass `None`; `metadata_partition_spec.rs` and `merge.rs` pass a real
    value per steps 17-18. The same 6 call sites also gain the new `force` parameter added in step
    15: `net_spans_view.rs`, `thread_spans_view.rs`, and `merge.rs`'s `create_merged_partition` pass
    `false` (unreachable under `force: true` — a forced regeneration always takes the
    `CreateFromSource` strategy, so `create_merged_partition` is never invoked from that path, per
    Design §3); `sql_partition_spec.rs`'s `SqlPartitionSpec::write` and `block_partition_spec.rs`'s
    `BlockPartitionSpec::write` plumb through the `force` value each now receives on its own
    `PartitionSpec::write` call (step 18) unchanged, rather than hardcoding `false`, so the guard
    still applies if a future or ad hoc `regenerate_partitions` call targets a non-blocks view
    backed by one of these specs; `metadata_partition_spec.rs`'s `MetadataPartitionSpec::write`
    plumbs through the `force` value it receives the same way.
17. `rust/analytics/src/lakehouse/metadata_partition_spec.rs`: add `pub sort_order:
    Option<Vec<String>>` to `MetadataPartitionSpec` and a matching parameter to
    `fetch_metadata_partition_spec`; pass it through to `write_partition_from_rows` in `write()`.
    `rust/analytics/src/lakehouse/blocks_view.rs`: pass
    `Some(vec!["insert_time".to_string(), "block_id".to_string()])` at its
    `fetch_metadata_partition_spec` call site.
18. `rust/analytics/src/lakehouse/view.rs`: add `get_merged_partition_sort_order(&self,
    partitions_to_merge: &[Partition]) -> Option<Vec<String>> { None }` to the `View` trait.
    `rust/analytics/src/lakehouse/merge.rs`: call `view.get_merged_partition_sort_order(&filtered_partitions)`
    in `create_merged_partition` and pass the result to `write_partition_from_rows`.
    `rust/analytics/src/lakehouse/blocks_view.rs`: override it to return
    `Some(vec!["insert_time".to_string(), "block_id".to_string()])` only when every partition in the
    slice already has that `sort_order`, `None` otherwise — the same predicate step 6 uses.
    Separately, in the same file, add a `force: bool` parameter to the `PartitionSpec::write` trait
    method (`fn write(&self, lake: Arc<DataLakeConnection>, logger: Arc<dyn Logger>, force: bool) ->
    Result<()>`), and update its three implementations to match:
    `rust/analytics/src/lakehouse/block_partition_spec.rs`'s `BlockPartitionSpec::write` and
    `rust/analytics/src/lakehouse/sql_partition_spec.rs`'s `SqlPartitionSpec::write` accept the new
    `force` parameter and pass it straight through to their `write_partition_from_rows` call (step
    16); `rust/analytics/src/lakehouse/metadata_partition_spec.rs`'s `MetadataPartitionSpec::write`
    does the same. There are two production call sites of `PartitionSpec::write` that must be
    updated for the new parameter: `materialize_partition` (step 8, `batch_update.rs:171`) is what
    supplies a real `force` value; `write_partition_from_blocks`'s `block_spec.write(...)` call
    (`rust/analytics/src/lakehouse/jit_partitions.rs:632`) is the other — it passes `force: false`,
    since JIT partition generation at query time never forces regeneration.
19. `rust/analytics/src/lakehouse/partition_cache.rs`: add `sort_order` to the `SELECT` column
    lists and `Partition` literals in `fetch_overlapping_insert_range`,
    `fetch_overlapping_insert_range_for_view`, and `LivePartitionProvider::fetch` (both query
    branches).
20. `rust/analytics/src/lakehouse/list_partitions_table_function.rs`: add `sort_order` to both
    `SELECT` query strings and add the matching `List(Utf8)` field to `schema()`.
21. `rust/analytics/tests/thread_spans_ordering_tests.rs`: add `sort_order: None` to the
    `make_partition()` test helper (`:34-47`) — required for the crate to compile against the new
    `Partition` field.
22. Documentation: add the `sort_order` column to `list_partitions()`'s column table in
    `mkdocs/docs/admin/functions-reference.md`.
23. Tests (see Testing Strategy).
24. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.
25. Manual verification (see Testing Strategy) against a running environment with an existing
    unsorted merged blocks partition.

## Files to Modify

- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` — `OrderingBounds`, generalized
  bounds helper.
- `rust/analytics/tests/thread_spans_ordering_tests.rs` — update its 3
  `make_partitioned_execution_plan` call sites to pass `OrderingBounds::EventTime`; add
  `sort_order: None` to the `make_partition()` helper.
- `rust/analytics/src/lakehouse/materialized_view.rs` — pass `OrderingBounds::EventTime`.
- `rust/analytics/src/lakehouse/partitioned_table_provider.rs` — ordering-aware constructor.
- `rust/analytics/src/lakehouse/merge.rs` — `QueryMerger` ordering builder method; when the declared
  scan ordering is non-empty, `execute_merge_query` sets
  `datafusion.optimizer.repartition_file_scans = false` and fails the merge loudly (before
  executing) unless the optimized physical plan contains neither a `SortExec` nor a
  `SortPreservingMergeExec` (Design §1/§4); call `view.get_merged_partition_sort_order(&filtered_partitions)`
  in `create_merged_partition` and pass it to `write_partition_from_rows`; pass `false` for the new
  `force` parameter (unreachable under forced regeneration, per Design §3).
- `rust/analytics/src/lakehouse/blocks_view.rs` — ordered vs. plain merger selection in
  `merge_partitions`, keyed on `partitions_to_merge`'s recorded `sort_order`; pass a declared
  `sort_order` to `fetch_metadata_partition_spec`; override `get_merged_partition_sort_order` with
  the same input-dependent predicate.
- `rust/analytics/src/lakehouse/metadata_partition_spec.rs` — streaming `write()`; add
  `sort_order` field/parameter and thread it to `write_partition_from_rows`; accept the
  `PartitionSpec::write` trait's new `force: bool` parameter and plumb it through to
  `write_partition_from_rows` too.
- `rust/analytics/src/lakehouse/batch_update.rs` — `force` parameter, new
  `materialize_partition_range_impl` + `regenerate_partition_range` (existing
  `materialize_partition_range` signature unchanged), new
  `verify_force_regeneration_alignment` guard for the `force` path; thread `force` into
  `materialize_partition`'s `partition_spec.write(...)` call.
- `rust/analytics/src/dfext/task_log_exec_plan.rs`, `rust/analytics/src/dfext/async_log_stream.rs`,
  `rust/analytics/src/response_writer.rs` — generalize the log-stream channel item type to
  `Result<(DateTime<Utc>, String), String>` so an `Err` item ends the query with an error instead
  of a log row; `LogSender::write_log_entry` wraps in `Ok`, existing spawners unchanged.
- `rust/analytics/src/lakehouse/regenerate_partitions_table_function.rs` — new.
- `rust/analytics/src/lakehouse/mod.rs` — declare the new module (`pub mod
  regenerate_partitions_table_function;`).
- `rust/analytics/src/lakehouse/query.rs` — register new UDTF.
- `python/micromegas/micromegas/flightsql/client.py` — add `regenerate_partitions(...)` method.
- `mkdocs/docs/query-guide/functions-reference.md` — document `regenerate_partitions`.
- `mkdocs/docs/admin/maintenance.md` — document `regenerate_partitions`.
- `mkdocs/docs/query-guide/python-api.md` — document `regenerate_partitions`.
- `rust/analytics/src/lakehouse/migration.rs` — v6→v7 migration adding
  `lakehouse_partitions.sort_order`.
- `rust/analytics/src/lakehouse/partition.rs` — `Partition::sort_order` field.
- `rust/analytics/src/lakehouse/write_partition.rs` — `sort_order` parameter on
  `write_partition_from_rows`; new `insert_partition` bind. Also a `force: bool` parameter on both
  `write_partition_from_rows` and `insert_partition`, and the in-transaction concurrent-write
  overlap recheck (Design §3) inside `insert_partition`, gated on `force`.
- `rust/analytics/src/lakehouse/view.rs` — new `get_merged_partition_sort_order()` method; add
  `force: bool` to the `PartitionSpec::write` trait method signature.
- `rust/analytics/src/lakehouse/partition_cache.rs` — read `sort_order` in all 3
  partition-fetching query paths.
- `rust/analytics/src/lakehouse/list_partitions_table_function.rs` — expose `sort_order` column.
- `rust/analytics/src/lakehouse/net_spans_view.rs`,
  `rust/analytics/src/lakehouse/thread_spans_view.rs`,
  `rust/analytics/src/lakehouse/sql_partition_spec.rs`,
  `rust/analytics/src/lakehouse/block_partition_spec.rs` — pass `None` for `sort_order` at their
  `write_partition_from_rows` call sites (new parameter, no behavior change).
  `sql_partition_spec.rs` (`SqlPartitionSpec::write`) and `block_partition_spec.rs`
  (`BlockPartitionSpec::write`) additionally accept the `PartitionSpec::write` trait's new `force:
  bool` parameter and plumb it straight through to their own `write_partition_from_rows` call.
- `rust/analytics/src/lakehouse/jit_partitions.rs` — `write_partition_from_blocks`'s
  `block_spec.write(...)` call (`:632`) is a second production call site of `PartitionSpec::write`
  (alongside `materialize_partition` in `batch_update.rs`); update it to pass `force: false`.
- `mkdocs/docs/admin/functions-reference.md` — document the `sort_order` column in
  `list_partitions()`'s column table.

## Trade-offs

- **`ORDER BY` + declared scan ordering vs. `ORDER BY` alone.** An `ORDER BY` with no declared
  source ordering would still produce correct output but pay a full buffering `SortExec` on
  every merge — the exact problem `tasks/jit_single_query_plan.md` was written to avoid on the
  query side. Declaring the `insert_time` ordering lets DataFusion elide the sort node entirely
  instead (the merge source scan is a single sequential file group, so there is no second stream
  requiring a `SortPreservingMergeExec`), so the merge itself gains the same memory bound this
  plan is adding to the source write path — see Design §1 for why this elision is also what makes
  the `(insert_time, block_id)` ordering guarantee correct, not just cheap.
- **Scoping `repartition_file_scans = false` and the plan-shape check to ordering-declared merges
  only, vs. applying both to every merge.** `QueryMerger::execute_merge_query` (`merge.rs:65-98`)
  is shared with `SqlBatchView`'s aggregation mergers (`GROUP BY` queries) and `View::merge_partitions`'s
  own default plain merger — neither declares a scan ordering. Disabling `repartition_file_scans`
  for the blocks-view ordered merge costs nothing, because preserving global order already forces
  that scan into one sequential file group regardless — but for an aggregation merge, file groups
  can otherwise be split/rebalanced across `target_partitions` to parallelize the scan and any
  partial aggregation sitting upstream of the single merged output stream; disabling the setting
  globally would give that up for no benefit. Likewise, an unconditional plan-shape check would
  misfire on any legitimate `ORDER BY` merge query with a real `Sort` node. Both are therefore
  gated on the merger's declared scan ordering being non-empty (Design §1 point 3, §4).
- **Generalizing `OrderingBounds` vs. a separate insert-time-only code path.** A parallel
  `sort_and_check_non_overlapping_by_insert_time` function would duplicate ~40 lines with one
  field access changed. An enum parameter keeps one implementation and makes the event-time
  behavior's non-regression explicit (`OrderingBounds::EventTime` at every existing call site).
- **`force: bool` on `materialize_partition_range` vs. a standalone regeneration code path.**
  Reimplementing the write/swap plumbing outside `batch_update.rs` would duplicate
  `materialize_partition`'s partition-spec/write/retire logic. A boolean that skips two
  decisions — the up-to-date check and the `get_max_partition_time_delta` subdivision check — is
  a small, localized behavioral change that reuses everything else, including the streaming fix
  from part 2 and the atomic retire-then-insert swap in `insert_partition`.
- **Generalizing `TaskLogExecPlan`/`AsyncLogStream`/`LogSender` to a `Result` channel item vs. a
  parallel "fallible" copy.** A mirrored `FallibleTaskLogExecPlan`/`FallibleAsyncLogStream` pair
  would leave the existing files untouched, but at the cost of ~220 duplicated lines and two
  nearly identical implementations to keep in sync. The plain tuple type appears in exactly three
  places (the `TaskSpawner` alias, `AsyncLogStream::rx`, `LogSender::sender`), `AsyncLogStream` is
  constructed only by `TaskLogExecPlan::execute`, and both existing spawners touch the channel
  only through `LogSender::write_log_entry` — so wrapping in `Ok` there makes the generalization
  behavior-preserving for every existing caller, which simply never sends an `Err` item.
- **Chunked `sqlx` row streaming vs. a Postgres `DECLARE CURSOR` / `COPY`-based approach.**
  `sqlx::query(...).fetch(...)` already streams rows off the wire without server-side cursor
  management.
- **Byte-based flush threshold vs. a row-count chunk.** A row count (the per-count precedent
  elsewhere, e.g. `JitPartitionConfig::max_nb_objects`) is simpler to compute but bounds nothing
  when a handful of rows carry MB-sized `properties`/`objects_metadata` payloads — exactly the
  variance blocks-view's joined JSONB/binary columns exhibit. Flushing on estimated accumulated
  bytes (`SOURCE_BYTES_PER_BATCH` = 8 MB) mirrors the Parquet writer's own byte-based 100 MB
  threshold and bounds memory by construction. It is deliberately the *only* flush metric — a
  secondary row-count cap would bound nothing the byte metric doesn't and would just add a second
  knob. 8 MB sits well past the per-flush-overhead knee (one Arrow conversion, one time-bounds
  pass, one channel send per flush) while keeping chunk-side peak (~2× chunk during Arrow
  conversion) far below the writer's own 100 MB in-progress buffer, which dominates peak memory
  regardless; anything in the ~8–64 MB band behaves near-identically, and the output file is
  byte-identical for any chunk size (the writer accumulates chunks into its own row groups).
- **Not declaring the JIT-consumer-side `(insert_time, block_id)` ordering in this plan.** Doing
  so before every active merged partition is regenerated would silently mis-group blocks for any
  partition still written under the old, unordered merge — exactly the failure mode
  `sort_and_check_non_overlapping` is designed to catch loudly for *new* overlaps, but it cannot
  detect "sorted-looking file that just happens to be wrong inside its own bounds." Declaring
  trust is a rollout step gated on regenerating and verifying every affected partition, not a
  code change bundled with this plan. Design §4's `sort_order` column turns that gate from a
  flag-day, all-or-nothing trust decision into a per-partition, footer-free check: the follow-up
  plan can require `sort_order = ['insert_time', 'block_id']` on every partition in a query's
  scope (already loaded in the partition cache at planning time) before trusting the declared
  ordering for that scope, instead of trusting it globally once every partition happens to have
  been regenerated.
- **A `sort_order TEXT[]` column vs. re-deriving the guarantee from data.** Re-checking whether a
  partition happens to be sorted (e.g. re-running the Testing Strategy's `lag()`-based query
  against every partition on every planning decision) would be correct but is exactly the kind of
  full-file-scan cost this plan is trying to avoid; footers that used to make an inexpensive check
  possible are gone (`upgrade_v5_to_v6`, `migration.rs:418-426`). Recording the guarantee once, at
  write time, in the same row that already carries `min_event_time`/`max_event_time`/`num_rows`,
  makes it as cheap to consult as any other planning-time partition statistic.
- **Threading `sort_order` from the writer (`MetadataPartitionSpec`, `View::get_merged_partition_sort_order`)
  vs. hardcoding it in `insert_partition`/`write_partition_from_rows`.** The shared write path has
  no way to know, from a `RecordBatch` stream alone, whether its rows are actually sorted or by
  what columns — that knowledge only exists where the query/`ORDER BY` that produced the stream is
  defined. A hardcoded `if view_set_name == "blocks"` check in the shared path would work today but
  would need updating for every future view that wants the same guarantee; a per-view/per-spec
  value keeps `write_partition_from_rows` and `insert_partition` generic, matching how
  `source_data_hash` and `get_scan_output_ordering()` are already supplied by the caller rather
  than inferred centrally.
- **A separate `get_merged_partition_sort_order()` vs. reusing `QueryMerger`'s declared
  `with_merge_scan_ordering`.** The two are not the same value for blocks-view: the declared
  ordering passed to DataFusion for elision is only `[insert_time]` (§1 explains why a two-column
  declaration would never validate), but the *actual* guarantee the elided, disjoint, pre-sorted
  merge produces is the fuller `[insert_time, block_id]`. Reusing the elision-declaration field
  would either under-record the guarantee (just `[insert_time]`, losing the `block_id` tie-break
  fact §1 establishes) or conflate two different contracts (a DataFusion validation input vs. a
  Postgres-recorded fact about the output). A separate method keeps them independently correct.

## Testing Strategy

- **Offline ordering tests** (new `rust/analytics/tests/blocks_view_merge_ordering_tests.rs`,
  following the existing no-DB pattern in `thread_spans_ordering_tests.rs`): build synthetic
  insert-time-disjoint `Partition`s, confirm `make_partitioned_execution_plan` under
  `OrderingBounds::InsertTime` with a single-column `insert_time` declared ordering elides the
  `Sort` node entirely (no `SortExec`, no `SortPreservingMergeExec` — one sequential file group
  already satisfies the ordering) with `datafusion.optimizer.repartition_file_scans = false` set on
  the session, and confirm an insert-time overlap is rejected loudly (negative control, mirroring
  `overlapping_partitions_are_rejected`). Include the single-non-empty-file merge shape explicitly
  — a source with exactly one non-empty partition (the others empty, dropped before scan
  construction) — asserting it still elides to a plain concatenation rather than falling back to
  `repartition_evenly_by_size` + `SortPreservingMergeExec`. This elision assertion is a
  correctness check, not a performance one: per Design §1, the merged output is only guaranteed
  fully `(insert_time, block_id)`-sorted when the sort is elided (a no-op concatenation of
  already-sorted, disjoint files); if elision ever regressed to a real `insert_time`-only
  `SortExec`, the `block_id` tie-break order would silently stop being guaranteed, alongside the
  loss of the memory bound.
- **`MetadataPartitionSpec` streaming unit tests**: exercise `write()` (or the extracted
  `flush_chunk` helper) against a small `Vec<PgRow>`-free scenario if feasible, or an integration
  test against a live Postgres fixture, with enough data to cross the `SOURCE_BYTES_PER_BATCH`
  threshold at least twice (or the threshold lowered for the test), asserting: chunk boundaries
  don't drop/duplicate rows,
  the produced partition's row count matches `record_count`, and the emitted `RecordBatch`(es)
  concatenate to the same content as today's single-batch `fetch_all` path (a semantic
  equivalence test, not a performance one).
- **`cargo test`**: full suite must pass (`thread_spans_ordering_tests.rs`'s 3
  `make_partitioned_execution_plan` call sites are updated per Implementation Steps to pass
  `OrderingBounds::EventTime`; its behavioral assertions are otherwise unchanged — regression on
  `write_partition_tests.rs`, `partition_metadata_tests.rs`, etc.).
- **Force-regeneration alignment guard test**: a DB-backed test (alongside the existing
  `materialize_partition_range` tests, e.g. `thread_spans_ordering_db_test.rs`-style) asserting
  `regenerate_partition_range`/`verify_force_regeneration_alignment` returns an `Err` when the
  requested `(begin, end)` partially overlaps an existing partition instead of exactly containing
  it (e.g. a daily partition regenerated with `delta=3600`), and succeeds when the range exactly
  matches the partition's boundaries — confirming the guard fails loudly rather than silently
  leaving a duplicate partition. Also assert `regenerate_partition_range` returns an `Err` upfront,
  before any partition is written, when `delta` does not exactly tile `(begin, end)` (e.g. a `delta`
  longer than the range, or one that leaves a remainder) — confirming the non-tiling case is
  rejected loudly instead of silently regenerating a partial or empty span.
- **Forced-regeneration concurrent-write race test**: a DB-backed test that simulates the race in
  Design §3's "Guarding against a concurrent daemon write" in *both* overlap directions:
  (a) insert a partition *fully contained within* the target range (standing in for the daemon's
  overlapping hourly write, e.g. `[h, h+1)` inside a regenerated `[d, d+1)`) *after*
  `verify_force_regeneration_alignment`'s snapshot would have been taken but *before* the forced
  `insert_partition` call commits; and (b) the reverse — regenerate a smaller range (e.g.
  `[h, h+1)`) while a concurrently-committed partition that *contains* it (e.g. a daemon daily
  `[d, d+1)` where `d` spans `h`) commits in that same window. In both cases, assert the forced
  write's in-transaction overlap recheck returns an `Err` (rather than committing) and that no
  duplicate/overlapping partition rows remain afterwards — both the pre-existing partition being
  replaced and the injected concurrent one should still be exactly and only what they were before
  the failed forced write, confirming the transaction rolled back cleanly rather than partially
  applying, in either direction.
- **Query-level failure test for `regenerate_partitions`**: a query-level test (running the UDTF
  through `ctx.sql(...)`/FlightSQL, not just calling `regenerate_partition_range` directly) asserting
  that `SELECT * FROM regenerate_partitions(...)` with a misaligned range/delta returns a query
  `Err` to the caller — not a successful, possibly-empty result set containing only a log row with
  the error text. This is the behavior `materialize_partitions`'s log-only spawner deliberately
  does not provide (Design §3); the test should fail if `regenerate_partitions`'s spawner is ever
  refactored to only log its errors instead of sending the `Err` item.
- **Manual regeneration + memory check**: start services
  (`python3 local_test_env/ai_scripts/start_services.py` or monolith), find a real merged blocks
  partition, run `SELECT * FROM regenerate_partitions('blocks', <begin>, <end>, <delta>);`, and
  confirm: the partition's `updated`/`file_path` change, its row content is unchanged (same
  source-hash-derived count), and process RSS (system_monitor gauges from #1330) stays flat
  during regeneration of a busy day instead of spiking with the range width — the core check for
  the OOM concern this plan addresses.
- **Sortedness verification query** — run against a regenerated partition to confirm ordering.
  `lag(...) OVER ()` has no `ORDER BY`, so its result depends on physical row arrival order; this
  must be pinned to a single, non-repartitioned scan (default settings otherwise split the parquet
  scan into byte-range partitions and interleave them via `CoalescePartitionsExec`, producing false
  failures against a perfectly sorted partition). `SET` statements don't persist across FlightSQL
  statements (`execute_query` in `flight_sql_service_impl.rs` and `query()` in
  `lakehouse/query.rs` both build a fresh `make_session_context` per statement), so this cannot be
  run as a two-statement `SET; SELECT;` through the query service (CLI/python client/Grafana) —
  the `SET`s would configure a context that is discarded before the `SELECT` runs, silently
  falling back to default repartitioning. Instead, pin the settings within one session, either:
  - a small Rust test/tool that calls `make_session_context` with a `SessionConfigurator` that
    sets `datafusion.execution.target_partitions = 1` and
    `datafusion.optimizer.repartition_file_scans = false` on the `SessionContext` before running
    the query below, in the same session; or
  - read the regenerated partition's parquet file directly (outside the query service) with
    pyarrow or `datafusion-cli`, where the file is read as one sequential stream:
  ```sql
  SELECT count(*) FROM (
    SELECT insert_time, block_id,
           lag(insert_time) OVER () AS prev_insert_time,
           lag(block_id) OVER () AS prev_block_id
    FROM view_instance('blocks', 'global')
    WHERE insert_time >= $1 AND insert_time < $2
  ) t
  WHERE (prev_insert_time, prev_block_id) > (insert_time, block_id);
  ```
  A non-zero count means the partition is still out of order (was not regenerated, or the merge
  fix has a bug). This is the "verifiable per partition" check referenced in the GitHub issue —
  run it per merged partition before any future plan declares
  `(insert_time, block_id)` a trusted consumer-side scan ordering.
- **Migration test**: a DB-backed test — the first test exercising `migrate_lakehouse`/
  `execute_lakehouse_migration` (no existing migration coverage exists to run alongside), using the
  same live-Postgres env-var harness as `histo_view_test.rs`. It runs `migrate_lakehouse` to bring a
  fresh database to the latest schema (v7), inserts a `lakehouse_partitions` row, then simulates a
  pre-existing v6 database by dropping the `sort_order` column
  (`ALTER TABLE lakehouse_partitions DROP COLUMN sort_order;`) and setting
  `lakehouse_migration.version` back to 6. Re-running `migrate_lakehouse` from that simulated v6
  state must bring `lakehouse_migration.version` back to 7, recreate the `sort_order` column, and
  read the pre-inserted row back with `sort_order = NULL` (no ordering guarantee) — the
  "automatically correct for every existing partition" claim in Design §4.
- **`sort_order` recording tests**: extend the offline blocks-view merge ordering tests (above) and
  a `MetadataPartitionSpec`/DB-backed test to assert: (a) a freshly materialized `BlocksView`
  partition (via `MetadataPartitionSpec::write`) is inserted with
  `sort_order = Some(['insert_time', 'block_id'])`; (b) an order-merged blocks partition (via
  `create_merged_partition` under §1), where every input partition already has
  `sort_order = Some(['insert_time', 'block_id'])`, is inserted with the same value; (c) a partition
  from another view (e.g. `ThreadSpansView`, or any view exercised by the existing
  `histo_view_test.rs`/`sql_view_test.rs` suites) is inserted with `sort_order = None`; (d) a
  `BlocksView` merge where at least one input partition has `sort_order = None` (simulating a
  pre-fix merged hourly) does not declare a scan ordering, runs the plain unordered merger, and is
  inserted with `sort_order = None` — confirming the merge self-reports as unguaranteed instead of
  inheriting a false guarantee from its sibling, guaranteed-sorted inputs.
- **`list_partitions()` exposure test**: a query-level test asserting `SELECT sort_order FROM
  list_partitions()` returns the column with the expected type and values for a mix of blocks-view
  and other-view partitions — confirming the `ListPartitionsTableProvider` schema/query change
  and the generic `TEXT[]` reader agree (no `DataFusionError` from a schema mismatch).

## Open Questions

- Which merged blocks partitions are "active" today and need `regenerate_partitions` run against
  them? Design §4's `sort_order` column makes this SQL-answerable —
  `SELECT * FROM list_partitions() WHERE view_set_name = 'blocks' AND sort_order IS NULL` lists
  exactly the partitions still lacking the guarantee. The rollout should additionally exclude, via
  a `begin_insert_time` filter, any partition whose insert-time range falls within one partition
  width of the ingestion retention horizon: its source rows in Postgres may already be partially
  aged out, so regenerating it would (per Design §3's accepted "smaller partition outside
  retention is fine" behavior) truncate data that is still queryable for up to that window, rather
  than merely reproduce a smaller-but-equivalent partition. Because the merge only declares the ordering
  and records it when every input already carries it (Design §1/§4), this correctly includes not
  just pre-fix partitions written before this change, but also any *post-fix* merge whose
  inputs were still unguaranteed at merge time — those self-report `NULL` too rather than
  being missed by this query. The rollout is therefore: run `regenerate_partitions` finest
  granularity first, coarsest last (minutely, then hourly, then daily — the daemon's merge cascade,
  `maintenance.rs:68-174`), because a merged partition built from inputs that were `NULL` at the
  time it was merged stays `NULL` itself until it is regenerated or re-merged, even after its
  inputs are later fixed — re-running the query after each pass shows the remaining work. Once every existing partition reaches `sort_order = ['insert_time', 'block_id']`,
  every subsequent ordinary (non-forced) daily merge sees all-guaranteed hourly inputs and
  automatically takes the ordered path itself, propagating the guarantee forward with no further
  manual regeneration. Running `regenerate_partitions` over whatever the query returns in
  production is still an operational rollout step, tracked separately (not blocking landing the
  code).
- Does `estimate_row_bytes`'s raw-column-length sum track real allocator footprint closely enough
  under blocks-view's widest real-world `streams.properties`/`processes.properties` payloads? The
  8 MB threshold itself needs no precision (anything in the ~8–64 MB band behaves the same, and
  the Parquet writer's 100 MB buffer dominates peak memory regardless) — the check is only that
  the estimate isn't off by an order of magnitude; worth a sanity look once this lands.
- Declaring `(insert_time, block_id)` as a trusted `get_scan_output_ordering()` for
  blocks-view/JIT consumers is intentionally deferred to a follow-up plan (per
  `tasks/jit_single_query_plan.md`'s Open Questions), gated on the regeneration rollout above. That
  follow-up can use Design §4's `sort_order` column as its gate — checking
  `sort_order = ['insert_time', 'block_id']` on the partitions in a query's scope, already loaded
  in the partition cache at planning time — instead of a global flag-day trust decision (see
  Trade-offs).
