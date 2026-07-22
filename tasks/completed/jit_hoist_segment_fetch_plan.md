# Hoist Per-Segment Partition Fetch (JIT) Plan

## Overview

`generate_process_jit_partitions` slices a process's insert-time span into
`max_insert_time_slice` (1 hour) segments and, for **each** segment, makes its
own `PartitionCache::fetch_overlapping_insert_range_for_view` round-trip to
Postgres before scanning. For a multi-day query that is dozens to hundreds of
near-identical fetches returning largely the same daily `blocks_view`
partitions. This plan hoists that fetch **out of the loop**: fetch the partition
list once for the whole insert-time range, then filter it in memory per segment
with `PartitionCache::filter_insert_range`. It is a behavior-preserving refactor
— identical output, no schema change, no new concurrency — and it leaves the
segment helper taking a `&PartitionCache`, which is the shape later JIT work will
build on. The per-stream path (`generate_stream_jit_partitions`, used by
`thread_spans_view`) has the identical shape and gets the same hoist (Design §3).

This is deliberately the *simple and safe* first step. It removes the redundant
Postgres round-trips only; it does **not** remove the Parquet scans (the
`MIN/MAX(insert_time)` range query and the per-segment `query_partitions` still
read the same files). Those are left for follow-up work.

## Current State

`generate_process_jit_partitions` (`rust/analytics/src/lakehouse/jit_partitions.rs:391-480`):
1. Fetches `blocks_view` partitions overlapping `query_time_range` (event-time
   bounded, via `LivePartitionProvider::fetch`) and runs a
   `MIN(insert_time)/MAX(insert_time)` query to compute `insert_time_range`
   (lines 399-458).
2. Loops **sequentially** over consecutive 1-hour segments from
   `insert_time_range.begin` to `.end` (lines 460-478), `.await`-ing
   `generate_process_jit_partitions_segment` for each.

`generate_process_jit_partitions_segment` (lines 247-385):
- Lines 255-264: `PartitionCache::fetch_overlapping_insert_range_for_view(
  &lakehouse.lake().db_pool, blocks_view.get_view_set_name(),
  blocks_view.get_view_instance_id(), *insert_time_range)` — **a fresh Postgres
  round-trip scoped to this one segment.**
- Lines 265-385: builds a `source`-backed `query_partitions` filtered by
  `process_id` + `stream_tag` over that segment's partitions, collects rows, and
  groups them into `SourceDataBlocksInMemory` by `max_nb_objects`.

Because `blocks_view` is merged into at-most-1-day partitions
(`blocks_view.rs` `MergeExisting(_) => TimeDelta::days(1)`), the partitions
overlapping any given 1-hour segment are, in the common case, the same one or
two day-partitions that overlap every other segment in that day. So the
per-segment fetch re-fetches the same rows repeatedly.

`PartitionCache` already exposes `filter_insert_range(range)`
(`partition_cache.rs`), an in-memory, non-async filter using the identical
insert-time overlap predicate (`begin < range.end && end > range.begin`) as
`fetch_overlapping_insert_range_for_view`. This is the tool needed to eliminate
the redundant fetches without changing overlap semantics.

The per-stream path (`generate_stream_jit_partitions` /
`generate_stream_jit_partitions_segment`, lines 196-243 / 102-191, used by
`thread_spans_view`) has the identical redundant-fetch-per-segment shape at
lines 110-116.

## Design

### 1. Segment helper accepts a pre-fetched cache

Change `generate_process_jit_partitions_segment` to take
`partitions: &PartitionCache` instead of fetching internally, and replace its
DB call with an in-memory filter:

```rust
pub async fn generate_process_jit_partitions_segment(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,      // was: fetched from db_pool inside this fn
    insert_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: &str,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let partitions = partitions.filter_insert_range(*insert_time_range).partitions;
    // ...unchanged from here down (query_partitions + grouping)...
}
```

### 2. Caller fetches once, before the loop

In `generate_process_jit_partitions`, after `insert_time_range` is computed
(line 458), fetch the partition list once for the whole range and pass it by
reference into every segment call:

```rust
let segment_source_partitions = instrument_named!(
    PartitionCache::fetch_overlapping_insert_range_for_view(
        &lakehouse.lake().db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        insert_time_range,
    ),
    "fetch_overlapping_insert_range_for_view"
)
.await?;

let mut begin_segment = insert_time_range.begin;
let mut end_segment = begin_segment + config.max_insert_time_slice;
let mut partitions = vec![];
while end_segment <= insert_time_range.end {
    let segment_range = TimeRange::new(begin_segment, end_segment);
    let mut segment_partitions = generate_process_jit_partitions_segment(
        config,
        lakehouse.clone(),
        blocks_view,
        &segment_source_partitions,
        &segment_range,
        process.clone(),
        stream_tag,
    )
    .await?;
    partitions.append(&mut segment_partitions);
    begin_segment = end_segment;
    end_segment = begin_segment + config.max_insert_time_slice;
}
```

The loop stays **sequential** — no concurrency change.

### 3. Same hoist on the per-stream path

`generate_stream_jit_partitions_segment` /  `generate_stream_jit_partitions`
(`jit_partitions.rs:102-243`, used by `thread_spans_view`) have the identical
shape: the segment helper fetches per-segment (lines 110-116) and the caller
loops over consecutive segments (lines 227-241). Apply the same transform:

- Change `generate_stream_jit_partitions_segment` to take
  `partitions: &PartitionCache` instead of calling
  `fetch_overlapping_insert_range_for_view` internally; its body's first two
  lines (`let cache = …fetch…; let partitions = cache.partitions;`) become
  `let partitions = partitions.filter_insert_range(*insert_time_range).partitions;`.
