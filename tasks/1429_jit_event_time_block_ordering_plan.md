# JIT Block Ordering by Event Time Plan (#1429)

## Overview

`ThreadSpansView` declares `ScanOrdering::Concatenated { columns: [begin ASC] }`, which lets
`EnforceSorting` elide the `ORDER BY begin` in the Perfetto export path (#1303). That declaration is
violated *inside a single partition file* because JIT source blocks are enumerated in `insert_time`
order while the writer treats the list as event-time ordered. Perfetto export fails loudly (`thread
spans out of order`); plain `ORDER BY begin` queries silently return mis-ordered rows.

This plan makes the block list genuinely event-time ordered so the existing comments become true,
without weakening the ordering declaration or changing the scan plan shape (so #1303's memory win is
preserved). The non-obvious part is that reordering blocks by event time breaks a second invariant
the reorder makes easy to miss: JIT partitions' `insert_time` ranges must stay non-overlapping, or
the `lakehouse_partitions_no_overlap` exclusion constraint rejects the write. The design therefore
pairs event-time ordering with *insert-safe cut points*, and scopes the ordering change to the two
views that need it.

## Current State

### Where the ordering comes from

`rust/analytics/src/lakehouse/jit_partitions.rs:124` (stream variant) and `:279` (process variant)
both enumerate source blocks with `ORDER BY insert_time, block_id`, then walk the result and cut
partitions whenever `max_nb_objects` would be exceeded. The two cut loops are near-duplicates
(`jit_partitions.rs:140-186` and `:297-385`).

`rust/analytics/src/lakehouse/thread_spans_view.rs:132-133` states the assumption outright:

```rust
// for jit partitions, we assume that the blocks were registered in order
// since they are built based on begin_ticks, not insert_time
```

It then walks `spec.blocks` in list order, cuts them into tick-contiguous runs, and appends one call
tree per run (`thread_spans_view.rs:154-180`). Row order in the parquet file is append order.
`net_spans_view.rs:157-188` has the identical shape.

### Why the assumption is false

`telemetry-sink`'s HTTP event sink uploads blocks concurrently (semaphore-bounded `spawn_item` plus
retry/backoff), so two consecutive blocks of one thread stream can be registered in reverse order.
Measured on one thread stream over ~2.5 minutes: **430 blocks, 6 event-time inversions** (~1.4%).
Inversions are local — the observed pairs are ~12 ms apart in insert time and adjacent in event time
(`B.end_ticks == A.begin_ticks`).

### The three failures this causes today

1. **Row-level mis-order inside one file.** `write_partition` emits A's call tree then B's, so the
   `begin` column regresses mid-file. `perfetto_trace_execution_plan.rs:405-411`'s monotonicity guard
   is the only surface that notices, and it aborts the export. A direct
   `SELECT ... ORDER BY begin` gets its `Sort` elided and silently returns mis-ordered rows.
   (`ORDER BY "begin", "end"` works around it — a two-column requirement is not satisfied by the
   declared single-column ordering, so a real `SortExec` survives.)

2. **Spurious call-tree fragmentation.** The tick-contiguity test (`block.begin_ticks == last_end`)
   sees an inversion as a discontinuity and starts a new call tree with a new synthetic root, even
   though the blocks are genuinely contiguous.

3. **Wrong recorded event-time bounds.** `thread_spans_view.rs:181-183` and
   `net_spans_view.rs:189-191` derive `rows_time_range` from `blocks[0].begin_ticks` /
   `blocks[last].end_ticks`, i.e. from insert-ordered endpoints. Under an inversion those bounds are
   *narrower* than the real row range. That is worse than cosmetic: `LivePartitionProvider::fetch`
   selects partitions with `min_event_time <= $end AND max_event_time >= $begin`
   (`partition_cache.rs:375-376`), so a query whose window falls in the truncated margin **skips the
   partition entirely and silently drops rows**. It also feeds
   `sort_and_check_non_overlapping`/`attach_ordering_statistics`, so the cross-partition non-overlap
   check can pass on a genuine overlap.

### The invariant that constrains the fix

`migration.rs:502-509` installs:

```sql
ALTER TABLE lakehouse_partitions ADD CONSTRAINT lakehouse_partitions_no_overlap
  EXCLUDE USING gist (view_set_name WITH =, view_instance_id WITH =,
                      file_schema_hash WITH =,
                      tstzrange(begin_insert_time, end_insert_time) WITH &&);
```

