# Faster JIT Partition Generation: Batched Queries + Streaming Specs Plan

## Overview

JIT (just-in-time) lakehouse views resolve their source blocks in
`rust/analytics/src/lakehouse/jit_partitions.rs` by slicing the insert-time range into
1-hour segments and running **one DataFusion query per segment, sequentially** — a trace
showed ~256ms per `collect_partition_blocks` span, back-to-back, ~15s for one query. On
top of that, the functions return a `Vec<SourceDataBlocksInMemory>` holding **every block
of the whole range in RAM at once**.

This plan makes two coupled changes:

1. **Fewer queries**: query blocks in *batches of many hour-buckets* (default 1 day = 24
   buckets per query) instead of one query per hour, reproducing the hour-segmentation
   client-side from each row's `insert_time`. A week-long range goes from 168 sequential
   queries to 7, and source partitions overlapping several hours are re-read once per
   batch instead of once per hour.
2. **Bounded memory**: the generators return a **stream of partition specs** instead of a
   `Vec`, and consume each batch query via `execute_stream()`. Nothing ever holds the full
   range's block list: peak memory is one batch's sorted rows (bounded by the batch width)
   plus the one partition spec currently being accumulated/materialized.

A single full-range query was considered and rejected — see Trade-offs: its global
`ORDER BY` would buffer every block row of the range inside DataFusion's sort, which is
exactly the unbounded-memory behavior we must avoid, and sort elision via declared scan
ordering is unsound today (see Current State).

This plan supersedes `tasks/jit_partition_segment_concurrency_plan.md` (bounded-concurrency
version of the per-hour loop, never implemented) — delete that file when this lands.

## Current State

### Call graph

Seven views call into this file, all with `JitPartitionConfig::default()`
(`max_nb_objects: 20Mi`, `max_insert_time_slice: 1h`):

- `generate_process_jit_partitions` (`jit_partitions.rs:392`) — used by `log_view.rs:170`,
  `metrics_view.rs:174`, `net_spans_view.rs:315`, `images_view.rs:129`,
  `async_events_view.rs:155`, `otel/spans_view.rs:133`. Callers loop over the returned vec:
  `is_jit_partition_up_to_date` → `write_partition_from_blocks`, one spec at a time.
- `generate_stream_jit_partitions` (`jit_partitions.rs:192`) — used by
  `thread_spans_view.rs:303`; caller loops `update_partition` per spec.

Both generators have the same three-step shape:

1. **MIN/MAX pre-query**: find the insert-time range of blocks whose *event* time overlaps
   the query range (`generate_process_jit_partitions` inlines it at `:418-440`; the stream
   variant uses `get_insert_time_range`, `:49-99`). The result is truncated to slice
   boundaries: `[trunc(min), trunc(max) + slice)`.
2. **Postgres fetch**: `PartitionCache::fetch_overlapping_insert_range_for_view` loads the
   blocks-view partition list for that whole insert range (`:220-229`, `:461-470`) — runs
   once, already hoisted out of the loop by #1335.
3. **Sequential per-hour loop** (`:231-250`, `:472-492`): for each 1h window, call
   `generate_*_jit_partitions_segment`, which filters the partition cache
   (`filter_insert_range`, overlap semantics — `partition_cache.rs:234`), builds SQL scoped
   to `insert_time >= begin AND insert_time < end ORDER BY insert_time, block_id`, runs
   `query_partitions(...).collect()`, and chunks rows into `SourceDataBlocksInMemory`
   capped at `max_nb_objects`. All results accumulate into one output vec.

Note the segment queries filter by **insert_time only** (not event time): a JIT partition
must cover its whole hour bucket regardless of the triggering query's event range, so
cached partitions are query-independent. Any redesign must preserve that.

### What's wasteful / dangerous

- **Per-query overhead × N segments, serialized.** Each hour pays `SessionContext` setup,
  object-store round trips, and Parquet decode (~256ms observed) even for sparse hours.
- **Redundant re-reads.** `filter_insert_range` keeps any blocks-view partition
  *overlapping* the hour; a merged daily partition is scanned by all 24 hour-queries.
- **Unbounded output.** The full range's `Vec<SourceDataBlocksInMemory>` (block metadata +
  per-block `Arc<StreamMetadata>`) sits in RAM while partitions are materialized one by
  one. For a long-lived chatty process this list alone can be huge. This is a pre-existing
  problem the new API must fix, not preserve.

