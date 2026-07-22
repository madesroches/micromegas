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

Three coupled changes:
1. Make blocks-view merges order-preserving (declared scan ordering + explicit `ORDER BY`), so
   merged partitions stay internally sorted by `(insert_time, block_id)` going forward.
2. Make the Postgres-source partition write path (`MetadataPartitionSpec::write`, used by
   `BlocksView::make_batch_partition_spec`) stream in bounded chunks instead of
   `fetch_all`-ing a whole insert range into memory.
3. Add an admin table function to force-regenerate existing (already-merged, potentially
   unsorted) partitions online from the Postgres source tables, reusing the now-streaming write
   path — no downtime, no schema change.

Declaring `(insert_time, block_id)` as a *trusted* scan ordering for blocks-view consumers (the
JIT partition generators, per `tasks/jit_single_query_plan.md`) is explicitly **out of scope**
here — it only becomes safe after every active merged partition has been regenerated under (1),
which is an operational rollout step, not a code change. See Open Questions.

## Current State

### Merges are not order-preserving today

`BlocksView` (`rust/analytics/src/lakehouse/blocks_view.rs`) writes fresh partitions sorted —
`data_sql` ends with `ORDER BY blocks.insert_time, blocks.block_id` (`blocks_view.rs:44`). But
`BlocksView` does not override `merge_partitions`, so it gets `View::merge_partitions`'s default
(`view.rs:101-117`): a `QueryMerger` running `SELECT * FROM source;` — no `ORDER BY` — over a
`PartitionedTableProvider` source table built with **no declared scan ordering**
(`merge.rs:81-86` always constructs `PartitionedTableProvider::new(...)`, whose `scan()` always
passes `&[]` for `output_ordering`, `partitioned_table_provider.rs:63-70`). DataFusion is free to
interleave file reads from the source partitions in any order, so a merged daily blocks
partition is not guaranteed sorted, even though its inputs are.

This matters because `partitioned_execution_plan.rs`'s declared-ordering machinery — used today
by `MaterializedView` (`materialized_view.rs:94`, via `View::get_scan_output_ordering`) for
per-view consumer queries (e.g. `ThreadSpansView`, `thread_spans_view.rs:357`) — is entirely
event-time-based: `sort_and_check_non_overlapping` and `attach_ordering_statistics`
(`partitioned_execution_plan.rs:27,58`) read `Partition::min_event_time()`/`max_event_time()`.
Blocks-view ordering is insert-time-based (`Partition::begin_insert_time()`/`end_insert_time()`,
`partition.rs:44-51` — always `Some`, unlike the `Option` event-time bounds), and
`PartitionedTableProvider` never plumbs any ordering into its source scan at all. Neither piece
of existing infrastructure covers the merge source table today.

### The Postgres-source write path buffers a full insert range