Today JIT partitions satisfy this **by construction**: the block list is insert-ordered, so cutting
it yields partitions whose `[min_insert, max_insert]` ranges are non-decreasing and non-overlapping
(`tstzrange` is `[)`, so a shared boundary does not conflict). Insert-time slices at the segment
level (`generate_stream_jit_partitions:234-249`, 1-hour `max_insert_time_slice`) are half-open, so
they cannot overlap either.

Sorting the block list by `begin_ticks` destroys that guarantee. If an inversion straddles a cut
point, partition *k*'s `max_insert` exceeds partition *k+1*'s `min_insert`, the ranges overlap, and
`insert_partition` fails with the exclusion-constraint error at `write_partition.rs:438-454`. With
~66 blocks/partition and a ~1.4% inversion rate the issue estimates ~10% of streams would hit this —
i.e. the naive fix trades a read-time failure for a write-time failure on a tenth of streams. This
plan's cut-point rule is what closes that gap.

### Blast radius of a change to `jit_partitions.rs`

`generate_process_jit_partitions` is shared by six views — `log_view`, `metrics_view`, `images_view`,
`async_events_view`, `otel/spans_view`, `net_spans_view` — and `generate_stream_jit_partitions` only
by `thread_spans_view`. The first five decode each block independently
(`write_partition_from_blocks` → `BlockPartitionSpec::write`, which tracks per-block event-time
ranges), declare no scan ordering, and are order-insensitive. Only `thread_spans_view` and
`net_spans_view` build cross-block trees and derive bounds from list endpoints, so only they need
event-time ordering — and changing grouping forces partition retirement, so touching the other five
would cost five extra schema-hash bumps for no benefit.

## Design

### 1. Ordering becomes an explicit, per-view choice

Add to `jit_partitions.rs`:

```rust
/// How source blocks are ordered before being cut into JIT partitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockOrder {
    /// Registration order (`insert_time`, `block_id`). Correct for views that decode each block
    /// independently and declare no scan ordering.
    InsertTime,
    /// Event order (`begin_ticks`, `end_ticks`). Required by views that build cross-block trees or
    /// derive event-time bounds from the list endpoints, and by any view declaring
    /// `ScanOrdering::Concatenated` over an event-time column.
    EventTime,
}

pub struct JitPartitionConfig {
    pub max_nb_objects: i64,
    pub max_insert_time_slice: TimeDelta,
    pub block_order: BlockOrder,
}
```

`Default` keeps `block_order: BlockOrder::InsertTime`, so the five order-insensitive views are
untouched. `thread_spans_view` and `net_spans_view` construct
`JitPartitionConfig { block_order: BlockOrder::EventTime, ..Default::default() }`.

Ticks are process-relative (`ConvertTicks::delta_ticks_to_ns` is applied to raw `begin_ticks`), so
`begin_ticks` is comparable across streams of one process — the process variant can sort by it too.

### 2. One shared, pure grouping function

Extract the duplicated cut loops into a pure, DB-free function — this is what makes the invariants
unit-testable:

```rust
pub fn group_blocks_into_partitions(
    config: &JitPartitionConfig,
    blocks: Vec<Arc<PartitionSourceBlock>>,
) -> Vec<SourceDataBlocksInMemory>
```

Both `generate_*_jit_partitions_segment` functions collect their record batches into a
`Vec<Arc<PartitionSourceBlock>>` (they already hold every batch in memory) and delegate. The SQL
`ORDER BY insert_time, block_id` stays as the stable base order; the sort inside the helper is a
*stable* sort on `(begin_ticks, end_ticks)`, so ties break on `(insert_time, block_id)` and grouping
is deterministic.

### 3. Insert-safe cut points

A cut before block `i` is legal only if every block already in the current partition was inserted no
later than every remaining block. Precompute suffix minima of `insert_time` and test that before
cutting:

