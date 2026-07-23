# Blocks-View Ordered Merges + Bounded-Memory Regeneration Plan

## Overview

[#1336](https://github.com/madesroches/micromegas/issues/1336): make blocks-view partition
merges order-preserving so consumers (starting with JIT partition generation) can eventually
trust a declared `insert_time` scan ordering and drop a redundant `SortExec`. This
plan also closes a related OOM hazard: the same Postgres-backed materialization path that would
regenerate a merged partition currently loads the *entire* insert-time range's block rows into
one `Vec<PgRow>` and one `RecordBatch` before writing anything — for a busy day that is an
unbounded amount of memory. Regeneration must never buffer more than one bounded chunk of blocks
at a time.

Four coupled changes:
1. Make blocks-view merges order-preserving (declared scan ordering + explicit `ORDER BY`), so
   merged partitions stay internally sorted by `insert_time` going forward.
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

Declaring a *trusted* `insert_time` scan ordering for blocks-view consumers (the JIT partition
generators, per `tasks/jit_single_query_plan.md`) — so they could drop their own `SortExec` — is
explicitly **out of scope** here: it only becomes safe after every active merged partition has been
regenerated under (1), which is an operational rollout step, not a code change. Note the JIT
consumer does **not** need a trusted `(insert_time, block_id)` *total* order: it owns its bucketing
determinism via tie-atomic, soft-cap segmentation (a pure function of `insert_time`; see
`tasks/jit_single_query_plan.md`), so single-column `insert_time` is all this plan ever records or
declares. See Trade-offs.

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

  Only `insert_time` is declared, and the query's `ORDER BY` matches it exactly:
  `attach_ordering_statistics` can only attach min/max stats for the leading declared column, and
  `Partition` metadata has no `block_id` bounds to attach — a two-column declared ordering would
  never validate (DataFusion 54 requires present min/max stats for *every* declared sort column)
  and DataFusion would fall back to a full buffering `SortExec`. Single-column `insert_time` is
  exactly the guarantee this plan records and the ordering it declares — the merge does not need
  and does not promise any `block_id` tie-break (see Trade-offs for why the consumer no longer
  requires one). Source partitions are non-overlapping in insert_time by construction and each is
  already internally `insert_time`-sorted, so once the source scan's single sequential file group
  (see below) has a validated `insert_time` ordering, its file-by-file concatenation is already
  exactly the `insert_time` order the merge needs.

  `create_merged_partition` already calls `filtered_partitions.sort_by_key(|p|
  p.begin_insert_time())` before invoking `view.merge_partitions` (`merge.rs:171`), and
  time-sliced partitions are non-overlapping in insert_time by construction, so
  `sort_and_check_non_overlapping` under `OrderingBounds::InsertTime` should always pass for
  well-formed input — a failure here would indicate a genuine partitioning bug, matching the
  existing "fail loudly" philosophy for the event-time case.

  Why the merged output ends up internally `insert_time`-sorted — **conditionally**, gated on the
  inputs actually being trustworthy:
  1. Each input file is internally `insert_time`-sorted *if and only if* its own recorded
     `sort_order` (Design §4) already equals `['insert_time']`. The gate is on the *recorded*
     value, not on file stats, and that distinction is load-bearing: DataFusion validates a
     declared ordering against each file's min/max stats only, **not** against its internal row
     order (see point 3), so a file whose `[begin, end)` range is disjoint from its siblings but
     whose rows are not actually `insert_time`-sorted internally would still pass validation and be
     concatenated verbatim — silently producing an out-of-order merged file. Recorded
     `sort_order == ['insert_time']` is the only thing that certifies internal sortedness. It is
     true for partitions written fresh via `data_sql`'s `ORDER BY blocks.insert_time, blocks.block_id`
     (`blocks_view.rs:46` — a superset of `insert_time`) and for partitions produced by a prior run
     of this same ordered merge — but it is **not** true for a partition merged before this change
     shipped: the maintenance daemon (`rust/public/src/servers/maintenance.rs:68-174`) creates
     1-second `CreateFromSource` blocks partitions, then rolls them up through minutely, hourly, and
     daily merges, and every pre-fix merge (at any granularity) ran `View::merge_partitions`'s
     unordered `SELECT * FROM source;` default (`view.rs:101-124`) with no declared scan ordering —
     its output is not internally sorted, and (before Design §4 exists) it has no `sort_order` to
     say so. `BlocksView::merge_partitions` therefore only takes the ordered path below — declaring
     the `[insert_time]` scan ordering and later recording `['insert_time']` — when *every*
     partition in `partitions_to_merge` is either empty (`is_empty()`, `num_rows == 0` — an empty
     partition contributes no rows and no file to the scan, `make_partitioned_execution_plan` drops
     it before building the file group, so it vacuously satisfies any ordering whatever its recorded
     `sort_order`) or already has `sort_order == Some(['insert_time'])`, **and** at least one input
     is non-empty; if even one non-empty input's `sort_order` is not that exact value (every pre-fix
     partition, merged or not), the merge instead runs today's plain unordered query and records
     `NULL` itself, rather than trusting a per-file order it cannot verify (see Design §4 and Rollout
     for the resulting rollout property). The all-empty case — a real production shape, since ≥2 empty
     partitions still satisfy `create_merged_partition`'s "≥2 partitions" merge trigger (a quiet
     day) — must take the plain merger for a different reason: an all-empty source scans as an
     `EmptyExec` (`partitioned_execution_plan.rs:152-161`), which declares no output ordering, and
     no DataFusion 54 rule removes a `SortExec` above an empty source (there is no physical
     empty-propagation rule; logical `PropagateEmptyRelation` fires only on
     `LogicalPlan::EmptyRelation`, not on a table scan that turns out empty at physical planning;
     and `EnforceSorting` elides a sort only when its child already satisfies the ordering, which
     `EmptyExec` never does) — so on the ordered path the plan-shape check below would see a
     never-elided `SortExec` and emit a spurious memory-regression warning on every daemon retry,
     forever (a quiet day is not a memory problem). Routed to the plain merger, an all-empty merge still
     records the guarantee per Design §4: it is vacuously true of the empty output. Points 2 and 3
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

  Point 3's elision is what delivers the **memory bound**: an elided sort is a no-op
  concatenation, so the merge streams the already-`insert_time`-sorted files through without
  buffering. Crucially, this is *not* an ordering-correctness dependency. The merge query keeps its
  explicit `ORDER BY insert_time`, so the output rows come out `insert_time`-sorted whether or not
  the `SortExec` is elided — if elision fails, DataFusion runs a real `SortExec` that sorts by
  `insert_time` and produces the same ordered result, only at the cost of buffering. So there is
  exactly **one correctness guard**, plus a separate memory-health check:
  - **Correctness — the input-`sort_order` gate (point 1).** This is what makes the *elided* path
    correct: elision concatenates files in validated insert-time order without re-sorting, trusting
    each file to be internally `insert_time`-sorted, and that trust is certified only by recorded
    `sort_order == ['insert_time']`, because DataFusion validates file min/max stats, not internal
    row order (point 3). If the gate were wrong or skipped, elision would faithfully concatenate an
    internally-unsorted file in validated range order and produce an out-of-order merged file. (On
    the fallback `SortExec` path this gate is not needed for correctness — the sort re-orders
    regardless — but that path is the one this plan exists to avoid.)
  - **Memory health — the plan-shape check.** Because DataFusion's `validate_orderings` silently
    drops an unprovable declared ordering rather than erroring, an elision failure produces no error
    on its own and would quietly reintroduce the full-buffering `SortExec` this plan exists to
    avoid. So the ordered path (only — never the plain merger; Design §4, scoping rationale in
    Trade-offs) builds the optimized physical plan once and inspects it before executing. Two
    outcomes:
    - **Not a single output partition.** A hard `bail!` before executing anything — but for a
      *mechanical* reason, not an ordering one: `datafusion::physical_plan::execute_stream` itself
      requires a single-partition plan, so this shape cannot run at all. It also indicates
      `repartition_file_scans = false` (point 3) did not take effect.
    - **Single-partition, but still containing a `SortExec` and/or a `SortPreservingMergeExec`
      node.** Either operator means elision did not fully happen (a bare `SortExec` means the
      declared ordering never validated; a `SortPreservingMergeExec` with no `SortExec` means
      `repartition_file_scans` didn't stop a single-file group from being byte-range-split, per
      point 3 — the exact failure mode a check that only looked for `SortExec` would miss, since the
      string `"SortPreservingMergeExec"` does not contain the substring `"SortExec"`). This does
      **not** fail the merge and does **not** change the recorded `sort_order`: the plan as built,
      `Sort` node and all, still computes a correct `insert_time`-ordered result. The merge logs a
      loud warning identifying the query and `insert_range`, executes that same plan, and reports
      via its return value that the memory bound was not honored for this one merge — so the
      maintenance daemon's `blocks` materialization keeps making progress instead of stalling on
      every retry until a human patches a DataFusion upgrade or config regression. See Trade-offs
      for why failing open on memory here is right, in contrast to the not-single-partition case
      above and to `sort_and_check_non_overlapping`'s insert-time overlap check, both of which stay
      hard errors.

    Concretely, `PartitionMerger::execute_merge_query` and `View::merge_partitions` both return a
    small `MergeQueryResult { stream, ordering_honored: bool }` instead of a bare stream (see
    Implementation Steps 5/18). `ordering_honored` is trivially `true` whenever no ordering was
    declared to DataFusion in the first place (the plain merger, `BatchPartitionMerger`, every
    non-`BlocksView` caller of `View::merge_partitions`), and is only ever computed dynamically, from
    the plan-shape check above, by the ordered `QueryMerger`. It now drives only the warning and the
    memory-health reporting — **not** the recorded `sort_order`.

  So a false `sort_order` guarantee is prevented by the single input-`sort_order` gate alone: it
  stops the elided merge from trusting inputs it can't verify. The recorded `sort_order` is then a
  truthful function of *which path ran* — `['insert_time']` on the ordered path, `NULL` on the plain
  path — recorded unconditionally on the ordered path, independent of whether elision succeeded at
  the physical level (an elision miss costs memory, not correctness; see Design §4's value
  semantics).

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