`BlocksView::make_batch_partition_spec` (`blocks_view.rs:67-93`) delegates to
`fetch_metadata_partition_spec` (`metadata_partition_spec.rs:29-53`), which runs a `COUNT(*)`
query up front (cheap, one row) and returns a `MetadataPartitionSpec` holding that count and the
sorted `data_sql`. The actual data fetch happens later, in
`MetadataPartitionSpec::write` (`metadata_partition_spec.rs:66-108`):

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
`TimeDelta::days(1)`, `view.rs:127-129`) — as one `Vec<PgRow>`, then `rows_to_record_batch`
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
- `create_merged_partition` (`merge.rs:127-220`) consumes the merge's `SendableRecordBatchStream`
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
registered in `query.rs:120-131`) calls `batch_update::materialize_partition_range`
(`batch_update.rs:195-215`), which for each `partition_time_delta`-sized bucket calls
`materialize_partition` (`batch_update.rs:98-176`). That function always runs
`verify_overlapping_partitions` (`batch_update.rs:20-91`) first, which compares the *source data
hash* (an object count) against existing partitions' stored hashes and returns
`PartitionCreationStrategy::Abort` when they already match (`batch_update.rs:88-90`, "already up
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
  `SqlBatchView::merge_partitions`, `sql_batch_view.rs:249-266`, which just delegates to a
  pre-built merger) to build and reuse a `QueryMerger` configured with:
  - query: `"SELECT * FROM source ORDER BY insert_time;"`
  - ordering: `[ScanSortColumn { column: "insert_time", descending: false }]`

  Only `insert_time` is declared (not `block_id`), and the query's `ORDER BY` matches it exactly:
  `attach_ordering_statistics` can only attach min/max stats for the leading declared column, and
  `Partition` metadata has no `block_id` bounds to attach — a two-column declared ordering would
  never validate (DataFusion 54 requires present min/max stats for *every* declared sort column)
  and DataFusion would fall back to a full buffering `SortExec`. A single-column `insert_time`
  ordering is still sufficient for full `(insert_time, block_id)` correctness: source partitions
  are non-overlapping in insert_time by construction and each is already sorted by
  `(insert_time, block_id)` (`blocks_view.rs:44`), so once the source scan's single sequential
  file group (see below) has a validated `insert_time` ordering, its file-by-file concatenation is
  already exactly the order the merge needs — no second declared column is required to reach that
  result.

  `create_merged_partition` already calls `filtered_partitions.sort_by_key(|p|
  p.begin_insert_time())` before invoking `view.merge_partitions` (`merge.rs:158`), and
  time-sliced partitions are non-overlapping in insert_time by construction, so
  `sort_and_check_non_overlapping` under `OrderingBounds::InsertTime` should always pass for
  well-formed input — a failure here would indicate a genuine partitioning bug, matching the
  existing "fail loudly" philosophy for the event-time case.

  Why the merged output ends up fully `(insert_time, block_id)`-sorted, not just
  `insert_time`-sorted:
  1. Each input file is already internally `(insert_time, block_id)`-sorted — it was written from
     `data_sql`'s `ORDER BY blocks.insert_time, blocks.block_id` (`blocks_view.rs:44`).
  2. Input partitions have disjoint, half-open `[begin, end)` insert-time ranges by construction,
     so no two files can contain equal-`insert_time` rows — ties never need to be broken *across*
     files, only within one (already-sorted) file.
  3. `make_partitioned_execution_plan` puts every source file into one sequential file group, and
     DataFusion validates the declared `[insert_time]` ordering against each file's min/max
     insert-time stats; validation only passes if the files are non-overlapping and already in
     ascending order. Once validated, `EnforceSorting` proves the `ORDER BY insert_time` is
     already satisfied and elides the sort node entirely — the plan becomes the plain in-order
     concatenation of the files, with no `SortExec` and no `SortPreservingMergeExec` (there is only
     one file group, hence one stream; nothing to k-way merge).

  Point 3's elision is what turns points 1 and 2 into a guarantee on the actual output: an elided
  sort is a no-op concatenation, so the already-`(insert_time, block_id)`-sorted files stay in that
  order end to end. **This is a correctness dependency, not just a memory optimization.** If
  elision failed instead (e.g. missing/absent min-max stats on some file), the fallback is a real
  `SortExec` ordering by `insert_time` alone — and a plain (non-stable-guaranteed) sort on one
  column does not preserve the `block_id` tie-break order from point 1, so the merged output could
  come out **not** sorted by `block_id` within an `insert_time` value, silently breaking the
  ordering guarantee this design exists to provide — while also reintroducing the full buffering
  `SortExec` this plan is meant to avoid. The offline test below asserting sort elision is
  therefore load-bearing for both the ordering guarantee's correctness and its memory bound, not a
  performance-only check.

### 2. Bounded-memory Postgres-source writes

Rewrite `MetadataPartitionSpec::write` to stream rows from Postgres in bounded chunks instead of
`fetch_all`-ing the whole range, following the same "spawn the writer, stream `PartitionRowSet`s
into its channel" shape already used by `create_merged_partition`, `SqlPartitionSpec::write`, and
`BlockPartitionSpec::write`:

```rust
const SOURCE_ROWS_PER_BATCH: usize = 20_000; // bounds peak memory to one chunk, not one day

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
            let mut chunk = Vec::with_capacity(SOURCE_ROWS_PER_BATCH);
            while let Some(row) = rows.try_next().await? {
                chunk.push(row);
                if chunk.len() >= SOURCE_ROWS_PER_BATCH {
                    flush_chunk(&mut chunk, &ctx, &self.compute_time_bounds, &tx).await?;
                }
            }
            if !chunk.is_empty() {
                flush_chunk(&mut chunk, &ctx, &self.compute_time_bounds, &tx).await?;
            }
        }
        Ok(())
    }.await;

    drop(tx);
    match stream_result {
        Ok(()) => { join_handle.await??; Ok(()) }
        Err(e) => { /* abort the writer task, mirroring create_merged_partition's error path */ }
    }
}
```