```rust
// suffix_min[i] = min(insert_time of blocks[i..]);  suffix_min[n] = DateTime::<Utc>::MAX_UTC
let mut out = vec![];
let mut start = 0usize;
let mut nb_objects = 0i64;
let mut prefix_max_insert = DateTime::<Utc>::MIN_UTC;   // max insert_time of blocks[start..i]

for i in 0..n {
    let block_nb = blocks[i].block.nb_objects as i64;
    let full = nb_objects + block_nb > config.max_nb_objects && i > start;
    // Cutting here would give blocks[start..i] an insert range ending at prefix_max_insert and
    // every later partition an insert range starting at >= suffix_min[i]. Only cut when those
    // cannot overlap, or the lakehouse_partitions_no_overlap exclusion constraint rejects the
    // second insert.
    if full && prefix_max_insert <= suffix_min[i] {
        out.push(make_partition(&blocks[start..i], nb_objects));
        start = i;
        nb_objects = 0;
        prefix_max_insert = DateTime::<Utc>::MIN_UTC;
    } else if full {
        // deferred cut: count it, warn once per partition below
    }
    nb_objects += block_nb;
    prefix_max_insert = prefix_max_insert.max(blocks[i].block.insert_time);
}
if start < n {
    out.push(make_partition(&blocks[start..n], nb_objects));
}
```

Properties:

- Under `BlockOrder::InsertTime` the guard is always satisfied (`prefix_max_insert ==
  blocks[i-1].insert_time <= blocks[i].insert_time == suffix_min[i]`), so behavior is **bit-identical
  to today** for the five untouched views.
- Under `BlockOrder::EventTime` a deferred cut costs one or two extra blocks in practice (observed
  inversions are adjacent, ~12 ms apart).
- `<=` rather than `<` matches today's tolerance for equal insert timestamps: `tstzrange` is `[)`, so
  `[a, t)` and `[t, b)` do not conflict.
- Because `suffix_min` covers the whole remainder (not just the next partition), non-overlap is
  transitive across all later partitions, not just adjacent ones.

Cross-segment safety is unchanged and free: segments are queried with `insert_time >= begin AND
insert_time < end`, so blocks from different segments have disjoint insert times.

When a cut is deferred, `warn!` once per emitted partition with the stream/process id, the deferred
block count, and the final object count — the observability hook for the pathological case where a
long inversion chain pushes a partition well past `max_nb_objects`.

### 4. Real min/max instead of list endpoints

With the list no longer in insert order, every first/last-element read of `insert_time` becomes
wrong. Replace with real min/max:

- `jit_partitions.rs:597-600` (`get_part_insert_time_range`) — feeds `is_jit_partition_up_to_date`.
- `jit_partitions.rs:618-622` (`write_partition_from_blocks`) — feeds `BlockPartitionSpec::insert_range`.
- `thread_spans_view.rs:134-135`, `net_spans_view.rs:130-131` — feed `write_partition_from_rows`'s
  `insert_range`.

These are the same value under `InsertTime` ordering, so the change is a no-op for the untouched
views. Factor it as one helper on the block slice (e.g. `insert_time_range(blocks) -> TimeRange`) so
there is a single implementation to keep right.

Recorded **event-time** bounds get the same treatment. Under event-time ordering
`blocks[0].begin_ticks` / `blocks[last].end_ticks` become correct by construction, but relying on
that re-creates the fragility this plan is removing. Use min/max over the slice for `rows_time_range`
in `thread_spans_view.rs:181-183` and `net_spans_view.rs:189-191`. (`net_spans_view` already prefers
`record_builder.get_time_range()` and falls back to the endpoints; the fallback gets fixed too.)

### 5. Materialization-time monotonicity check

Move the failure from every read to one write. After `record_builder.finish()` in
`thread_spans_view::write_partition`, scan the `begin` column of the produced batch and
`anyhow::ensure!` it is non-decreasing, naming the stream and the offending row. One pass over one
already-materialized column; it turns a contract breach into a single loud materialization error with
the writer in the stack trace, instead of an export error one layer removed from the cause. The
read-time guard in `perfetto_trace_execution_plan.rs` stays as the backstop for pre-existing
partitions.

### 6. Retiring existing partitions

Re-grouping changes which blocks land in which partition, so existing `thread_spans` / `net_spans`
partitions must stop being used. Bump `SCHEMA_VERSION` in both views (`1` → `2`) even though the
Arrow schema is unchanged:

- Queries filter partitions by `file_schema_hash`, so old partitions become invisible immediately.
- The exclusion constraint is scoped by `file_schema_hash`, so old and new partitions coexist legally
  during the transition (`migration.rs:496-501`).
- `retire_partitions` in the write path does *not* filter by schema hash, so any old partition
  contained in a newly written insert range is retired automatically as data is re-materialized.
