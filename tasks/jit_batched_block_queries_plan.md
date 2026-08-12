# Faster JIT Partition Generation: Batched Block Queries + Streaming Specs

## Overview

JIT (just-in-time) lakehouse views resolve their source blocks in
`rust/analytics/src/lakehouse/jit_partitions.rs` by slicing the insert-time range into 1-hour
segments and running **one DataFusion query per segment, sequentially** — a trace showed ~256ms
per `collect_partition_blocks` span, back-to-back, ~15s for one query. On top of that, the
functions return a `Vec<SourceDataBlocksInMemory>` holding **every block of the whole range in
RAM at once**.

Two coupled changes:

1. **Fewer queries** — query blocks in *batches of many hour-buckets* (default 1 day = 24 buckets
   per query) instead of one query per hour, recovering the hour bucketing client-side from each
   row's `insert_time`. A week-long range goes from 168 sequential queries to 7, and source
   partitions overlapping several hours are re-read once per batch instead of once per hour.
2. **Bounded memory** — the generators return a **stream of partition specs** instead of a `Vec`,
   consuming each batch query via `execute_stream()`. Nothing holds the full range's block list:
   peak memory is one hour-bucket's blocks plus the one spec currently being materialized.

**The grouping algorithm is not touched.** Each hour bucket's blocks are still handed to the
existing `group_blocks_into_partitions` (`jit_partitions.rs:227`) exactly as today, so the emitted
specs are byte-identical to the current code and every cached JIT partition stays valid. This is
what keeps the change cheap: all the risk in this area lives in the bucketing/cut-point rules, and
this plan changes neither.

This supersedes the earlier draft of this file (`tasks/jit_single_query_plan.md`, renamed), which
predated #1429/#1440 and proposed replacing the segmentation logic with a streaming
`JitPartitionAccumulator`. That part is now both unnecessary and unimplementable — see
*Superseded Design* below.

## Current State

### Call graph

Seven views call into this file:

- `generate_process_jit_partitions` (`jit_partitions.rs:635`) — `log_view.rs:172`,
  `metrics_view.rs:174`, `net_spans_view.rs:356`, `images_view.rs:129`,
  `async_events_view.rs:157`, `otel/spans_view.rs:133`. Callers loop over the returned vec:
  `is_jit_partition_up_to_date` → `write_partition_from_blocks` (or `update_partition` for
  `net_spans`), one spec at a time.
- `generate_stream_jit_partitions` (`jit_partitions.rs:465`) — `thread_spans_view.rs:375`; caller
  loops `update_partition` per spec.

`net_spans_view` and `thread_spans_view` pass `BlockOrder::EventTime`; the other five use the
`JitPartitionConfig::default()` (`BlockOrder::InsertTime`).

Both generators have the same three-step shape:

1. **MIN/MAX pre-query** — find the insert-time range of blocks whose *event* time overlaps the
   query range (`generate_process_jit_partitions` inlines it at `:658-702`; the stream variant uses
   `get_insert_time_range`, `:353-403`). Truncated to slice boundaries:
   `[trunc(min), trunc(max) + slice)`.
2. **Postgres fetch** — `PartitionCache::fetch_overlapping_insert_range_for_view` loads the
   blocks-view partition list for that whole insert range (`:493-502`, `:704-713`) — already
   hoisted out of the loop by #1335.
3. **Sequential per-hour loop** (`:504-523`, `:715-735`) — for each 1h window, call
   `generate_*_jit_partitions_segment`, which filters the partition cache (`filter_insert_range`,
   overlap semantics — `partition_cache.rs:248`), builds SQL scoped to
   `insert_time >= begin AND insert_time < end ORDER BY insert_time, block_id`, runs
   `query_partitions(...).collect()`, parses rows into `PartitionSourceBlock`s, and calls
   `group_blocks_into_partitions`. All results accumulate into one output vec.

The segment queries filter by **insert_time only** (not event time): a JIT partition must cover its
whole hour bucket regardless of the triggering query's event range, so cached partitions are
query-independent. That must be preserved.

### What's wasteful / dangerous