(The sketch shows only the streaming change: the `write_partition_from_rows` call also takes the
`sort_order` argument from Design §4 — see Implementation Steps 15–18. `write()`'s signature is
otherwise unchanged; Design §3's concurrency guard does not touch it — see Design §3.)

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

Add a new public `regenerate_partition_range` (`batch_update.rs`) as a sibling of
`materialize_partition_range`, not a flag on it: `materialize_partition_range` and the private
`materialize_partition` it calls are untouched by this part — no `force` parameter was added to
either — so their 9 existing external callers (the production maintenance daemon
(`rust/public/src/servers/maintenance.rs`), the `materialize_partitions` UDF
(`materialize_partitions_table_function.rs`), and 7 test call sites) needed zero changes.
`regenerate_partition_range` validates that `delta` exactly tiles `(begin, end)` (below), then
loops per-bucket calling a new private `regenerate_partition`. Unlike `materialize_partition`,
`regenerate_partition` never merges, never aborts on freshness, and never subdivides — each bucket
the tiling loop produces is already exactly one partition's worth: it calls
`verify_force_regeneration_alignment` (below) against that bucket, then
`view.make_batch_partition_spec(...)` and `partition_spec.write(lake, logger)` directly, always
writing a fresh `CreateFromSource` partition. This is a standalone function rather than a
`force: bool` threaded through `materialize_partition`'s strategy/subdivision branching (an earlier
revision of this plan) — regeneration's requirements (always-fresh, always-whole-bucket,
never-subdivide) are exactly what that branching exists to *avoid* doing unconditionally, so a
short standalone function duplicates less than parameterizing every branch of `materialize_partition`
to skip itself.

**Invariant `regenerate_partitions` callers must uphold:** the requested `(begin, end, delta)`
must exactly cover the boundaries of the partition(s) being regenerated.
`retire_partitions`'s cleanup only deletes partitions fully contained within the *new*
partition's own range (`begin_insert_time >= $3 AND end_insert_time <= $4`,
`write_partition.rs:226-246`); a misaligned range/delta (e.g. regenerating a daily partition with
`delta=3600`, or a sub-range that starts or ends mid-partition) means the new partition's range
doesn't fully contain the old one, so the old partition is never retired — it survives untouched
while the new one is inserted alongside it, producing silent duplicate rows.

This invariant is enforced, not just documented: `regenerate_partition_range` validates, before
entering its own per-bucket loop, that `delta` exactly tiles `(begin, end)` —
`(end - begin)` must be an exact, non-zero multiple of `delta`. `chrono::TimeDelta` implements no
`Rem`/`%` operator, so the check is integer arithmetic on nanoseconds: with
`span = (end - begin).num_nanoseconds()` and `step = delta.num_nanoseconds()` (both
`Option<i64>`-unwrapped with `expect`, well within range for any real time range), require
`step > 0 && span >= step && span % step == 0` — and returns a loud `Err` otherwise. This is a distinct failure
mode from the one `verify_force_regeneration_alignment` (below) catches: the loop
runs `while end_part <= insert_range.end`, so a
`delta` that doesn't tile the range makes it execute a partial or zero number of iterations and
return `Ok` without ever reaching `regenerate_partition` (and hence
`verify_force_regeneration_alignment`) for the untiled remainder — a silent partial or total no-op
that the alignment guard, which only runs per-bucket inside the loop, would never see.

`verify_force_regeneration_alignment` exists because `regenerate_partition` never calls
`verify_overlapping_partitions` at all — it goes straight to `make_batch_partition_spec` +
`write(...)` — so it needs its own guard against `verify_overlapping_partitions`'s *other* job:
rejecting a partial overlap (`begin < insert_range.begin || end > insert_range.end` → would
otherwise `Abort` in the `materialize_partition` path).
`verify_force_regeneration_alignment` runs that same partial-overlap test with no source-hash
comparison — there is no freshness check to reintroduce here, since `regenerate_partition` never
had one — producing only a loud `Err` on a misaligned request. Unlike `verify_overlapping_partitions`,
it must filter the
existing-partitions snapshot *hash-agnostically*: `PartitionCache::filter` also requires
`file_schema_hash` equality (`partition_cache.rs:220`), which would make the guard blind to a
partially-overlapping partition written under an older schema hash — a row that
`retire_partitions`'s equally hash-agnostic range-containment delete (`write_partition.rs:226-246`)
would not retire either, leaving exactly the silent duplicate this guard exists to prevent. It
therefore uses the hash-free `filter_insert_range` (`partition_cache.rs:234-247`) and matches
view name/instance itself:

```rust
fn verify_force_regeneration_alignment(
    existing_partitions_all_views: &PartitionCache,
    insert_range: TimeRange,
    view_set_name: &str,
    view_instance_id: &str,
) -> Result<()> {
    // hash-agnostic on purpose: any same-view partition overlapping the range
    // matters here, whatever schema hash it was written under
    let filtered = existing_partitions_all_views.filter_insert_range(insert_range);
    for part in &filtered.partitions {
        if *part.view_metadata.view_set_name != view_set_name
            || *part.view_metadata.view_instance_id != view_instance_id
        {
            continue;
        }
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

async fn regenerate_partition(
    existing_partitions_all_views: Arc<PartitionCache>,
    lakehouse: Arc<LakehouseContext>,
    insert_range: TimeRange,
    view: Arc<dyn View>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let view_set_name = view.get_view_set_name();
    let view_instance_id = view.get_view_instance_id();
    verify_force_regeneration_alignment(
        &existing_partitions_all_views,
        insert_range,
        &view_set_name,
        &view_instance_id,
    )?;
    let partition_spec = view
        .make_batch_partition_spec(lakehouse.clone(), existing_partitions_all_views, insert_range)
        .await?;
    partition_spec.write(lakehouse.lake().clone(), logger).await
}
```

No `PartitionCreationStrategy`/subdivision branching runs here at all — `regenerate_partition`
never consults `get_max_partition_time_delta`, so there is no subdivision decision to skip. Each
call already writes exactly one whole bucket as `CreateFromSource`, because `regenerate_partition_range`'s
tiling loop guarantees that invariant before this function is ever called.

This guard is meant to fail the call loudly instead of silently leaving both the stale and the new
partition in place — but that requires `regenerate_partitions` to surface the `Err` as a
query-level failure, not as a log row `TaskLogExecPlan` streams back as if the query succeeded (see
below for why the existing `materialize_partitions`/`TaskLogExecPlan` pattern cannot do this, and
the mechanism `regenerate_partitions` uses instead).