- The remainder is mopped up with `micromegas.admin.retire_incompatible_partitions(client,
  'thread_spans')` (and `'net_spans'`), which drives the `retire_partition_by_metadata` UDF off a
  `list_partitions()`/`list_view_sets()` schema-hash mismatch.

### Data flow after the change

```
blocks_view (SQL: ORDER BY insert_time, block_id)
        │
        ▼
group_blocks_into_partitions(config, blocks)
        │  1. stable sort by (begin_ticks, end_ticks)     [EventTime only]
        │  2. cut on max_nb_objects, deferring any cut that
        │     would overlap insert ranges
        ▼
Vec<SourceDataBlocksInMemory>
        │   event-time ordered within a partition
        │   event-time ranges ascending & non-overlapping across partitions
        │   insert-time ranges non-overlapping across partitions
        ▼
thread_spans_view::write_partition
        │  tick-contiguous runs → one call tree each, appended in event order
        │  insert_range  = min/max insert_time over blocks
        │  rows_time_range = min/max event time over blocks
        │  ensure! begin non-decreasing
        ▼
parquet file: begin ascending  →  ScanOrdering::Concatenated is honest
```

## Implementation Steps

### Phase 1 — grouping mechanics (`jit_partitions.rs`)

1. Add `BlockOrder` and the `block_order` field on `JitPartitionConfig`; `Default` →
   `BlockOrder::InsertTime`.
2. Add `insert_time_range(blocks: &[Arc<PartitionSourceBlock>]) -> Result<TimeRange>` returning real
   min/max, and use it in `get_part_insert_time_range` and `write_partition_from_blocks`.
3. Extract `group_blocks_into_partitions(config, blocks)` with the stable event-time sort, the
   suffix-min insert-safe cut rule, and the deferred-cut `warn!`. Preserve `block_ids_hash =
   partition_nb_objects.to_le_bytes()`.
4. Rewrite `generate_stream_jit_partitions_segment` and `generate_process_jit_partitions_segment` to
   collect blocks into a `Vec` and delegate to the helper, deleting both duplicated cut loops.
5. Update the module/function docs to describe the two orderings and the insert-range invariant the
   cut rule upholds.

### Phase 2 — thread spans (`thread_spans_view.rs`)

6. Pass `block_order: BlockOrder::EventTime` in `jit_update`.
7. Replace the first/last `insert_time` reads (`:134-135`) with `insert_time_range`, and the
   `rows_time_range` endpoints (`:181-183`) with min/max over the blocks.
8. Add the `begin`-monotonicity `ensure!` after `record_builder.finish()`.
9. Rewrite the `:132-133` comment from an assumption to an enforced invariant, pointing at
   `BlockOrder::EventTime`.
10. Bump `SCHEMA_VERSION` to `2`.

### Phase 3 — net spans (`net_spans_view.rs`)

11. Same as steps 6, 7, 9, 10 (`:130-131`, `:189-191`; the `get_time_range()` fallback). No
    monotonicity check — `net_spans` declares no scan ordering.

### Phase 4 — stale documentation

12. `view.rs:158-163` — the `Concatenated` contract note: drop "documented but not enforced", state
    that `ThreadSpansView` obtains it from `BlockOrder::EventTime` grouping, and keep the residual
    caveats (cross-hour-segment inversions, TSC-frequency drift).
13. `partitioned_execution_plan.rs:52-58` and `:78-82` — the non-overlap error message and doc
    currently list "blocks registered out of event-time order" as a likely cause; that cause is now
    structurally excluded within a segment. Reduce it to the residual cases.
14. `write_partition.rs:365-366` — the same stale "we assume that the blocks were registered in
    order" comment sits above `retire_partitions`; it is unrelated to what that call does. Remove or
    replace with a note about containment-based retirement.

### Phase 5 — tests

15. New `rust/analytics/tests/jit_partition_grouping_tests.rs` (see Testing Strategy).
16. Extend `rust/analytics/tests/thread_spans_ordering_db_test.rs` with a reversed-registration case.

### Phase 6 — rollout

17. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.
18. After deploy, run `micromegas.admin.retire_incompatible_partitions` for `thread_spans` and
    `net_spans`.

## Files to Modify