- **Per-query overhead × N segments, serialized.** Each hour pays `SessionContext` setup, object
  store round trips, and Parquet decode (~256ms observed) even for sparse hours.
- **Redundant re-reads.** `filter_insert_range` keeps any blocks-view partition *overlapping* the
  hour; a merged daily partition is scanned by all 24 hour-queries.
- **Unbounded output.** The full range's `Vec<SourceDataBlocksInMemory>` (block metadata +
  per-block `Arc<StreamMetadata>`) sits in RAM while partitions are materialized one at a time. For
  a long-lived chatty process this list alone can be large. Pre-existing problem the new API fixes.

### Invariants that must be preserved

- **Bucketing must be reproducible across runs.** `is_jit_partition_up_to_date` (`:756`) matches
  each spec against `lakehouse_partitions` by its blocks' `[min_insert_time, max_insert_time]`, so
  cache reuse depends on a given block set producing the same specs it produced last run. Buckets
  are `duration_trunc(max_insert_time_slice)`-aligned and specs never span a bucket boundary.
  `row_bucket = insert_time.duration_trunc(slice)` reproduces today's `[begin, end)` windows
  exactly, because those windows are themselves `duration_trunc`-aligned — so segmentation needs no
  query-per-bucket.
- **Tie order inside a bucket must be preserved.** Under `BlockOrder::InsertTime`,
  `group_blocks_into_partitions` cuts the list greedily in arrival order, so *which* equal-
  `insert_time` blocks land on either side of a cut depends on the SQL tie order. The batched query
  therefore keeps `ORDER BY insert_time, block_id` verbatim. (This is also why the sort-elision
  follow-up below is not free.)
- **Grouping stays per hour bucket, never per batch.** `View::get_scan_output_ordering`'s docs
  (`view.rs:186`) list "an insert-time inversion straddling a JIT *segment* boundary (segments are
  still grouped independently)" as a known, loudly-backstopped residual caveat. Widening the
  *query* window does not touch that; widening the *grouping* window would change which inversions
  are visible to `group_blocks_into_partitions` and change emitted specs. Only the query window
  widens here.

### Existing streaming precedent

`SourceDataBlocks::get_blocks_stream` (`partition_source_data.rs:121`) — the global-view analog —
already consumes a blocks query via `df.execute_stream()` inside `async_stream::try_stream!`,
hoisting column accessors once per batch. This rewrite follows that pattern. `query_partitions`
(`query.rs:79-91`) returns a `DataFrame` precisely to leave streaming open. `async-stream` and
`futures` are already dependencies of the analytics crate.

## Design

### New shape (both variants)

```
pre-queries (unchanged, run before the stream is returned):
  1. MIN/MAX insert-time query               -> [trunc(min), trunc(max) + slice)
  2. fetch_overlapping_insert_range_for_view -> PartitionCache

returned stream (async_stream::try_stream!):
  for each batch window (max_query_insert_time_range wide, slice-aligned):
      filter cache to the batch window
      SQL: insert_time >= batch_begin AND insert_time < batch_end
           ORDER BY insert_time, block_id
      df.execute_stream()
      for each RecordBatch (accessors hoisted once per batch):
          for each row:
              bucket = insert_time.duration_trunc(slice)
              if bucket != current_bucket:
                  yield each spec from group_blocks_into_partitions(config, take(pending))
                  current_bucket = bucket
              pending.push(block)
  yield each spec from group_blocks_into_partitions(config, take(pending))   // tail bucket
```

- Rows arrive ordered by `insert_time`, so a bucket is complete as soon as a later-bucket row
  appears: `pending` never holds more than one hour bucket's blocks, independent of batch width.
- Batch windows are consecutive runs of hour-buckets, so window edges coincide with bucket edges
  and batch boundaries need no special handling. Batches run sequentially in ascending time order,
  so the concatenated bucket sequence is exactly today's.
- Empty buckets contribute no rows, hence no specs — same as today's empty per-hour query.

### Equivalence argument (keep in the PR description)