### The segmentation invariant that must be preserved

`is_jit_partition_up_to_date` (`jit_partitions.rs:498`) matches each spec against
`lakehouse_partitions` by its blocks' `[min_insert_time, max_insert_time]`. Cache reuse
therefore depends on generated specs having **exactly the same bucketing** as previous
runs: specs never span an hour boundary, buckets align via
`duration_trunc(max_insert_time_slice)`, and blocks are ordered by `(insert_time,
block_id)` (`get_part_insert_time_range`, `:591-601`, reads `blocks[0]`/`blocks[last]`).

Segmentation is a **pure function of each row's `insert_time`** — it does not require one
query per bucket. `row_bucket = insert_time.duration_trunc(slice)` reproduces today's
`[begin, end)` windows exactly, because the windows are themselves `duration_trunc`-aligned.

### Why sort elision (and therefore one giant query) is off the table for now

The client-side grouping needs rows sorted by `(insert_time, block_id)`. A single
full-range query would put that `ORDER BY` over the whole range; DataFusion's `SortExec`
buffers its entire input, i.e. every block row of the range — unacceptable.

The existing declared-scan-ordering infrastructure (`make_partitioned_execution_plan`'s
`output_ordering`, `partitioned_execution_plan.rs:107-179`) could elide the sort, and
freshly materialized blocks-view partitions *are* written sorted
(`ORDER BY blocks.insert_time, blocks.block_id`, `blocks_view.rs:46`). But **merged**
partitions are not: the default `View::merge_partitions` merge query is
`SELECT * FROM source;` with no `ORDER BY` (`view.rs:108`) and an undeclared scan order,
so daily merged blocks partitions cannot be trusted to be internally sorted. Declaring the
ordering would silently mis-group rows for existing data. Fixing merge ordering +
retiring/aging out existing merged partitions is a possible follow-up (see Open
Questions), after which the batch width could grow arbitrarily; it is not part of this
change.

### Existing streaming precedent

`SourceDataBlocks::get_blocks_stream` (`partition_source_data.rs:121`) — the global-view
analog — already consumes a blocks query via `df.execute_stream()` inside
`async_stream::try_stream!`, hoisting column accessors once per batch. The rewrite follows
that pattern. `query_partitions` (`query.rs:80-91`) returns a `DataFrame` precisely to
leave streaming open.

## Design

### New shape (both variants)

```
pre-queries (unchanged, run before the stream is returned):
  1. MIN/MAX insert-time query        -> [trunc(min), trunc(max) + slice)
  2. fetch_overlapping_insert_range_for_view -> PartitionCache

returned stream (async_stream::try_stream!):
  for each batch window (max_query_insert_time_range wide, slice-aligned):
      filter cache to window, build SQL: insert_time in [batch_begin, batch_end)
                                         ORDER BY insert_time, block_id
      df.execute_stream()
      for each RecordBatch, for each row:
          accumulator.push(block)  -> may yield a completed SourceDataBlocksInMemory
  accumulator.finish()             -> may yield the tail spec
```

- Batch windows are consecutive runs of hour-buckets, so window edges coincide with bucket
  edges; the accumulator's bucket-change flush makes batch boundaries need no special
  handling. Batches execute sequentially in ascending time order, so concatenated rows are
  globally ordered exactly like today's per-hour concatenation.
- `JitPartitionConfig` gains `max_query_insert_time_range: TimeDelta` (default
  `TimeDelta::days(1)`, i.e. 24 buckets per query). This is batching granularity, not a
  data cap — nothing is dropped; it only bounds the per-query sort buffer.
- Per-batch memory: `SortExec` buffers only the *post-filter* rows (this process/stream's
  blocks) for one window. Today's code holds one hour's rows *plus the entire output vec*;
  the new code holds one day's rows and no output vec — bounded regardless of range length.

### API change: return a stream of specs

```rust
pub async fn generate_process_jit_partitions(
    config: JitPartitionConfig,          // owned (derive Clone); was &JitPartitionConfig
    lakehouse: Arc<LakehouseContext>,
    blocks_view: Arc<BlocksView>,        // owned by the stream; was &BlocksView
    query_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: String,                  // owned; was &str
) -> Result<BoxStream<'static, Result<SourceDataBlocksInMemory>>>
```