| File | Change |
|---|---|
| `rust/analytics/src/lakehouse/jit_partitions.rs` | `BlockOrder`, config field, `insert_time_range`, `group_blocks_into_partitions`, both segment fns delegate |
| `rust/analytics/src/lakehouse/thread_spans_view.rs` | `EventTime` config, min/max bounds, monotonicity `ensure!`, comment, `SCHEMA_VERSION` → 2 |
| `rust/analytics/src/lakehouse/net_spans_view.rs` | `EventTime` config, min/max bounds, comment, `SCHEMA_VERSION` → 2 |
| `rust/analytics/src/lakehouse/view.rs` | `get_scan_output_ordering` doc: assumption → enforced |
| `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` | non-overlap doc + error message: drop the now-excluded cause |
| `rust/analytics/src/lakehouse/write_partition.rs` | remove stale block-ordering comment at `:365` |
| `rust/analytics/tests/jit_partition_grouping_tests.rs` | new — pure grouping invariants |
| `rust/analytics/tests/thread_spans_ordering_db_test.rs` | new case: blocks registered out of event order |

## Trade-offs

- **Reorder the blocks vs. weaken the declaration.** `ScanOrdering::Unordered` reintroduces the OOM
  #1303 fixed (one `ExternalSorter` per thread, concurrently, against a shared
  `datafusion.runtime.memory_limit`). `PerFile` + `SortPreservingMergeExec` was already considered
  and rejected in `tasks/completed/1297_perfetto_redundant_sort_plan.md:461-467` — it opens every
  partition file concurrently per thread. Fixing the data keeps the scan plan and fixes the silent
  SQL mis-order and the truncated-bounds data loss too, which a declaration change would not.

- **Insert-safe cut points vs. deriving insert ranges from a tiling.** The alternative is to store
  each partition's `[begin_insert_time, end_insert_time)` as a synthetic non-overlapping tiling
  (`begin = max(min_insert, prev_end)`) instead of the true min/max, cutting wherever
  `max_nb_objects` says. It keeps partition sizes exact but breaks the "a partition's insert range
  covers its blocks' insert times" invariant that `is_jit_partition_up_to_date`,
  `PartitionCache::filter_insert_range` and `retire_expired_partitions` all read, in exchange for
  saving a block or two per cut. Deferring the cut keeps one invariant instead of introducing a
  second, weaker one.

- **Deferred cuts can exceed `max_nb_objects`.** Bounded in the worst case by the segment (one hour
  of one stream). Observed inversions defer by one block. Mitigated by the `warn!`, not by a cap: a
  cap would have to either cut unsafely (write fails) or silently drop blocks. Revisit only if the
  warning is ever seen with a large deferral count.

- **Per-view `BlockOrder` vs. event-time ordering everywhere.** A global switch would be simpler to
  reason about and would make partition event-time bounds structural for all JIT views. It also
  changes grouping for `log_entries`, `measures`, `images`, `async_events` and `otel_spans` — five
  more `SCHEMA_VERSION` bumps and five more retirement sweeps — for views that decode blocks
  independently, track event bounds per block, and declare no ordering. The knob puts the cost where
  the benefit is; the default keeps those five bit-identical.

- **Sorting in Rust vs. changing the SQL `ORDER BY`.** The issue proposes `ORDER BY begin_ticks,
  block_id` in SQL. Sorting inside `group_blocks_into_partitions` instead makes grouping unit-testable
  without a live lake and keeps the ordering decision next to the cut rule that depends on it. The
  SQL `ORDER BY insert_time, block_id` is retained as the stable tiebreak base.

- **Schema-hash bump vs. waiting for JIT expiry.** JIT partitions age out via
  `retire_expired_partitions`, so doing nothing eventually converges — but until then queries keep
  reading the mis-grouped partitions and the bug persists on existing data. A hash bump makes the
  cutover immediate and deterministic, at the cost of dead storage until the retirement sweep runs.

## Documentation

- Rustdoc is the primary surface: `jit_partitions.rs` (the two orderings and the insert-range
  invariant), `view.rs:150-176` (the `Concatenated` contract), `partitioned_execution_plan.rs:52-87`
  (remaining causes of a non-overlap failure), `thread_spans_view.rs:132` and
  `write_partition.rs:365` (stale comments).
- `tasks/completed/1297_perfetto_redundant_sort_plan.md` flagged the unenforced assumption only in
  the cross-partition dimension; this plan supersedes that note. Cross-link from this file rather
  than editing the completed plan.
- Optional: a sentence in `doc/how_to_query/README.md` §`thread_spans` (line 332) stating that a
  `thread_spans` view instance scan is ordered by `begin`, so `ORDER BY begin` is free. No existing
  doc page describes the JIT partitioning internals, so nothing else needs updating.