For a fixed set of source blocks, the new code feeds `group_blocks_into_partitions` *the same
list, in the same order, for the same buckets* as the old code:

- Same bucket set: batch windows tile `[trunc(min), trunc(max) + slice)` with slice-aligned edges,
  the same range the old loop walked.
- Same membership: the old per-bucket SQL predicate `insert_time >= b AND insert_time < b+slice` is
  exactly `insert_time.duration_trunc(slice) == b` for rows inside the batch window.
- Same order: `ORDER BY insert_time, block_id` within a batch, batches ascending, so each bucket's
  row sequence is identical.

Therefore emitted specs — including `block_ids_hash` and every cut point, under both `BlockOrder`
variants — are byte-identical, and every cached JIT partition still reports up to date. There is no
one-time regeneration cost.

### Config

`JitPartitionConfig` gains:

```rust
pub max_query_insert_time_range: TimeDelta,  // default TimeDelta::days(1)
```

and derives `Clone, Copy` (all fields are `Copy`) so the stream can own it. This is batching
granularity, not a data cap — nothing is dropped. If it is not a whole multiple of
`max_insert_time_slice`, round it up to one at construction of the window sequence, so batch edges
stay bucket-aligned.

### API change: return a stream of specs

```rust
pub async fn generate_process_jit_partitions(
    config: JitPartitionConfig,          // owned (Copy); was &JitPartitionConfig
    lakehouse: Arc<LakehouseContext>,
    blocks_view: Arc<BlocksView>,        // owned by the stream; was &BlocksView
    query_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: String,                  // owned; was &str
) -> Result<BoxStream<'static, Result<SourceDataBlocksInMemory>>>
```

Same change for `generate_stream_jit_partitions`. Pre-queries run before the stream is returned, so
setup errors still surface at the call site. Call sites change from `for part in all_partitions`
to `while let Some(part) = specs.try_next().await? { ... }`; each spec is materialized and
**dropped** before the next is pulled, so the query stream backpressures naturally while
`write_partition_from_blocks` / `update_partition` runs. Callers already build a fresh
`BlocksView::new()` per call — wrapping it in `Arc` is a one-line change at each site.

`net_spans_view` and `thread_spans_view` thread a `same_run_ranges: Vec<TimeRange>` accumulator
through their loop; that stays as-is (it accumulates ranges, not blocks, and is bounded by the
partition count).

### Keeping the segment functions

`generate_stream_jit_partitions_segment` (`:406`) and `generate_process_jit_partitions_segment`
(`:528`) are **kept, not deleted**: `rust/analytics/tests/thread_spans_ordering_db_test.rs` drives
`generate_stream_jit_partitions_segment` directly from ~10 sites (single query range, no bucket
subdivision, asserting on the returned specs). Deleting them would mean rewriting a 2000-line DB
test suite for no benefit.

To avoid duplicating the row-parsing logic between the segment functions and the new streaming
path, extract it into two helpers in `jit_partitions.rs`:

```rust
/// Column accessors for the stream-scoped blocks query, resolved once per RecordBatch.
struct StreamBlockRowReader<'a> { /* format column */ }
/// Column accessors for the process-scoped blocks query (rebuilds StreamMetadata per row),
/// resolved once per RecordBatch instead of once per row as today.
struct ProcessBlockRowReader<'a> { /* stream_id, process_id, streams.* columns */ }
```

each exposing `fn read_row(&self, rb: &RecordBatch, row: usize, ...) -> Result<Arc<PartitionSourceBlock>>`.
The segment functions keep their current `collect()`-based bodies but call the helpers; the
streaming path calls them from inside the `execute_stream()` loop. Hoisting the process-variant
accessors out of the per-row loop (`:577-586` today) is a free win on the side.

### Instrumentation

Keep `instrument_named!` spans; replace the per-segment `collect_partition_blocks` span (`:568`)
with one per batch (`stream_partition_blocks`) so the before/after is visible in traces.

## Implementation Steps