(same change for `generate_stream_jit_partitions`). Pre-queries run before the stream is
returned, so setup errors surface at the call. The seven call sites change from
`for part in all_partitions { ... }` to `while let Some(part) = specs.try_next().await? { ... }`
— each spec is materialized and **dropped** before the next is pulled, so the query stream
naturally backpressures while `write_partition_from_blocks` runs. Callers already
construct a fresh `BlocksView::new()` per call; wrapping it in `Arc` is a one-line change
at each site.

### The segmenting accumulator

Extract the duplicated chunking logic (currently `:150-184` and `:350-384`) into one
struct in `jit_partitions.rs`, yielding specs incrementally instead of collecting them:

```rust
pub struct JitPartitionAccumulator {
    max_nb_objects: i64,
    slice: TimeDelta,
    current_bucket: Option<DateTime<Utc>>,   // insert_time.duration_trunc(slice)
    blocks: Vec<Arc<PartitionSourceBlock>>,
    nb_objects: i64,
}

impl JitPartitionAccumulator {
    /// Returns a completed spec when `block` starts a new one.
    pub fn push(&mut self, block: Arc<PartitionSourceBlock>)
        -> Result<Option<SourceDataBlocksInMemory>>;
    /// Flushes the in-progress spec, if any.
    pub fn finish(self) -> Option<SourceDataBlocksInMemory>;
}
```

`push` flushes the in-progress spec when **either**:
- the block's bucket differs from `current_bucket` (replaces today's per-segment-call
  boundary — keeps specs from spanning hour boundaries), or