`flush_chunk` converts the accumulated `Vec<PgRow>` via the existing `rows_to_record_batch`
(unchanged — it already operates on a row slice, so it works identically on a partial chunk),
computes the chunk's event-time bounds via `compute_time_bounds.get_time_bounds(...)` (same call
already made once for the whole range today), sends a `PartitionRowSet`, and clears the `Vec` via
`mem::take`/`clear()` for reuse.

`source_data_hash` switches from `rows.len()` (recomputed from the fully-fetched `Vec<PgRow>`) to
`self.get_source_data_hash()` — the `record_count` already fetched by the earlier `COUNT(*)`
query in `fetch_metadata_partition_spec`. This is required because
`write_partition_from_rows` needs the hash *before* streaming starts (it's a spawn-time
parameter, not something derivable after the fact), and it's a simplification: `write()` no
longer computes a second, independently-arrived-at row count that could disagree with
`get_source_data_hash()`'s.

Peak memory becomes one `SOURCE_ROWS_PER_BATCH`-sized `Vec<PgRow>` plus one `RecordBatch` plus the
one in-flight channel item — bounded regardless of how many blocks exist in the requested
insert range.

### 3. Forced online regeneration

Add a `force: bool` parameter to `batch_update::materialize_partition` (`batch_update.rs:103`,
private, with only one caller: the loop inside `materialize_partition_range`).
`materialize_partition_range` itself (`batch_update.rs:195`) is `pub` with 8 existing callers —
the production maintenance daemon (`rust/public/src/servers/maintenance.rs:55`) plus 7 test call
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

This guard fails the call loudly (an `Err` surfaced through `regenerate_partitions`'s
`TaskLogExecPlan`) instead of the alternative of silently leaving both the stale and the new
partition in place.

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
`materialize_partitions`), copying `MaterializePartitionsTableFunction`'s shape
(`materialize_partitions_table_function.rs`) exactly, down to argument parsing
(`view_set_name`, `begin`, `end`, `delta_seconds`) and the `LogStreamTableProvider` /
`TaskLogExecPlan` spawn pattern — the only difference is its `_impl` function calls the new
`regenerate_partition_range(...)` (force: true baked in) instead of `materialize_partition_range`.

Usage for this issue: `SELECT * FROM regenerate_partitions('blocks', <day_begin>, <day_end>,
86400);` for each active merged daily blocks partition — `<day_begin>`/`<day_end>`/`86400` must
exactly match that partition's existing boundaries (see the alignment invariant above); a range or
delta that doesn't will now fail loudly instead of creating a duplicate partition.

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
   `QueryMerger`; use `PartitionedTableProvider::with_ordering` in `execute_merge_query`.
6. `rust/analytics/src/lakehouse/blocks_view.rs`: store a pre-built `QueryMerger` (ordering =
   `[insert_time, block_id]`, query = `"SELECT * FROM source ORDER BY insert_time, block_id;"`);
   override `merge_partitions` to delegate to it (mirror
   `SqlBatchView::merge_partitions`).
7. `rust/analytics/src/lakehouse/metadata_partition_spec.rs`: rewrite `write()` to stream via
   `sqlx::query(...).fetch(...)` in `SOURCE_ROWS_PER_BATCH`-sized chunks; add the `flush_chunk`
   helper; switch `source_data_hash` to `self.get_source_data_hash()`.
8. `rust/analytics/src/lakehouse/batch_update.rs`: add `force: bool` to (private)
   `materialize_partition`; factor `materialize_partition_range`'s loop body into a new private
   `materialize_partition_range_impl(..., force: bool)`. `materialize_partition_range` keeps its
   existing signature, delegating to `materialize_partition_range_impl(..., force: false)`, so its
   8 existing callers (`rust/public/src/servers/maintenance.rs` plus 7 test call sites in
   `histo_view_test.rs`, `sql_view_test.rs`, `thread_spans_ordering_db_test.rs`) need no changes.
   Add a new `regenerate_partition_range(...)` (same signature) that calls
   `materialize_partition_range_impl(..., force: true)`. When `force`, `materialize_partition`
   skips only `verify_overlapping_partitions`'s source-hash freshness check, and skips the
   `get_max_partition_time_delta` subdivision check, so the whole requested range is written as
   one `CreateFromSource` partition (letting `retire_partitions` cleanly retire the existing
   partition it replaces). Add a new `verify_force_regeneration_alignment` function, called only
   on the `force` path in place of `verify_overlapping_partitions`, that re-checks the same
   partial-overlap condition (`begin < insert_range.begin || end > insert_range.end`) against
   existing partitions and returns a loud `Err` on a misaligned `insert_range`/delta instead of
   silently leaving a duplicate partition behind.