Never subdividing is required, not optional: `BlocksView::get_max_partition_time_delta`
returns `TimeDelta::hours(1)` for `CreateFromSource` but `TimeDelta::days(1)` for
`MergeExisting` (`blocks_view.rs:140-147`). If `regenerate_partition` honored that 1-hour
`CreateFromSource` delta for a day-range bucket, it would need to write 24 hourly partitions
instead of one daily partition. `retire_partitions`'s range-containment delete only retires
partitions fully contained within *each new* partition's own range, so none of those 24 hourly
`[h, h+1h)` ranges would contain the pre-existing daily `[day_begin, day_end)` partition — it
would never be retired, leaving duplicate rows until a manual `retire_partitions` call. Writing
the whole bucket as one `CreateFromSource` partition makes the new partition's range exactly
`[day_begin, day_end)`, so `retire_partitions` cleanly deletes the old daily partition in the
same transaction as the insert.

Via `partition_spec.write()`, this still uses the now-streaming Postgres fetch from part 2 (so a
whole-day `CreateFromSource` write stays memory-bounded even without subdivision) and the
retire-then-insert atomic swap in `insert_partition` (part of the "Current State" analysis
above), so regeneration is online (no downtime) and memory-bounded from day one.

**Guarding against a concurrent write into an overlapping range.** `verify_force_regeneration_alignment`
only checks the `PartitionCache` snapshot fetched at the start of the call — it cannot see a
partition another writer (e.g. the maintenance daemon) commits *after* that snapshot but *before*
this write commits. This matters specifically when `regenerate_partitions` targets a range another
writer is still actively materializing at a different granularity (e.g. the current or a very
recent day, whose hours/minutes the daemon has not yet rolled up into a merged daily; already-merged
past dailies are not exposed to this race). `insert_partition`'s advisory lock (`write_partition.rs`)
is keyed on the exact `(view_set_name, view_instance_id, begin_insert_time, end_insert_time)`
tuple, so a regen write and a concurrent daemon write into an overlapping-but-different range take
*different* locks and can commit in either order, with the overlap running in either direction of
containment (e.g. a regen daily `[d, d+1)` racing a concurrent daemon hourly `[h, h+1)` contained
inside it, or the reverse — a regen hourly racing a daemon daily merge that spans it). Either shape
would otherwise leave both rows in place forever — `retire_partitions`'s cleanup only deletes
partitions fully *contained by* the newly-inserted partition's own range, which only catches one of
the two directions — and queries would double-count the overlapping blocks.