1. `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Add `max_query_insert_time_range: TimeDelta::days(1)` to `JitPartitionConfig`; derive
     `Clone, Copy`.
   - Extract `StreamBlockRowReader` / `ProcessBlockRowReader` and route the existing segment
     functions through them (behavior unchanged).
   - Add a private `batch_windows(insert_time_range, slice, batch_width) -> impl Iterator<Item = TimeRange>`
     helper (slice-aligned, last window clamped to the range end).
   - Rewrite `generate_stream_jit_partitions` to return a `BoxStream`: keep the pre-queries, then
     `async_stream::try_stream!` over batch windows, consuming each via `df.execute_stream()`,
     flushing a bucket through `group_blocks_into_partitions` on bucket change and at the end.
   - Same for `generate_process_jit_partitions`.
2. Update the seven call sites to consume the stream (`futures::TryStreamExt::try_next` loop,
   `Arc::new(BlocksView::new()?)`, owned `stream_tag`): `log_view.rs`, `metrics_view.rs`,
   `net_spans_view.rs`, `images_view.rs`, `async_events_view.rs`, `otel/spans_view.rs`,
   `thread_spans_view.rs`.
3. Add `rust/analytics/tests/jit_batch_windows_tests.rs` — pure unit tests for `batch_windows` and
   the bucket-change flush (see Testing Strategy).
4. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.
5. Manual verification — see Testing Strategy.

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — main change.
- `rust/analytics/src/lakehouse/{log_view,metrics_view,net_spans_view,images_view,async_events_view,thread_spans_view}.rs`,
  `rust/analytics/src/lakehouse/otel/spans_view.rs` — consume the stream.
- `rust/analytics/tests/jit_batch_windows_tests.rs` — new.
- `rust/analytics/tests/thread_spans_ordering_db_test.rs` — unchanged (segment functions kept).

## Trade-offs

- **Batched queries vs. one full-range query.** A single query would need a global
  `ORDER BY insert_time, block_id`, and `SortExec` buffers its entire input — every block row of
  the range. Day-wide batches give ~24× fewer queries with a sort buffer bounded by one window. See
  *Follow-ups* for why eliding the sort is no longer blocked but still not free.
- **Batched queries vs. parallelizing the per-hour loop.** Concurrency (`buffered(8)`) hides
  latency but keeps `1 + N` queries, keeps re-reading partitions spanning several hours, holds up
  to 8 collected segments in RAM, and adds a tuning constant. Batching removes the redundant work
  instead of overlapping it.
- **Streaming API vs. returning a Vec.** The Vec is the current unbounded-memory hazard; all
  callers already process specs one at a time, so a stream matches their shape. Cost: a signature
  change across seven views and owned (`Arc`/cloned) captures.
- **Reusing `group_blocks_into_partitions` per bucket vs. a streaming accumulator.** A per-block
  accumulator cannot express `BlockOrder::EventTime`, which stable-sorts a whole segment by event
  time and computes a suffix-minimum over the full list to find insert-safe cut points. Buffering
  one bucket costs the same memory the old per-hour `collect()` already paid, and buys byte-identical
  output.
- **Keeping the MIN/MAX pre-query.** Insert-time bounds are needed *before* the blocks queries
  (they scope the Postgres partition fetch and the batch windows), and an event-time filter on the
  blocks query would break the "JIT partition covers its whole hour" invariant.
  `1 + ceil(hours/24)` queries is the floor for this design.
- **Not widening `max_insert_time_slice`.** Fewer/wider buckets would also cut query count but
  changes partition bucketing, invalidating every cached JIT partition. Out of scope.
- **Sequential batches vs. prefetching the next batch.** Prefetch (`buffered(2)`) would overlap one
  batch's materialization with the next query, at the cost of holding two batches' rows. Start
  sequential; the batch width already amortizes per-query overhead. Easy follow-up if traces still
  show query/materialization ping-pong.

## Superseded Design

The earlier version of this plan proposed a streaming `JitPartitionAccumulator` with a tie-atomic
soft cap, replacing the inline chunking that existed at the time. Do not resurrect it:

- #1429/#1440 already replaced that inline chunking with `group_blocks_into_partitions`
  (`jit_partitions.rs:227`), which handles the determinism concern and adds `BlockOrder::EventTime`
  for `thread_spans`/`net_spans`.
- Under `EventTime`, grouping stable-sorts a whole segment by `(begin_ticks, end_ticks)` and uses a
  suffix-minimum over the entire list to pick insert-safe cut points, so cut decisions cannot be
  made from a per-block streaming `push()`. The accumulator design is structurally incompatible.
- Its accepted one-time cache-regeneration cost (specs whose old cut fell mid-tie) is no longer
  worth paying, since this plan achieves the same performance with byte-identical output.

## Follow-ups (not in scope)

**Sort elision.** The original blocker — merged blocks partitions being written unsorted — is gone:
#1336 shipped as #1340. `BlocksView` now carries an `ordered_merger`
(`SELECT * FROM source ORDER BY insert_time` + `ScanOrdering::Concatenated{insert_time}`,
`blocks_view.rs:74-88`) and records `sort_order = ["insert_time"]` on the partitions it produces
(`blocks_view.rs:143`). `make_partitioned_execution_plan` supports `ScanOrdering::PerFile { columns }`,
gated on every non-empty partition's `certifies_sort_order` and degrading silently to `Unordered`
otherwise (`partitioned_execution_plan.rs:266-275`) — the safe way to declare an ordering over a
partition set that may still contain pre-#1340 files.

Adding a `with_scan_ordering` variant of `query_partitions` and declaring
`PerFile { ["insert_time"] }` on the JIT blocks query would let `SortPreservingMergeExec` (bounded,
k-way) replace the per-batch `SortExec`, after which the batch width becomes a pure tuning knob.
**But it is not a drop-in:** the recorded guarantee is `insert_time` only, so a k-way merge orders
equal-`insert_time` rows by file arrival, not by `block_id`. That changes tie order inside a bucket,
which under `BlockOrder::InsertTime` moves greedy cut points and invalidates cached specs. Making
that safe needs either a `(insert_time, block_id)` guarantee end-to-end (the merge SQL currently
sorts on `insert_time` alone) or a tie-atomic cut rule in `group_blocks_into_partitions`. Given
day-wide batching already reduces a week-long range to 7 queries, the marginal win is small —
file it, don't bundle it.

**Open an issue** for traceability before implementation (repo convention links plans to issues),
and revisit the `TimeDelta::days(1)` default under a real workload; it bounds the sort buffer to one
day of one process's block rows, with no load testing yet.

## Testing Strategy

- **Unit tests** (`rust/analytics/tests/jit_batch_windows_tests.rs`, pure logic — no DB or object
  store):
  - `batch_windows` tiles a slice-aligned range with no gaps or overlaps, edges land on bucket
    boundaries, and the last window ends exactly at the range end;
  - a batch width shorter than / not a multiple of the slice is rounded up to whole buckets;
  - a single-bucket range yields one window.
- **Existing suites**: `cargo test` from `rust/` must pass unchanged — notably
  `thread_spans_ordering_db_test.rs`, which pins `BlockOrder::EventTime` cut-point behavior through
  the retained segment functions.
- **Manual cache-stability check** (the load-bearing regression test): start services
  (`python3 local_test_env/ai_scripts/start_services.py` or monolith), run a multi-day JIT query
  (e.g. a process log view via `micromegas-query`) against the **old** binary, then the **new** one.
  Because the design is output-identical, the new binary must log `partition up to date` for
  *every* partition — any regeneration is a bug, not an accepted cost. Repeat for a
  `BlockOrder::EventTime` view (`thread_spans` / `net_spans`).
- **Performance check**: in the trace, per-hour `collect_partition_blocks` chains are replaced by
  ~1 `stream_partition_blocks` per day, and end-to-end latency for the multi-day query drops.
- **Memory check**: long-range query with RSS gauges (system_monitor, #1330) flat — no growth
  proportional to range length.

## Documentation

No mkdocs page covers JIT partition internals (verified by grep). Rustdoc on the new config field,
the batch-window helper, the row-reader helpers, and the rewritten functions suffices.