9. New `rust/analytics/src/lakehouse/regenerate_partitions_table_function.rs`: copy
   `MaterializePartitionsTableFunction`'s shape, call `regenerate_partition_range(...)`. Declare
   the new module in `rust/analytics/src/lakehouse/mod.rs` (`pub mod
   regenerate_partitions_table_function;`, alongside the existing `pub mod
   materialize_partitions_table_function;`) — required for it to compile into the crate.
10. `rust/analytics/src/lakehouse/query.rs`: register `regenerate_partitions` UDTF next to
    `materialize_partitions`.
11. `python/micromegas/micromegas/flightsql/client.py`: add a `regenerate_partitions(...)` method
    mirroring `materialize_partitions(...)` (same argument shape), issuing
    `SELECT * FROM regenerate_partitions(...)` instead.
12. Documentation: add `regenerate_partitions` alongside `materialize_partitions` in
    `mkdocs/docs/query-guide/functions-reference.md`, `mkdocs/docs/admin/maintenance.md`, and
    `mkdocs/docs/query-guide/python-api.md` (documenting the `client.regenerate_partitions(...)`
    method added in step 11).
13. Tests (see Testing Strategy).
14. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.
15. Manual verification (see Testing Strategy) against a running environment with an existing
    unsorted merged blocks partition.

## Files to Modify

- `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` — `OrderingBounds`, generalized
  bounds helper.
- `rust/analytics/tests/thread_spans_ordering_tests.rs` — update its 3
  `make_partitioned_execution_plan` call sites to pass `OrderingBounds::EventTime`.
- `rust/analytics/src/lakehouse/materialized_view.rs` — pass `OrderingBounds::EventTime`.
- `rust/analytics/src/lakehouse/partitioned_table_provider.rs` — ordering-aware constructor.
- `rust/analytics/src/lakehouse/merge.rs` — `QueryMerger` ordering builder method.
- `rust/analytics/src/lakehouse/blocks_view.rs` — ordered merge override.
- `rust/analytics/src/lakehouse/metadata_partition_spec.rs` — streaming `write()`.
- `rust/analytics/src/lakehouse/batch_update.rs` — `force` parameter, new
  `materialize_partition_range_impl` + `regenerate_partition_range` (existing
  `materialize_partition_range` signature unchanged), new
  `verify_force_regeneration_alignment` guard for the `force` path.
- `rust/analytics/src/lakehouse/regenerate_partitions_table_function.rs` — new.
- `rust/analytics/src/lakehouse/mod.rs` — declare the new module (`pub mod
  regenerate_partitions_table_function;`).
- `rust/analytics/src/lakehouse/query.rs` — register new UDTF.
- `python/micromegas/micromegas/flightsql/client.py` — add `regenerate_partitions(...)` method.
- `mkdocs/docs/query-guide/functions-reference.md` — document `regenerate_partitions`.
- `mkdocs/docs/admin/maintenance.md` — document `regenerate_partitions`.
- `mkdocs/docs/query-guide/python-api.md` — document `regenerate_partitions`.

## Trade-offs

- **`ORDER BY` + declared scan ordering vs. `ORDER BY` alone.** An `ORDER BY` with no declared
  source ordering would still produce correct output but pay a full buffering `SortExec` on
  every merge — the exact problem `tasks/jit_single_query_plan.md` was written to avoid on the
  query side. Declaring the `insert_time` ordering lets DataFusion elide the sort node entirely
  instead (the merge source scan is a single sequential file group, so there is no second stream
  requiring a `SortPreservingMergeExec`), so the merge itself gains the same memory bound this
  plan is adding to the source write path — see Design §1 for why this elision is also what makes
  the `(insert_time, block_id)` ordering guarantee correct, not just cheap.
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
- **Chunked `sqlx` row streaming vs. a Postgres `DECLARE CURSOR` / `COPY`-based approach.**
  `sqlx::query(...).fetch(...)` already streams rows off the wire without server-side cursor
  management; a fixed row-count chunk (not byte-size-based, unlike the Parquet writer's 100MB
  flush threshold) is simpler and consistent with existing per-count batching elsewhere (e.g.
  `JitPartitionConfig::max_nb_objects`). `SOURCE_ROWS_PER_BATCH = 20_000` is a first cut, not
  load-tested against blocks-view's wide joined columns (JSONB tags/properties) — worth a sanity
  check under a real workload before tuning, same caveat the JIT plan raised for its own batch
  width.