Rather than closing this with a `force`-gated in-transaction `SELECT` recheck scoped to the write's
own range (an earlier revision of this plan, which read-then-checked under Postgres's `READ
COMMITTED` isolation and so only shrank the race window rather than closing it, and only for
regeneration's own writes), the shipped design enforces disjointness unconditionally, at the
database layer, for every writer — regeneration, ordinary materialization, and merges alike. The
v6→v7 migration (`migration.rs`'s `upgrade_v6_to_v7`) adds
`CREATE EXTENSION IF NOT EXISTS btree_gist;` — needed because `EXCLUDE USING gist` over the
constraint's equality columns requires the gist equality operator class `btree_gist` provides; on
PostgreSQL ≤ 12, or if the migrating role lacks `CREATE` on the database, a superuser must run this
once out of band — and then:

```sql
ALTER TABLE lakehouse_partitions ADD CONSTRAINT lakehouse_partitions_no_overlap
EXCLUDE USING gist (
    view_set_name WITH =,
    view_instance_id WITH =,
    file_schema_hash WITH =,
    tstzrange(begin_insert_time, end_insert_time) WITH &&
);
```

Scoped by `file_schema_hash` (`btree_gist` supports `bytea` equality) because a schema-hash bump
legally leaves old- and new-schema partitions coexisting with overlapping ranges until
`retire_incompatible_partitions` cleans up the old ones — queries already filter by schema hash, so
only a *same-schema* overlap is ever a real bug. `tstzrange` is `'[)'`, so adjacent partitions
sharing a boundary do not conflict, and the write path's retire-then-insert runs inside one
transaction, so replacing a partition with itself never self-conflicts. A Postgres exclusion
constraint can't be added `NOT VALID`, so the migration first runs a detect-then-fail query for any
pre-existing same-view/instance/schema-hash overlapping pair (excluding zero-width ranges, which
never overlap under `tstzrange` semantics) and `bail!`s naming the first 20 conflicting file pairs
— instructing the operator to retire them (e.g. via `retire_partition_by_metadata`) and restart —
rather than failing later with a raw constraint-violation error the migration can't attribute to
specific rows.

Unlike a `SELECT`-based recheck under `READ COMMITTED` (which can only see rows already committed
at the moment it runs, so two concurrent transactions whose snapshots don't see each other's
uncommitted rows can both pass it), the exclusion constraint is enforced by the index itself even
across concurrent transactions: the second conflicting insert blocks on the first, then fails once
the first commits. `insert_partition_transaction` (`write_partition.rs`) catches the resulting
constraint violation via `db_err.constraint() == Some("lakehouse_partitions_no_overlap")` and
re-raises it as a legible domain error naming the new partition's view/instance/range and pointing
at `retire_partition_by_metadata` or a range/delta alignment fix, instead of surfacing Postgres's
raw SQLSTATE `23P01`. This closes the race completely and unconditionally — including between two
concurrent *forced* regenerations of overlapping-but-different ranges, which a range-scoped
in-transaction recheck could not have closed either — and needed no `force` parameter anywhere in
`PartitionSpec::write`/`write_partition_from_rows`/`insert_partition`: those three keep the exact
signatures they had before this plan.

This also closes a related gap for every insert failure, not only constraint violations: on any
`insert_partition` failure, `delete_if_orphan` (`write_partition.rs`) now checks
`lakehouse_partitions` for the just-written file's per-write UUID path and deletes it from object
storage only if no row references it, rather than leaving it orphaned — a failed commit may still
have been applied server-side, so the returned error alone can't tell us whether the file is
orphaned, and this checks the authoritative state instead of guessing.

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
iterating the Rollout section's discovery query would otherwise see success for a partition that was
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
  `Poll::Ready(Some(Err(DataFusionError::Execution(msg))))` and ends the stream (one
  `poll_recv_many` batch can contain `Ok` items ahead of the `Err`; the `Err` wins and those
  same-batch progress rows are dropped — acceptable, they are transient progress lines on a query
  that is failing anyway) — a
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
implied, e.g. `{insert_time}`.

Deployment note: the migration is graceful for already-running writers, not a flag-day. A
pre-upgrade binary's `insert_partition` builds a positional 13-value `INSERT`, and Postgres maps a
short `VALUES` list to the first N columns in declared order, filling the rest with defaults — so
against the migrated 14-column table that `INSERT` succeeds and leaves the trailing `sort_order`
`NULL`, which is exactly the correct "no guarantee" value for a partition written by pre-fix code.
A v6 binary that is already running can therefore keep writing safely while the migration lands.
The hard edge is at (re)start only: a v6 binary refuses to start against an already-migrated v7
database (the `assert_eq!(current_version, LATEST_LAKEHOUSE_SCHEMA_VERSION)` guards in
`migrate_lakehouse` and `execute_lakehouse_migration`, `migration.rs:48,97`), so every
partition-writing binary must be upgraded to the v7 build before its next restart.

**Value semantics** — the recorded guarantee is what the written file actually satisfies, not what
was declared to DataFusion for sort elision:
- Blocks partitions written fresh via `MetadataPartitionSpec::write` (part 2): `data_sql` ends
  `ORDER BY blocks.insert_time, blocks.block_id` (`blocks_view.rs:46`), so record `['insert_time']`
  — the guaranteed prefix. (The file is also `block_id`-ordered within each `insert_time`, but that
  is not recorded or relied on; only the `insert_time` prefix is a promise — see Trade-offs.)
- Blocks partitions written by the part-1 ordered merge: `['insert_time']` **only when every
  partition being merged already carries that exact `sort_order` or is empty** — `BlocksView`
  (Design §1) declares `[insert_time]` to DataFusion and takes the ordered path at all only when
  every entry in `partitions_to_merge` is empty or already has `sort_order == Some(['insert_time'])`,
  and at least one is non-empty (an all-empty merge runs the plain merger to avoid the never-elided
  `SortExec` over an `EmptyExec` — Design §1 — but still records the guarantee, vacuously true of
  its empty output); if even one non-empty input's `sort_order` is `NULL` (every pre-fix partition,
  including pre-fix *merged* partitions — see Design §1), the merge instead runs the plain unordered
  query with no declared ordering and records `NULL`, exactly like today. The recorded value is
  unconditional on the ordered path, and it is *not* gated on whether elision physically happened:
  the merge query keeps `ORDER BY insert_time`, so the output is `insert_time`-sorted whether the
  `SortExec` is elided or falls back to a real buffering sort (Design §1). Elision is a memory
  optimization here, not a correctness precondition for the recorded value. The runtime plan-shape
  check (`merge.rs`'s ordered path only — never the plain merger; Design §1/Trade-offs on scoping)
  therefore serves memory health, not ordering correctness: it builds the optimized physical plan
  before executing and checks whether it is a single-partition plan containing **neither** a
  `SortExec` **nor** a `SortPreservingMergeExec` node. A plan that isn't single-partition at all
  fails the merge loudly — but for the *mechanical* reason that `execute_stream` requires a
  single-partition plan, not an ordering one. A single-partition plan that still contains one of
  those nodes means elision did not fully happen, for whatever reason (config drift, a DataFusion
  upgrade, or `repartition_file_scans` not taking effect) — the merge does **not** fail: it executes
  the plan as built, logs a loud warning identifying the query and `insert_range`, and reports back
  (via `MergeQueryResult`'s `ordering_honored` field, Design §1/Implementation Steps) that the
  memory bound was not honored. `create_merged_partition` records `['insert_time']` for a merge
  whenever the input-`sort_order` gate holds (i.e. `get_merged_partition_sort_order` returns a
  value); `ordering_honored` drives only the warning, not the recorded value. A false guarantee can
  never be persisted because the gate refuses to trust inputs it can't verify — see Trade-offs for
  why failing open on the memory check, unlike the not-single-partition case, is the right call.
- Forced regeneration (part 3) goes through the same fresh-write path via `partition_spec.write()`,
  so it records `['insert_time']` automatically — no extra code needed at the regeneration call site.
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
    (`blocks_view.rs:67-99`) passes `Some(vec!["insert_time".to_string()])` — the guaranteed prefix
    of the ordering `data_sql`'s `ORDER BY` produces (`blocks_view.rs:46`, which also sorts by
    `block_id` within each `insert_time`, but that tie-break is not recorded or promised).
  - `View` (`view.rs:52-153`) gains a new method `fn get_merged_partition_sort_order(&self,
    _partitions_to_merge: &[Partition]) -> Option<Vec<String>> { None }` (default `None`, ignoring
    the argument). It takes the same partitions the merge is about to run over — the recorded value
    is a function of those specific inputs (Design §1), not a static per-view constant. This is a
    distinct concept from the existing `get_scan_output_ordering()` (`view.rs:150-152`): that one is
    a *trusted scan-ordering declaration for consumers*, deliberately left empty for blocks-view in
    this plan (see Design §1 and Trade-offs on JIT trust); `get_merged_partition_sort_order()` is a
    *record of what this specific merge actually produced*, independent of what's declared to
    DataFusion for elision. `create_merged_partition` (`merge.rs:132-232`) calls
    `view.get_merged_partition_sort_order(&filtered_partitions)` — a pure function of the input
    slice alone, so it can be (and is) called before `view.merge_partitions(...)` executes — and
    passes its result straight into the `write_partition_from_rows` call (`merge.rs:184-192`). The
    value is **not** gated on the `ordering_honored` flag `merge_partitions` returns: the output is
    `insert_time`-sorted whether or not elision happened (Design §1), so a gate would only
    under-record a true guarantee. `ordering_honored` is used solely to emit the memory-regression
    warning. `BlocksView` overrides the method to return `Some(vec!["insert_time".to_string()])`
    only when every partition in the given slice is empty or already has that exact `sort_order`,
    `None` otherwise — sharing the same predicate `BlocksView::merge_partitions` uses (its merger
    choice adds only the "at least one non-empty input" clause, which switches between two paths
    that both uphold the recorded value — see Design §1), so the two decisions can't diverge; every
    other view keeps the default `None`.
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

With this in place, which merged blocks partitions still need regeneration becomes a SQL query
rather than tribal knowledge (see Rollout), and the
deferred JIT-consumer-trust follow-up (`tasks/jit_single_query_plan.md`) has a concrete,
footer-free way to check per-partition sort status before it declares an
`insert_time` scan ordering trusted (see Trade-offs).

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
5. `rust/analytics/src/lakehouse/merge.rs`:
   - Change `PartitionMerger::execute_merge_query`'s return type from
     `Result<SendableRecordBatchStream>` to `Result<MergeQueryResult>`, a new
     `pub struct MergeQueryResult { pub stream: SendableRecordBatchStream, pub ordering_honored:
     bool }` (Design §1). `ordering_honored` is `true` whenever the merger declares no scan
     ordering at all — the unconditional case for every implementer/caller that exists before this
     step.
   - Add `with_merge_scan_ordering` builder method to `QueryMerger`; use
     `PartitionedTableProvider::with_ordering` in `execute_merge_query`. When the merger's declared
     scan ordering is non-empty, `execute_merge_query` must also: set
     `datafusion.optimizer.repartition_file_scans = false` on the session context (Design §1 point
     3; `SessionContext` has no direct setter — mutate through the state,
     `ctx.state_ref().write().config_mut().options_mut().optimizer.repartition_file_scans = false`,
     before `ctx.sql(...)` so physical planning sees it); and, instead of
     `DataFrame::execute_stream()`, build the optimized physical plan once
     (`df.create_physical_plan()` — takes `&self` in DataFusion 54), inspect it (e.g. via
     `displayable(plan.as_ref()).indent(true)`) — the build-once/inspect/execute pattern already
     used in `rust/public/src/servers/flight_sql_service_impl.rs:420-432`, so the query is planned
     and run exactly once — and:
     - `bail!` before executing anything if the plan is not a single output partition (Design §1);
     - otherwise, if it contains a `SortExec` or a `SortPreservingMergeExec` node anywhere, log a
       `warn!` identifying the query and `insert_range` and proceed with `ordering_honored: false`
       rather than erroring;
     - execute the plan via `datafusion::physical_plan::execute_stream(plan, task_ctx)` in every
       case that reaches this point (elided or not), and return
       `MergeQueryResult { stream, ordering_honored }`.
     Both the setting and this whole check are no-ops (`ordering_honored` always `true`, plain
     `execute_stream()` as today) when the declared ordering is empty (every existing
     `View::merge_partitions` caller, plus `sql_batch_view.rs`'s aggregation merger), matching the
     scoping rationale in Trade-offs.
   - `BatchPartitionMerger::execute_merge_query` (`batch_partition_merger.rs:104-...`),
     `View::merge_partitions`'s default impl (`view.rs:101-124`), and
     `SqlBatchView::merge_partitions` (`sql_batch_view.rs:250-269`) update their return types to
     `Result<MergeQueryResult>` and wrap their existing stream as
     `MergeQueryResult { stream, ordering_honored: true }` — mechanical, no behavior change, since
     none of them ever declares a merge scan ordering.
   - `create_merged_partition` destructures the `MergeQueryResult` returned by
     `view.merge_partitions(...)`, passes `view.get_merged_partition_sort_order(&filtered_partitions)`
     straight to `write_partition_from_rows` (not gated on `ordering_honored` — Design §1/§4), and
     uses the `ordering_honored` field only to drive the memory-regression warning — see step 18.
6. `rust/analytics/src/lakehouse/blocks_view.rs`: store two pre-built `QueryMerger`s — an ordered
   one (ordering = `[insert_time]`, i.e. the `Arc<String>`-wrapped `ScanSortColumn` from Design §1,
   query = `"SELECT * FROM source ORDER BY insert_time;"`) and the
   plain unordered one (empty ordering, query = `"SELECT * FROM source;"`, matching
   `View::merge_partitions`'s default); add a helper predicate over `partitions_to_merge` (every
   input is empty or already has `sort_order == Some(['insert_time'])`); override
   `merge_partitions` to delegate to the ordered merger when the predicate holds and at least one
   input is non-empty, otherwise to the plain merger — an all-empty source scans as an `EmptyExec`
   whose `SortExec` is never elided, so it must not take the ordered path (Design §1) — (mirror
   `SqlBatchView::merge_partitions`'s delegation pattern). Reuse the same predicate in step
   18's `get_merged_partition_sort_order` override so the two decisions can't diverge.
7. `rust/analytics/src/lakehouse/metadata_partition_spec.rs`: rewrite `write()` to stream via
   `sqlx::query(...).fetch(...)`, flushing whenever the pending chunk's estimated size reaches
   `SOURCE_BYTES_PER_BATCH` (8 MB); add the `flush_chunk` and `estimate_row_bytes` helpers;
   switch `source_data_hash` to `self.get_source_data_hash()`.
8. `rust/analytics/src/lakehouse/batch_update.rs`: `materialize_partition` and
   `materialize_partition_range` are untouched — no `force` parameter, no shared `_impl` split; their
   9 existing external callers (`rust/public/src/servers/maintenance.rs`, the
   `materialize_partitions` UDF in `materialize_partitions_table_function.rs`, plus 7 test call
   sites in `histo_view_test.rs`, `sql_view_test.rs`, `thread_spans_ordering_db_test.rs`) — and the
   internal recursive subdivision call — need no changes.
   Add a new public `regenerate_partition_range(...)` that first validates `delta` exactly tiles
   `(begin, end)` (Design §3's nanosecond `step > 0 && span >= step && span % step == 0` check, loud
   `Err` otherwise, before any partition is written) and then loops per-bucket calling a new private
   `regenerate_partition(...)`, which calls `verify_force_regeneration_alignment` (below), then
   `view.make_batch_partition_spec(...)` and `partition_spec.write(lakehouse.lake().clone(), logger)`
   directly — always writing one whole bucket as `CreateFromSource`, with no
   `PartitionCreationStrategy`/subdivision decision to make. Add a new
   `verify_force_regeneration_alignment` function that re-checks the same partial-overlap condition
   `verify_overlapping_partitions` guards (`begin < insert_range.begin || end > insert_range.end`)
   against existing partitions — filtered hash-agnostically via `filter_insert_range` + a view
   name/instance match, per Design §3 — and returns a loud `Err` on a misaligned `insert_range`/delta
   instead of silently leaving a duplicate partition behind. Design §3's concurrency guard against a
   concurrent write into an overlapping range is *not* implemented here (no `force` argument reaches
   `partition_spec.write(...)`) — it is instead a database-level `EXCLUDE` constraint added in step
   13's migration and handled in `insert_partition` (step 15).
9. Generalize the log-stream plumbing to carry an error (Design §3): change the channel item type
   to `Result<(DateTime<Utc>, String), String>` in `rust/analytics/src/dfext/task_log_exec_plan.rs`
   (the `TaskSpawner` alias), `rust/analytics/src/dfext/async_log_stream.rs` (`AsyncLogStream::rx`;
   `poll_next` batches `Ok` items into rows as today and turns an `Err(msg)` item into
   `Poll::Ready(Some(Err(DataFusionError::Execution(msg))))`, ending the stream and dropping any
   `Ok` items received in the same poll batch), and
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
    `upgrade_v5_to_v6`. In the same migration function (Design §3's concurrency guard): add
    `CREATE EXTENSION IF NOT EXISTS btree_gist;`; run a detect-then-fail `SELECT` for any
    pre-existing same-view/instance/schema-hash overlapping partition pair (excluding zero-width
    ranges) and `bail!` naming the first 20 conflicts if any are found; then
    `ALTER TABLE lakehouse_partitions ADD CONSTRAINT lakehouse_partitions_no_overlap EXCLUDE USING
    gist (view_set_name WITH =, view_instance_id WITH =, file_schema_hash WITH =,
    tstzrange(begin_insert_time, end_insert_time) WITH &&);`.
14. `rust/analytics/src/lakehouse/partition.rs`: add `pub sort_order: Option<Vec<String>>` to
    `Partition`.
15. `rust/analytics/src/lakehouse/write_partition.rs`: add a `sort_order: Option<Vec<String>>`
    parameter to `write_partition_from_rows`; thread it into the `Partition` literal and into
    `insert_partition`'s `INSERT` (new `$13` bind, physically after the existing literal `2`). No
    `force` parameter is added here — Design §3's concurrency guard is the step 13 exclusion
    constraint, not application-level plumbing. In `insert_partition_transaction`, on an `Err` from
    the `INSERT`, match the underlying `sqlx::Error`'s database error for
    `constraint() == Some("lakehouse_partitions_no_overlap")` and, if it matches, `bail!` a legible
    domain error naming the new partition's view/instance/range and pointing at
    `retire_partition_by_metadata` or a range/delta fix, instead of propagating the raw constraint
    error. Separately, add `delete_if_orphan(lake, file_path)` — queries `lakehouse_partitions` for
    the file path and deletes it from object storage only if unreferenced — called from
    `insert_partition` (the wrapper around `insert_partition_transaction`) whenever the transaction
    returns an `Err` and a `file_path` was set, best-effort and never masking the original error.
16. Update all 6 existing `write_partition_from_rows` call sites for the new `sort_order` parameter:
    `net_spans_view.rs`, `thread_spans_view.rs`, `sql_partition_spec.rs`, and
    `block_partition_spec.rs` pass `None`; `metadata_partition_spec.rs` and `merge.rs` pass a real
    value per steps 17-18.
17. `rust/analytics/src/lakehouse/metadata_partition_spec.rs`: add `pub sort_order:
    Option<Vec<String>>` to `MetadataPartitionSpec` and a matching parameter to
    `fetch_metadata_partition_spec`; pass it through to `write_partition_from_rows` in `write()`.
    `rust/analytics/src/lakehouse/blocks_view.rs`: pass
    `Some(vec!["insert_time".to_string()])` at its
    `fetch_metadata_partition_spec` call site.
18. `rust/analytics/src/lakehouse/view.rs`: add `get_merged_partition_sort_order(&self,
    partitions_to_merge: &[Partition]) -> Option<Vec<String>> { None }` to the `View` trait.
    `rust/analytics/src/lakehouse/merge.rs`: call `view.get_merged_partition_sort_order(&filtered_partitions)`
    in `create_merged_partition` *before* `view.merge_partitions(...)` runs (it's a pure function of
    the input slice alone), then pass that value straight to `write_partition_from_rows` — **not**
    gated on `ordering_honored`, because the merge query keeps `ORDER BY insert_time` and so its
    output is `insert_time`-sorted whether or not elision happened (Design §1); the
    `ordering_honored` field from the `MergeQueryResult` (step 5) drives only the memory-regression
    warning, not the recorded value.
    `rust/analytics/src/lakehouse/blocks_view.rs`: override it to return
    `Some(vec!["insert_time".to_string()])` only when every partition in the
    slice is empty or already has that `sort_order`, `None` otherwise — the same predicate step 6
    uses (without step 6's extra "at least one non-empty input" merger-choice clause, which only
    switches between two paths that both uphold the recorded value — Design §4).
    `PartitionSpec::write`'s trait signature (`fn write(&self, lake: Arc<DataLakeConnection>,
    logger: Arc<dyn Logger>) -> Result<()>`) is unchanged by this plan — Design §3's concurrency
    guard is the step 13 exclusion constraint plus step 15's `insert_partition_transaction` error
    translation, not a parameter on this trait or its three implementations
    (`BlockPartitionSpec::write`, `SqlPartitionSpec::write`, `MetadataPartitionSpec::write`), so none
    of them, nor either of their two production call sites (`materialize_partition`'s and
    `write_partition_from_blocks`'s in `jit_partitions.rs`), need any change for it.
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
- `rust/analytics/src/lakehouse/merge.rs` — `PartitionMerger::execute_merge_query` and
  `View::merge_partitions` return a new `MergeQueryResult { stream, ordering_honored }` instead of a
  bare stream; `QueryMerger` ordering builder method; when the declared scan ordering is non-empty,
  `execute_merge_query` sets `datafusion.optimizer.repartition_file_scans = false`, `bail!`s before
  executing only if the optimized physical plan isn't a single output partition, and otherwise
  executes it regardless of whether it still contains a `SortExec`/`SortPreservingMergeExec`,
  reporting `ordering_honored: false` (with a logged warning) when one is found instead of erroring
  (Design §1/§4/Trade-offs); `create_merged_partition` calls
  `view.get_merged_partition_sort_order(&filtered_partitions)` before merging and passes that value
  straight to `write_partition_from_rows` (not gated on `ordering_honored` — the output is
  `insert_time`-sorted whether elision happened or not; `ordering_honored` drives only the warning).
- `rust/analytics/src/lakehouse/batch_partition_merger.rs`,
  `rust/analytics/src/lakehouse/sql_batch_view.rs` — mechanical update of
  `BatchPartitionMerger::execute_merge_query` and `SqlBatchView::merge_partitions` to the new
  `Result<MergeQueryResult>` return type (`ordering_honored: true` always, since neither declares a
  merge scan ordering) — no behavior change.
- `rust/analytics/src/lakehouse/blocks_view.rs` — ordered vs. plain merger selection in
  `merge_partitions`, keyed on `partitions_to_merge`'s recorded `sort_order` and emptiness (empty
  inputs vacuously satisfy the gate; an all-empty merge takes the plain merger); pass a declared
  `sort_order` to `fetch_metadata_partition_spec`; override `get_merged_partition_sort_order` with
  the same input-dependent predicate.
- `rust/analytics/src/lakehouse/metadata_partition_spec.rs` — streaming `write()`; add
  `sort_order` field/parameter and thread it to `write_partition_from_rows`.
- `rust/analytics/src/lakehouse/batch_update.rs` — new `regenerate_partition_range` +
  `regenerate_partition` (both new, alongside the untouched `materialize_partition_range` /
  `materialize_partition`), new `verify_force_regeneration_alignment` guard called from
  `regenerate_partition`.
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
  `lakehouse_partitions.sort_order`; also (Design §3's concurrency guard) `CREATE EXTENSION IF NOT
  EXISTS btree_gist;`, a detect-then-fail pre-check for legacy overlapping partitions, and the
  `lakehouse_partitions_no_overlap` `EXCLUDE` constraint.
- `rust/analytics/src/lakehouse/partition.rs` — `Partition::sort_order` field.
- `rust/analytics/src/lakehouse/write_partition.rs` — `sort_order` parameter on
  `write_partition_from_rows`; new `insert_partition` bind. Also (Design §3) translate an
  `lakehouse_partitions_no_overlap` constraint-violation error into a legible domain error in
  `insert_partition_transaction`, and add `delete_if_orphan` called from `insert_partition` on any
  insert failure. No `force` parameter anywhere in this file.
- `rust/analytics/src/lakehouse/view.rs` — new `get_merged_partition_sort_order()` method; update
  the default `merge_partitions` impl's return type to `Result<MergeQueryResult>`
  (`ordering_honored: true`, mechanical). `PartitionSpec::write`'s signature is unchanged.
- `rust/analytics/src/lakehouse/partition_cache.rs` — read `sort_order` in all 3
  partition-fetching query paths.
- `rust/analytics/src/lakehouse/list_partitions_table_function.rs` — expose `sort_order` column.
- `rust/analytics/src/lakehouse/net_spans_view.rs`,
  `rust/analytics/src/lakehouse/thread_spans_view.rs`,
  `rust/analytics/src/lakehouse/sql_partition_spec.rs`,
  `rust/analytics/src/lakehouse/block_partition_spec.rs` — pass `None` for `sort_order` at their
  `write_partition_from_rows` call sites (new parameter, no behavior change).
- `mkdocs/docs/admin/functions-reference.md` — document the `sort_order` column in
  `list_partitions()`'s column table.

## Trade-offs

- **`ORDER BY` + declared scan ordering vs. `ORDER BY` alone.** An `ORDER BY` with no declared
  source ordering would still produce correct output but pay a full buffering `SortExec` on
  every merge — the exact problem `tasks/jit_single_query_plan.md` was written to avoid on the
  query side. Declaring the `insert_time` ordering lets DataFusion elide the sort node entirely
  instead (the merge source scan is a single sequential file group, so there is no second stream
  requiring a `SortPreservingMergeExec`), so the merge itself gains the same memory bound this
  plan is adding to the source write path — see Design §1 for why this elision is what delivers
  that memory bound (the recorded ordering guarantee is single-column `insert_time`, which a
  fallback `SortExec` preserves anyway, so elision is about memory, not ordering correctness).
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
- **Warning vs. failing the merge when the plan-shape check finds an unelided
  `Sort`/`SortPreservingMergeExec`.** An earlier version of this design had the ordered path `bail!`
  outright whenever elision didn't happen, on the theory that a false `sort_order` guarantee must
  never be persisted. But once the recorded guarantee is single-column `insert_time`, an unelided
  plan is *not* a false-guarantee risk at all: the query keeps `ORDER BY insert_time`, so a fallback
  `SortExec` still produces a correctly `insert_time`-ordered result and the recorded
  `['insert_time']` stays true (Design §1). The only thing an elision miss costs is the memory
  bound. So a hard `bail!` would couple `blocks`-view materialization *availability* to a pure
  memory optimization — a DataFusion upgrade or config regression that broke elision would stall the
  daemon's `blocks` merge on every retry, forever, until a human shipped a fix, for no correctness
  reason. Executing the plan anyway, recording `['insert_time']`, and logging a loud
  memory-regression warning keeps materialization available and the metadata truthful; the merge
  degrades only in peak memory, not in correctness or in the recorded guarantee. The
  not-single-partition case keeps the hard `bail!`, because that shape can't even be executed via
  `execute_stream` — it isn't a graceful-degradation candidate, it's a different and more
  fundamental break, and is left as a loud error accordingly.
- **Generalizing `OrderingBounds` vs. a separate insert-time-only code path.** A parallel
  `sort_and_check_non_overlapping_by_insert_time` function would duplicate ~40 lines with one
  field access changed. An enum parameter keeps one implementation and makes the event-time
  behavior's non-regression explicit (`OrderingBounds::EventTime` at every existing call site).
- **A standalone `regenerate_partition` vs. a `force: bool` threaded through `materialize_partition`.**
  An earlier revision of this plan added a `force: bool` parameter to `materialize_partition` (and,
  transitively, to `PartitionSpec::write`/`write_partition_from_rows`/`insert_partition`) that made
  it skip the up-to-date freshness check and the `get_max_partition_time_delta` subdivision check.
  That reused `materialize_partition`'s branching but meant every layer between it and
  `insert_partition` carried a boolean most callers always passed `false`, purely to support one
  caller. The shipped design instead gives regeneration its own short standalone function
  (`regenerate_partition`) that calls `verify_force_regeneration_alignment` then
  `make_batch_partition_spec` + `write(...)` directly — regeneration's requirements (always-fresh,
  always-whole-bucket, never-subdivide) are exactly what `materialize_partition`'s
  strategy/subdivision branching exists to *avoid* doing unconditionally, so duplicating its
  two-line write call costs less than parameterizing that branching to skip itself, and it keeps
  `PartitionSpec::write` and the rest of the write path signature-identical to before this plan.
  It still reuses the streaming fix from part 2 and the atomic retire-then-insert swap in
  `insert_partition`, since both live underneath `partition_spec.write(...)` regardless of caller.
- **A Postgres `EXCLUDE` constraint vs. a `force`-gated in-transaction `SELECT` recheck for the
  concurrency guard.** An earlier revision closed the "concurrent write into an overlapping range"
  race (Design §3) with an application-level recheck: `insert_partition`, when `force` was true, ran
  one more `SELECT` for an overlap immediately before committing. That only shrank the race window
  (to the gap between that `SELECT` and the transaction's `COMMIT`, under `READ COMMITTED`), only
  protected forced-regeneration writes (ordinary materialization/merge writes had no such recheck),
  and did not close the race between two concurrent forced regenerations of overlapping-but-different
  ranges at all (different advisory-lock keys, so both rechecks could pass before either commits).
  A database-level `EXCLUDE` constraint closes all of that at once: it is enforced by the index
  across concurrent transactions regardless of which code path is writing, so no application code
  needs to know which writes are "forced" or hold a boolean to opt into the check, and the race
  window shrinks to zero rather than merely narrowing. The cost is a one-time migration (detect-then-fail
  against legacy overlaps, since the constraint can't be added `NOT VALID`) and a `btree_gist`
  dependency, both paid once at upgrade time rather than on every write.
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

  **Checked against real production data.** Sampled this production fleet's actual rows,
  including its richest instrumented binaries (Unreal Engine game clients/servers, e.g.
  `UnrealEditor.exe` and similar, alongside the internal Rust services): over a full day,
  18,640 distinct streams, the single widest stream's
  `dependencies_metadata` + `objects_metadata` + `streams.properties` + `processes.properties`
  totaled 4,432 bytes; the same four columns averaged ~2.4 KB/row fleet-wide, consistent with a
  separate check of the full 29-column row estimate against real local telemetry (~2.4-2.8 KB/row
  average). No row anywhere approaches "MB-sized" — three orders of magnitude under the 8 MB
  threshold, at the widest real payload this fleet produces today. The byte-based-vs-row-count
  choice above therefore isn't guarding against an observed failure mode in this fleet, but it
  costs nothing to keep either way, and a future binary with a much larger instrumented
  event/object registry could still change that.

  This also quantifies the OOM hazard this plan's streaming rewrite actually addresses: the same
  day produced ~2.06M blocks (steady ~75k-95k/hour, no unusual daily peak observed) — at ~2.5-2.8
  KB/row that's roughly 5-6 GB of *aggregate* row data for one day's `CreateFromSource` write
  (Design §3's `regenerate_partitions('blocks', <day_begin>, <day_end>, 86400)` writes exactly this
  as one partition, with no subdivision). The hazard was never any single row's width — it's ~2M
  modest rows accumulating in one `Vec<PgRow>`/`RecordBatch` under the current `fetch_all` path. At
  8 MB/chunk and ~2.5 KB/row, that's ~3,000 rows and ~600-700 flushes for a full busy day —
  proportional, not degenerate.
- **Not declaring the JIT-consumer-side `insert_time` scan ordering in this plan.** Doing so
  before every active merged partition is regenerated would silently concatenate out-of-order rows
  for any partition still written under the old, unordered merge — exactly the failure mode
  `sort_and_check_non_overlapping` is designed to catch loudly for *new* overlaps, but it cannot
  detect "sorted-looking file that just happens to be wrong inside its own bounds." Declaring
  trust is a rollout step gated on regenerating and verifying every affected partition, not a
  code change bundled with this plan. Design §4's `sort_order` column turns that gate from a
  flag-day, all-or-nothing trust decision into a per-partition, footer-free check: the follow-up
  plan can require `sort_order = ['insert_time']` on every partition in a query's scope (already
  loaded in the partition cache at planning time) before trusting the declared ordering for that
  scope, instead of trusting it globally once every partition happens to have been regenerated.
  Note the JIT consumer needs only single-column `insert_time` here — it does **not** require a
  trusted `(insert_time, block_id)` *total* order, because it owns its own bucketing determinism via
  tie-atomic, soft-cap segmentation (`tasks/jit_single_query_plan.md`). This deferral is tracked as
  its own open item in `tasks/jit_single_query_plan.md`'s Open Questions, gated on the Rollout
  section below, not on any further design decision in this plan.
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
  `with_merge_scan_ordering`.** The two now carry the *same columns* for blocks-view — both
  `[insert_time]` — but they remain distinct *concepts* worth keeping as separate methods: the
  declared ordering is a *DataFusion validation input* (what the scan promises so the optimizer may
  elide the sort), while `get_merged_partition_sort_order()` is a *Postgres-recorded fact about the
  written output*. They coincide in columns but not in meaning or lifecycle — the recorded fact is
  written unconditionally on the ordered path (the output is `insert_time`-sorted whether or not
  elision happened, §1), whereas the declared ordering is consumed only during physical planning.
  Collapsing them into one field would conflate a planning hint with a persisted guarantee and make
  it impossible for a future view to record an ordering it produces but does not declare (or vice
  versa). A separate method keeps them independently correct.
- **Why the recorded `sort_order` is single-column `[insert_time]`, not `[insert_time, block_id]`.**
  `block_id` is not load-bearing anywhere this plan controls. It is not needed for merge correctness
  (§1 proves the elided merge is correct on `insert_time` alone — inputs are insert-time-disjoint)
  nor for the memory bound (the declared elision ordering is single-column `insert_time`; a fallback
  sort on `insert_time` preserves the guarantee regardless). It was tempting to record the fuller
  order because the merge output *happens* to stay `block_id`-sorted within each `insert_time`, but
  recording it would create a promise the merge does not actually need to keep and would have to
  thread through every path. The one consumer that cares about intra-`insert_time` determinism — the
  JIT segmenter in `tasks/jit_single_query_plan.md` — no longer relies on a stored total order:
  rather than a greedy per-row `max_nb_objects` cut (which was position-sensitive and could split a
  run of equal-`insert_time` blocks differently across runs), it now packs **tie-atomically** with a
  **soft** cap — flushing only at `insert_time` transitions and tolerating a small overshoot — so its
  bucketing is a pure function of the `(insert_time, nb_objects)` multiset, reproducible without any
  `block_id` tiebreak. That works because the split decision runs on cheap block *metadata* and
  `max_nb_objects` bounds output object-count only approximately. So the determinism requirement that
  once seemed to force a stored total order is met locally in the consumer; this plan records and
  declares single-column `insert_time` only. (`data_sql`'s `ORDER BY blocks.insert_time,
  blocks.block_id` may stay as a harmless fresh-write nicety — it costs nothing and yields
  deterministic files — but `block_id` is not a recorded guarantee.)

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
  `repartition_evenly_by_size` + `SortPreservingMergeExec`. This elision assertion guards the
  **memory bound**, not ordering correctness: per Design §1, the merged output is `insert_time`-sorted
  whether or not the sort is elided (a fallback `SortExec` still sorts by `insert_time`), so an
  elision regression costs only the streaming memory bound — the whole point of this plan — not the
  recorded `['insert_time']` guarantee. Asserting elision here is what keeps that memory bound from
  silently regressing. Additionally, add a unit test for `QueryMerger::execute_merge_query`'s
  plan-shape check in isolation — e.g. by defeating elision on purpose (leaving
  `repartition_file_scans` at its default, or otherwise forcing a `Sort`/`SortPreservingMergeExec`
  to remain) — asserting it returns `Ok(MergeQueryResult { ordering_honored: false, .. })` rather
  than an `Err` whenever the resulting plan is still single-partition, and asserting a hard `Err`
  only when the plan comes back multi-partition. This is the regression signal for the fail-open
  behavior in Design §1/Trade-offs, distinct from the `sort_order`-recording integration test in the
  DB-backed suite below.
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
- **Regeneration alignment guard test**: a DB-backed test (alongside the existing
  `materialize_partition_range` tests, e.g. `thread_spans_ordering_db_test.rs`-style) asserting
  `regenerate_partition_range`/`verify_force_regeneration_alignment` returns an `Err` when the
  requested `(begin, end)` partially overlaps an existing partition instead of exactly containing
  it (e.g. a daily partition regenerated with `delta=3600`) — including when the overlapping
  partition was written under a different `file_schema_hash`, since the guard filters
  hash-agnostically (Design §3) — and succeeds when the range exactly
  matches the partition's boundaries — confirming the guard fails loudly rather than silently
  leaving a duplicate partition. Also assert `regenerate_partition_range` returns an `Err` upfront,
  before any partition is written, when `delta` does not exactly tile `(begin, end)` (e.g. a `delta`
  longer than the range, or one that leaves a remainder) — confirming the non-tiling case is
  rejected loudly instead of silently regenerating a partial or empty span.
- **Partition overlap exclusion constraint test**: a DB-backed test asserting the
  `lakehouse_partitions_no_overlap` `EXCLUDE` constraint (Design §3) rejects an overlapping
  same-view/instance/`file_schema_hash` partition insert while allowing an adjacent
  (boundary-sharing) insert, a same-range insert under a different `view_instance_id`, and a
  same-range insert under a different `file_schema_hash` — and that
  `insert_partition_transaction`'s constraint-violation translation surfaces a legible error rather
  than a raw Postgres error. Separately, a migration test simulating a pre-existing v6 database
  (dropping the `sort_order` column and constraint, rolling back `lakehouse_migration.version`)
  asserts the v6→v7 migration's detect-then-fail pre-check `bail!`s naming the conflict when legacy
  overlapping partitions already exist, and succeeds (adding the constraint) when they don't. A
  version of this test (`migration_overlap_constraint_tests.rs`) was written and passed during
  implementation but was then deleted rather than committed: it mutated/downgraded live schema
  (rolling back `lakehouse_migration.version` to simulate a pre-migration database) on whatever
  database `MICROMEGAS_SQL_CONNECTION_STRING` pointed at, guarded only by a doc comment and not
  wired into CI — the same missing-disposable-test-database gap the Implementation Note below
  covers for the rest of this plan's DB-backed tests, so this test is deferred alongside them.
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
    SELECT insert_time,
           lag(insert_time) OVER () AS prev_insert_time
    FROM view_instance('blocks', 'global')
    WHERE insert_time >= $1 AND insert_time < $2
  ) t
  WHERE prev_insert_time > insert_time;
  ```
  A non-zero count means the partition is still out of order (was not regenerated, or the merge
  fix has a bug). The check is single-column `insert_time` because that is the recorded guarantee
  (`block_id` tie-break order is not promised — Trade-offs). This is the "verifiable per partition"
  check referenced in the GitHub issue — run it per merged partition before any future plan declares
  `insert_time` a trusted consumer-side scan ordering.
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
  `sort_order = Some(['insert_time'])`; (b) an order-merged blocks partition (via
  `create_merged_partition` under §1), where every input partition already has
  `sort_order = Some(['insert_time'])`, is inserted with the same value; (c) a partition
  from another view (e.g. `ThreadSpansView`, or any view exercised by the existing
  `histo_view_test.rs`/`sql_view_test.rs` suites) is inserted with `sort_order = None`; (d) a
  `BlocksView` merge where at least one non-empty input partition has `sort_order = None`
  (simulating a pre-fix merged hourly) does not declare a scan ordering, runs the plain unordered
  merger, and is inserted with `sort_order = None` — confirming the merge self-reports as
  unguaranteed instead of inheriting a false guarantee from its sibling, guaranteed-sorted inputs;
  (e) an all-empty `BlocksView` merge (≥2 empty input partitions — the quiet-day daemon case)
  routes to the plain merger, completes without tripping the ordered path's plan-shape check (a
  `SortExec` over an `EmptyExec` is never elided — Design §1), and is inserted with
  `sort_order = Some(['insert_time'])`, vacuously true of its empty output; (f) a
  `BlocksView` merge whose only unguaranteed inputs are *empty* (`NULL`-`sort_order` empty
  partitions mixed with non-empty guaranteed ones) still takes the ordered path and records the
  guarantee — empty inputs vacuously satisfy the gate; and (g) a `BlocksView` ordered merge whose
  inputs all satisfy the input-`sort_order` gate, but whose plan-shape check finds an unelided
  `SortExec`/`SortPreservingMergeExec` (simulated the same way as the offline plan-shape unit test
  above), completes successfully with correct row content and `ordering_honored: false`, and is
  **still inserted with `sort_order = Some(['insert_time'])`** (recording is not gated on
  `ordering_honored` — the output is `insert_time`-sorted whether or not elision happened) while
  logging the memory-regression warning — confirming the memory-health check (Design §1/Trade-offs)
  degrades only in memory, not in correctness or in the recorded guarantee.