## Testing Strategy

### Unit — `rust/analytics/tests/jit_partition_grouping_tests.rs` (no DB)

Build synthetic `PartitionSourceBlock` lists (contiguous `begin_ticks`/`end_ticks`, `insert_time`
permuted) and assert on `group_blocks_into_partitions`:

1. **Reproduces the bug's shape.** Two consecutive blocks registered in reverse order, single
   partition: output blocks are event-ordered.
2. **Event ordering across partitions.** Enough blocks to force several cuts: concatenating the
   partitions yields non-decreasing `begin_ticks`, and each partition's `[min begin_ticks, max
   end_ticks]` is disjoint from and ordered against the next.
3. **Insert ranges never overlap.** For every adjacent pair, `max_insert(P_k) <= min_insert(P_k+1)` —
   the exclusion-constraint precondition. Include a case with an inversion placed exactly on the
   `max_nb_objects` cut point (the ~10% case that would otherwise fail the write) and assert the cut
   moved rather than overlapped.
4. **`InsertTime` is unchanged.** Same inputs under `BlockOrder::InsertTime` produce exactly today's
   grouping, including partition boundaries and `block_ids_hash`.
5. **Size respected when it can be.** With no inversions, `EventTime` grouping matches `InsertTime`
   grouping block-for-block.
6. **Degenerate inputs.** Empty list; one block; all blocks with identical `insert_time`; identical
   `begin_ticks` (tiebreak determinism — same input order in, same grouping out).

### Unit — bounds helpers

7. `insert_time_range` over a permuted list returns true min/max, and equals the endpoints for a
   sorted list.
8. `thread_spans_view`'s monotonicity `ensure!` rejects a hand-built batch with a regressing `begin`
   and passes a monotone one.

### Integration — `thread_spans_ordering_db_test.rs` (live lake)

9. New case mirroring the existing harness: push N blocks to one thread stream but insert them into
   the ingestion service **out of event order** (swap an adjacent pair, and a second pair positioned
   to straddle a partition cut by lowering `max_nb_objects` in the test config). Then assert:
   - `jit_update` succeeds (no exclusion-constraint error) — the write-side regression this plan's
     cut rule prevents;
   - `SELECT "begin" FROM view_instance('thread_spans', <stream>) ORDER BY begin` returns
     non-decreasing `begin` with the `Sort` elided;
   - `perfetto_trace_chunks(...)` completes without `thread spans out of order`;
   - `list_partitions()` shows non-overlapping `[min_event_time, max_event_time]` and
     `[begin_insert_time, end_insert_time]` ranges, and the union of event ranges covers every
     ingested block (guards the truncated-bounds data loss).
10. Confirm the existing `net_spans_test.rs` and `span_tests.rs` still pass with the bumped schema
    versions.

### Manual verification against the live lake

11. On the process/stream from the issue: `perfetto_trace_chunks` over the failing window succeeds;
    `ORDER BY "begin"` and `ORDER BY "begin", "end"` return identical row sequences (today they
    differ — 4 regressions in the reported ~627k-row window); `process_spans(process_id, 'thread')`
    row count is unchanged.

## Open Questions

1. **Should `SCHEMA_VERSION` be bumped, or is JIT expiry acceptable?** Bumping is deterministic but
   strands storage until `retire_incompatible_partitions` runs. Recommendation: bump — the silent
   mis-order and the row-dropping bounds bug are both live on existing partitions.
2. **Is the deferred-cut `warn!` enough, or should a hard ceiling exist?** No safe cut exists once the
   ceiling is hit, so any cap would have to accept an insert-range overlap. Recommendation: ship the
   warning, add a metric only if it fires in practice.
3. **Cross-segment inversions stay possible.** `generate_stream_jit_partitions` segments on 1-hour
   `insert_time` slices, so an inversion straddling an hour boundary still produces overlapping
   partition event ranges and trips `sort_and_check_non_overlapping`. Observed inversions are ~12 ms,
   so this is theoretical; both guards stay as backstops. Worth a follow-up issue rather than
   widening this change?
4. **`net_spans` monotonicity check.** `net_spans` declares no scan ordering, so nothing requires
   `begin_time` monotonicity today. Leave it unchecked, or add the same `ensure!` so a future
   `Concatenated` declaration starts from an enforced invariant?