- **Not declaring the JIT-consumer-side `(insert_time, block_id)` ordering in this plan.** Doing
  so before every active merged partition is regenerated would silently mis-group blocks for any
  partition still written under the old, unordered merge — exactly the failure mode
  `sort_and_check_non_overlapping` is designed to catch loudly for *new* overlaps, but it cannot
  detect "sorted-looking file that just happens to be wrong inside its own bounds." Declaring
  trust is a rollout step gated on regenerating and verifying every affected partition, not a
  code change bundled with this plan.

## Testing Strategy

- **Offline ordering tests** (new `rust/analytics/tests/blocks_view_merge_ordering_tests.rs`,
  following the existing no-DB pattern in `thread_spans_ordering_tests.rs`): build synthetic
  insert-time-disjoint `Partition`s, confirm `make_partitioned_execution_plan` under
  `OrderingBounds::InsertTime` with a single-column `insert_time` declared ordering elides the
  `Sort` node entirely (no `SortExec`, no `SortPreservingMergeExec` — one sequential file group
  already satisfies the ordering), and confirm an insert-time overlap is rejected loudly (negative
  control, mirroring `overlapping_partitions_are_rejected`). This elision assertion is a
  correctness check, not a performance one: per Design §1, the merged output is only guaranteed
  fully `(insert_time, block_id)`-sorted when the sort is elided (a no-op concatenation of
  already-sorted, disjoint files); if elision ever regressed to a real `insert_time`-only
  `SortExec`, the `block_id` tie-break order would silently stop being guaranteed, alongside the
  loss of the memory bound.
- **`MetadataPartitionSpec` streaming unit tests**: exercise `write()` (or the extracted
  `flush_chunk` helper) against a small `Vec<PgRow>`-free scenario if feasible, or an integration
  test against a live Postgres fixture, asserting: chunk boundaries don't drop/duplicate rows,
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
  leaving a duplicate partition.
- **Manual regeneration + memory check**: start services
  (`python3 local_test_env/ai_scripts/start_services.py` or monolith), find a real merged blocks
  partition, run `SELECT * FROM regenerate_partitions('blocks', <begin>, <end>, <delta>);`, and
  confirm: the partition's `updated`/`file_path` change, its row content is unchanged (same
  source-hash-derived count), and process RSS (system_monitor gauges from #1330) stays flat
  during regeneration of a busy day instead of spiking with the range width — the core check for
  the OOM concern this plan addresses.
- **Sortedness verification query** (documented, not new code) — run against a regenerated
  partition to confirm ordering. `lag(...) OVER ()` has no `ORDER BY`, so its result depends on
  physical row arrival order; this must be pinned to a single, non-repartitioned scan (session
  defaults otherwise split the parquet scan into byte-range partitions and interleave them via
  `CoalescePartitionsExec`, producing false failures against a perfectly sorted partition):
  ```sql
  SET datafusion.execution.target_partitions = 1;
  SET datafusion.optimizer.repartition_file_scans = false;

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

## Open Questions

- Which merged blocks partitions are "active" today and need `regenerate_partitions` run against
  them? This plan adds the tool; running it over the existing fleet is an operational rollout
  step, tracked separately (not blocking landing the code).
- Is `SOURCE_ROWS_PER_BATCH = 20_000` the right default? No load testing yet against blocks-view's
  widest real-world `streams.properties`/`processes.properties` payloads — worth revisiting once
  this lands.
- Declaring `(insert_time, block_id)` as a trusted `get_scan_output_ordering()` for
  blocks-view/JIT consumers is intentionally deferred to a follow-up plan (per
  `tasks/jit_single_query_plan.md`'s Open Questions), gated on the regeneration rollout above.