- **`list_partitions()` exposure test**: a query-level test asserting `SELECT sort_order FROM
  list_partitions()` returns the column with the expected type and values for a mix of blocks-view
  and other-view partitions — confirming the `ListPartitionsTableProvider` schema/query change
  and the generic `TEXT[]` reader agree (no `DataFusionError` from a schema mismatch).

## Rollout

Which merged `blocks` partitions still need `regenerate_partitions` run against them is answered by
Design §4's `sort_order` column, SQL-queryable rather than tribal knowledge —
`SELECT * FROM list_partitions() WHERE view_set_name = 'blocks' AND sort_order IS NULL AND
num_rows > 0` lists exactly the partitions still lacking the guarantee (empty partitions can stay
`NULL`: they satisfy the merge gate vacuously, Design §1, so regenerating them buys nothing). The
rollout should additionally exclude, via a `begin_insert_time` filter, any partition whose
insert-time range falls within one partition width of the ingestion retention horizon: its source
rows in Postgres may already be partially aged out, so regenerating it would (per Design §3's
accepted "smaller partition outside retention is fine" behavior) truncate data that is still
queryable for up to that window, rather than merely reproduce a smaller-but-equivalent partition.

Because the merge only declares the ordering and records it when every non-empty input already
carries it (Design §1/§4), this query correctly includes not just pre-fix partitions written before
this change, but also any *post-fix* merge whose inputs were still unguaranteed at merge time —
those self-report `NULL` too rather than being missed by this query. The rollout is therefore: run
`regenerate_partitions` finest granularity first, coarsest last (minutely, then hourly, then daily —
the daemon's merge cascade, `maintenance.rs:68-174`), because a merged partition built from inputs
that were `NULL` at the time it was merged stays `NULL` itself until it is regenerated or re-merged,
even after its inputs are later fixed — re-running the query after each pass shows the remaining
work. Once every existing partition reaches `sort_order = ['insert_time']`, every
subsequent ordinary (non-forced) daily merge sees all-guaranteed hourly inputs and automatically
takes the ordered path itself, propagating the guarantee forward with no further manual
regeneration.

Running `regenerate_partitions` over whatever the query above returns in production is an
operational rollout step, tracked separately from — and not blocking — landing the code in this
plan.

## Implementation Note: DB-backed test harness not yet landed

The offline (no-DB) tests from the Testing Strategy section were implemented and pass
(`rust/analytics/tests/blocks_view_merge_ordering_tests.rs`): `make_partitioned_execution_plan`
under `OrderingBounds::InsertTime` (elision + overlap rejection), and `QueryMerger::execute_merge_query`'s
plan-shape check in isolation (elision succeeds; elision is defeated on purpose and reports
`ordering_honored: false` without erroring).

The DB-backed tests (`sort_order` recording for fresh/merged partitions, the regeneration
alignment/tiling guards, the partition overlap exclusion constraint test, the query-level failure
test for `regenerate_partitions`, and the migration test) were prototyped against a real local
Postgres during implementation but are **not** included in the committed test suite: the
prototype's cleanup step deleted rows from the `blocks` ingestion table by time range to make
re-runs idempotent, which is not an acceptable thing for a committed, automatically-runnable test
to do against a real/shared Postgres instance (there is no dedicated, disposable test database in
this environment — these tests would run against whatever `MICROMEGAS_SQL_CONNECTION_STRING`
points at). A version of the exclusion-constraint/migration test
(`migration_overlap_constraint_tests.rs`) was actually written, passed, and briefly committed, but
was then deleted rather than kept: it rolled back `lakehouse_migration.version` and dropped the
`sort_order` column to simulate a pre-migration v6 database, guarded only by a doc comment and not
wired into CI, so an `--ignored` invocation could mutate/downgrade schema on whatever database was
configured — the same class of risk as the deleted `blocks`-table cleanup. This needs a safer
harness before landing as committed tests, e.g.:
- a dedicated/disposable test database or schema (spun up and torn down per test run), so cleanup
  can freely `DROP`/`TRUNCATE`/roll back schema version without touching real data or a real
  database's migration state; or
- scoping all cleanup strictly to rows the test itself created (by process/stream id), never a
  blanket time-range `DELETE` against `blocks`; or
- accepting permanent, harmless accumulation of tiny test partitions in a fixed, far-past time
  window (no deletion at all), if that's judged acceptable for the target test database.

Follow-up: build one of the above, then add back DB-backed coverage for the `sort_order` /
`regenerate_partitions` / exclusion-constraint behaviors this plan introduces, per the Testing
Strategy section above. The migration test (v6→v7, including simulating a pre-existing v6 database
by dropping the `sort_order` column and rolling back `lakehouse_migration.version`) additionally
mutates the shared `lakehouse_partitions` schema itself while other tests may be reading it
concurrently, so it should run in isolation (e.g. its own dedicated database) rather than share a
database with any other test.

## Open Questions

None. (The former question — whether the `block_id` tie-break is load-bearing — is resolved by
*removing* the requirement rather than answering it: the recorded/declared blocks-view ordering is
now single-column `['insert_time']`, and the one consumer that needed intra-`insert_time`
determinism, the JIT segmenter, owns it locally by packing **tie-atomically with a soft
`max_nb_objects` cap** — flushing only at `insert_time` transitions — so its bucketing is a pure
function of the `(insert_time, nb_objects)` multiset, reproducible without any total order. See the
Trade-offs bullet "Why the recorded `sort_order` is single-column `[insert_time]`" and
`tasks/jit_single_query_plan.md`'s segmenter design.)