- `nb_objects + block_nb_objects > max_nb_objects` and the current spec is non-empty
  (identical to today's cap logic).

A flushed spec carries `block_ids_hash: nb_objects.to_le_bytes().to_vec()` — byte-for-byte
today's value (`hash_to_object_count` reads it as a count). Inputs must arrive sorted by
`insert_time`; the per-batch `ORDER BY` plus sequential batch order guarantees it.

**Equivalence argument** (keep in the PR description): today a fresh accumulator per hour
segment resets specs at aligned hour boundaries; the bucket-change flush reproduces the
reset at exactly the same boundaries (`duration_trunc` alignment; a block exactly on a
boundary starts the next bucket in both designs). Empty hours yield no rows, hence no
specs. Per-batch sort concatenated in batch order equals today's per-hour sort
concatenated in hour order. Output specs are identical, so `is_jit_partition_up_to_date`
keeps hitting partitions cached by the old code.

### Row parsing

- **Stream variant**: the per-row parsing at `:143-177` moves into the stream-consumption
  loop unchanged (block + format; stream/process metadata already known).
- **Process variant**: the per-row `StreamMetadata` reconstruction at `:301-348` moves into
  the loop; hoist the column accessors to once per `RecordBatch` (as
  `partition_source_data.rs:127-160` does) instead of once per row as today.

The two `generate_*_jit_partitions_segment` functions (pub, but no callers outside this
file — verified by workspace grep) are folded into their parents and deleted.

Keep `instrument_named!` spans; replace the `collect_partition_blocks` span with one per
batch (e.g. `stream_partition_blocks`) so the before/after is visible in traces.

## Implementation Steps

1. `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Add `max_query_insert_time_range: TimeDelta::days(1)` to `JitPartitionConfig`;
     derive/impl `Clone`.
   - Add `JitPartitionAccumulator` with doc comments.
   - Rewrite `generate_stream_jit_partitions`: keep pre-queries, then return an
     `async_stream::try_stream!`-based `BoxStream` iterating batch windows; inline the row
     parsing from `generate_stream_jit_partitions_segment`; delete that function.
   - Rewrite `generate_process_jit_partitions` the same way (hoist per-batch column
     accessors); delete `generate_process_jit_partitions_segment`.
2. Update call sites to consume the stream (`futures::TryStreamExt::try_next` loop,
   `Arc::new(BlocksView::new()?)`): `log_view.rs`, `metrics_view.rs`, `net_spans_view.rs`,
   `images_view.rs`, `async_events_view.rs`, `otel/spans_view.rs`, `thread_spans_view.rs`.
3. Add accumulator unit tests in `rust/analytics/tests/jit_partition_accumulator_tests.rs`.
4. Delete `tasks/jit_partition_segment_concurrency_plan.md` (superseded).
5. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.
6. Manual verification: start services
   (`python3 local_test_env/ai_scripts/start_services.py` or monolith), run a
   multi-day JIT query (e.g. a process log view via `micromegas-query`) **twice** — the
   second run must log `partition up to date` for every partition (proves bucketing
   unchanged and caching still hits). Ideally run once with the old binary first, then the
   new one, to prove compatibility with partitions bucketed by the old code. Confirm in
   the trace that per-hour `collect_partition_blocks` chains are replaced by ~1 query per
   day and latency drops; watch process RSS (system_monitor gauges from #1330) on a
   long-range query to confirm memory stays flat.

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — main change.
- `rust/analytics/src/lakehouse/{log_view,metrics_view,net_spans_view,images_view,async_events_view,thread_spans_view}.rs`,
  `rust/analytics/src/lakehouse/otel/spans_view.rs` — consume the stream.
- `rust/analytics/tests/jit_partition_accumulator_tests.rs` — new.
- `tasks/jit_partition_segment_concurrency_plan.md` — delete (superseded).

## Trade-offs

- **Batched queries vs. one full-range query.** One query minimizes query count but its
  global `ORDER BY` buffers the whole range's rows in `SortExec` — unbounded memory. Sort
  elision via declared scan ordering is unsound while merged blocks partitions are written
  unsorted (`view.rs:108`). Day-wide batches get ~24× fewer queries with a sort buffer
  bounded by one window; when merge ordering is fixed later, growing/removing the batch
  width becomes a config change, not a redesign.
- **Batched queries vs. parallelizing the per-hour loop** (the superseded plan):
  concurrency (`buffered(8)`) hides latency but keeps `1 + N` queries, keeps re-reading
  partitions that span several hours, holds up to 8 collected segments in RAM, and adds a
  tuning constant. Batching removes the redundant work instead of overlapping it.
- **Streaming API vs. returning a Vec.** The Vec is the current unbounded-memory hazard;
  all callers already process specs one at a time, so a stream matches their shape.
  Cost: a signature change across seven views and owned (`Arc`/cloned) captures.
- **Keeping the MIN/MAX pre-query.** Insert-time bounds are needed *before* the blocks
  queries (they scope the Postgres partition fetch and the batch windows), and an
  event-time filter on the blocks query would break the "JIT partition covers its whole
  hour" invariant. `1 + ceil(hours/24)` queries is the floor for this design.
- **Not widening `max_insert_time_slice`.** Fewer/wider buckets would also cut query count
  but changes partition bucketing, invalidating every cached JIT partition. Out of scope.
- **Sequential batches vs. prefetching the next batch.** Prefetch (`buffered(2)`) would
  overlap one batch's materialization with the next query, at the cost of holding two
  batches' rows. Start sequential; the batch width already amortizes per-query overhead.
  Easy follow-up if traces still show query/materialization ping-pong.

## Documentation

No mkdocs page covers JIT partition internals (verified by grep). Rustdoc on the new
accumulator, the config field, and the rewritten functions suffices.

## Testing Strategy

- **Accumulator unit tests** (pure logic, no DB/object store — fits the crate's test
  style): synthetic block sequences covering:
  - blocks within one bucket under the cap → one spec on bucket change/finish;
  - bucket change → flush at the aligned boundary (block exactly on a boundary starts the
    new bucket);
  - `max_nb_objects` overflow inside a bucket → split, matching today's "push without this
    block" semantics;
  - single oversized block → its own spec;
  - `block_ids_hash` = LE bytes of the spec's object count;
  - empty input → `finish()` yields `None`.
- **Existing suite**: `cargo test` from `rust/` must pass unchanged.
- **Manual cache-stability check** (the critical regression risk): Implementation Step 6 —
  second run must reuse all partitions from the first; old-binary→new-binary check proves
  bucket compatibility.
- **Memory check**: long-range query with RSS gauges flat (no growth proportional to range
  length).

## Open Questions

- Open a GitHub issue for traceability before implementation (repo convention links plans
  to issues).
- Follow-up filed as [#1336](https://github.com/madesroches/micromegas/issues/1336): make
  blocks-view merges order-preserving (merge query `ORDER BY insert_time, block_id`, or
  declare scan ordering on the merge source so the sort is elided) and, once existing
  merged partitions have been retired or aged out, declare `(insert_time, block_id)` scan
  ordering on the JIT blocks query — the per-batch sort disappears and the batch width can
  grow or go away entirely.
- Is `TimeDelta::days(1)` the right default batch width? It bounds the sort buffer to one
  day of one process's block rows; no load testing yet. Worth a sanity check under a real
  workload before tuning.