- In `generate_stream_jit_partitions`, hoist the single
  `fetch_overlapping_insert_range_for_view(insert_time_range)` (wrapped in
  `instrument_named!`) to just after `insert_time_range` is computed
  (line 223), and pass `&segment_source_partitions` into each segment call.

This path's caller — `generate_stream_jit_partitions` — is invoked only by
`thread_spans_view`, whose signature is unchanged. The behavior-preservation
argument below applies verbatim (same view scoping, same overlap predicate,
`segment_range ⊆ insert_time_range`).

### Why this is behavior-preserving

`segment_source_partitions` is fetched with
`fetch_overlapping_insert_range_for_view(insert_time_range)` — the same call the
segment helper used, but for the enclosing range. Since every `segment_range`
satisfies `segment_range ⊆ insert_time_range`,
`segment_source_partitions.filter_insert_range(segment_range)` returns exactly
the subset that `fetch_overlapping_insert_range_for_view(segment_range)` would
have returned (identical view scoping, identical overlap predicate). The set of
blocks read, the partition boundaries, and each partition's `source_data_hash`
are therefore unchanged, so nothing downstream — including
`is_jit_partition_up_to_date` — can observe a difference. This preserves the
JIT-partition determinism guarantee (issue #488): partition identity is a
function of insert-time segment content, which is untouched.

## Implementation Steps

1. In `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Change `generate_process_jit_partitions_segment`'s signature to take
     `partitions: &PartitionCache` (drop the internal
     `fetch_overlapping_insert_range_for_view` call); first line becomes
     `let partitions = partitions.filter_insert_range(*insert_time_range).partitions;`.
   - In `generate_process_jit_partitions`, add the single hoisted
     `fetch_overlapping_insert_range_for_view` call after `insert_time_range` is
     computed, and pass `&segment_source_partitions` into each segment call.
   - Keep the `instrument_named!` span on the hoisted fetch; the in-memory
     filter needs no instrumentation.
2. Apply the same hoist to the per-stream path (Design §3):
   - Change `generate_stream_jit_partitions_segment`'s signature to take
     `partitions: &PartitionCache`; replace its two-line internal fetch with
     `let partitions = partitions.filter_insert_range(*insert_time_range).partitions;`.
   - In `generate_stream_jit_partitions`, add the single hoisted
     `fetch_overlapping_insert_range_for_view` call (wrapped in
     `instrument_named!`) after `insert_time_range` is computed, and pass
     `&segment_source_partitions` into each segment call.
   Keep this as its own commit so the process-path and stream-path changes are
   independently reviewable/revertable, but land both in the same PR.
3. From `rust/`: `cargo fmt`, then `cargo clippy --workspace -- -D warnings`.
4. From `rust/`: `cargo test` (compile/unit sanity only — see Testing Strategy
   for the actual regression net covering the JIT paths).
5. Manual check against a real process whose insert-time span covers many hours
   (e.g. `log_view` over a multi-day window): identical rows before/after, and
   the count of `fetch_overlapping_insert_range_for_view` spans per
   `generate_process_jit_partitions` drops from ~1-per-segment to 1.

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — only file changed.
  Process path: `generate_process_jit_partitions` and
  `generate_process_jit_partitions_segment`. Stream path:
  `generate_stream_jit_partitions` and `generate_stream_jit_partitions_segment`.
  No caller signatures change — the six process-path views (`log_view.rs`,
  `metrics_view.rs`, `async_events_view.rs`, `images_view.rs`,
  `net_spans_view.rs`, `otel/spans_view.rs`) call
  `generate_process_jit_partitions`, and `thread_spans_view` calls
  `generate_stream_jit_partitions`, each with its unchanged signature.

## Trade-offs

- **Scope kept minimal on purpose.** The larger wins discussed alongside this
  (eliminating the `MIN/MAX` scan via per-process partition-occupancy metadata;
  collapsing the segment loop into a single streaming insert-time fill;
  memoizing the `thread_spans` per-stream metadata fan-out) are deferred. This
  change is the zero-risk foundation and can ship independently; the prior
  backlog docs covering those directions are removed and will be regenerated if
  pursued.
- **Sequential loop retained.** Parallelizing the segments (`buffered`) was
  considered and left out — it adds concurrency on top of the already-concurrent
  per-stream fan-out and needs its own judgment, so it does not belong in a
  "simple and safe" change.
- **Per-segment clone.** `filter_insert_range` clones the matching subset per
  segment. This is in-memory and negligible next to the Postgres round-trip and
  Parquet scan it does not add.

## Testing Strategy

- `cargo test` from `rust/` — compile/unit sanity only. The rust tests that
  touch the named views (`log_tests.rs`, `metrics_test.rs`,
  `async_events_tests.rs`) are schema/parsing/builder unit tests; none call
  `generate_process_jit_partitions`, so this does not exercise the refactor.
- The real regression nets:
  - `rust/analytics/tests/thread_spans_ordering_db_test.rs` — the only test
    that exercises a JIT path (`ThreadSpansView::jit_update`, stream-path via
    `generate_stream_jit_partitions`). It is `#[ignore]`d and requires a live
    `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`; run it
    explicitly with `cargo test -- --ignored` after starting local services.
  - Python integration tests (`poetry run pytest` from
    `python/micromegas/`, e.g. `test_log.py`) against locally started services
    — exercises the process path end-to-end through `log_view`.
- Manual verification per Implementation Step 5 that Postgres round-trips to
  `lakehouse_partitions` for a single `generate_process_jit_partitions` call
  drop to one. The same drop is expected on the stream path
  (`generate_stream_jit_partitions` via `thread_spans_view`). This is the
  primary process-path verification, since no automated test covers it under
  a plain `cargo test`.
