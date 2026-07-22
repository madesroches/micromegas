# Optimize `generate_process_jit_partitions` Plan

## Overview

`generate_process_jit_partitions` (`rust/analytics/src/lakehouse/jit_partitions.rs:391-480`)
is the JIT-partition generator used by every process-scoped view —
`log_view`, `metrics_view`, `async_events_view`, `images_view`, `net_spans_view`,
and `otel/spans_view` — each calling it once per `jit_update`. Its per-segment
helper, `generate_process_jit_partitions_segment`, re-fetches the exact same
`blocks` partition listing from Postgres on every iteration of a sequential,
un-parallelized loop, instead of fetching it once and reusing it. This wastes
Postgres round-trips and serializes work that has no dependency between
iterations. The fix hoists the partition fetch out of the loop and runs the
now-independent per-segment work concurrently, cutting both Postgres load and
wall-clock time for processes whose queried insert-time span covers many
`max_insert_time_slice` (1 hour) segments.

This is a narrower, complementary fix to the redundant-unbounded-scan problem
already described in `tasks/backlog/jit_update_metadata_lookup.md` (which
covers `find_stream_from_view`/`find_process_with_latest_timing` against the
global `streams`/`processes` views). That problem is out of scope here; this
plan only touches `generate_process_jit_partitions` and its segment helper.

## Current State

`generate_process_jit_partitions` (`jit_partitions.rs:391-480`):
1. Fetches `blocks_view` partitions overlapping `query_time_range` (event-time
   bounded, via `LivePartitionProvider::fetch`) once, purely to compute the
   `MIN(insert_time)`/`MAX(insert_time)` of this process's matching blocks
   (lines 400-450).
2. Truncates/pads that observed range to `config.max_insert_time_slice`
   (1 hour, `JitPartitionConfig::default`, line 44) boundaries, producing
   `insert_time_range` (lines 452-458).
3. Loops **sequentially** over consecutive 1-hour segments from
   `insert_time_range.begin` to `.end` (lines 460-478), `.await`-ing
   `generate_process_jit_partitions_segment` for each one and appending its
   result.

`generate_process_jit_partitions_segment` (lines 247-385), called once per
segment:
- Line 255-264: calls `PartitionCache::fetch_overlapping_insert_range_for_view(
  &lakehouse.lake().db_pool, blocks_view.get_view_set_name(),
  blocks_view.get_view_instance_id(), *insert_time_range)` — **a fresh Postgres
  round-trip scoped to this one 1-hour segment.**
- Builds a `source`-backed DataFusion query filtered by `process_id` and
  `stream_tag` over that segment's partitions (lines 265-293), collects rows,
  and builds `SourceDataBlocksInMemory` groups from them (lines 296-383).

### The problem

Because `blocks_view` itself is merged into at-most-1-day partitions
(`blocks_view.rs:140-146`: `MergeExisting(_) => TimeDelta::days(1)`), the set
of partitions overlapping any given 1-hour segment is, in the common case,
**the same one or two day-partitions** that also overlap every other segment
within that day. A query spanning a multi-day or multi-week window for one
process turns into dozens of sequential, near-identical
`fetch_overlapping_insert_range_for_view` calls, each returning largely the
same rows, plus dozens of sequential `query_partitions`/`collect()` round
trips to object storage — none of which have any data dependency on each
other. Total wall-clock is the *sum* of every segment's latency, not the max.

`PartitionCache` already has `.filter_insert_range(range)`
(`partition_cache.rs:234-247`), an in-memory, non-async filter — the tool
needed to eliminate the redundant fetches without changing overlap semantics.

## Design

### 1. Hoist the partition fetch out of the per-segment loop

Fetch `blocks_view`'s partitions **once**, for the *entire* `insert_time_range`
computed in `generate_process_jit_partitions`, then pass that `PartitionCache`
by reference into every segment call. Each segment replaces its own Postgres
round-trip with a local `.filter_insert_range(*insert_time_range)` call —
semantically identical (overlap of a sub-range is always a subset of overlap
of the enclosing range), zero extra network calls.

`generate_process_jit_partitions_segment` signature changes from fetching
internally to accepting the pre-fetched cache:

```rust
pub async fn generate_process_jit_partitions_segment(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,      // was: fetched from `pool` inside this fn
    insert_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: &str,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let partitions = partitions.filter_insert_range(*insert_time_range).partitions;
    // ...unchanged from here down...
}
```

`generate_process_jit_partitions` fetches once, before the loop:

```rust
let segment_source_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
    &lakehouse.lake().db_pool,
    blocks_view.get_view_set_name(),
    blocks_view.get_view_instance_id(),
    insert_time_range,
)
.await?;
```

Note: this is a *different* fetch from the one at the top of the function
that determines `insert_time_range` in the first place (that one is bound by
`query_time_range` using event-time columns via `LivePartitionProvider`; this
one is bound by the already-computed `insert_time_range` using insert-time
columns via `PartitionCache`). They use different overlap semantics
(event-time vs. insert-time) and are not safe to conflate — see Trade-offs.

### 2. Run the now-independent segments concurrently

With the shared fetch removed, nothing about one segment's work depends on
another's — they can run concurrently instead of via a sequential `while`
loop. Use the same bounded, **order-preserving** (`buffered`, not
`buffer_unordered`) concurrency pattern already used for per-stream queries in
`process_spans_table_function.rs:274-306`:

```rust
let max_concurrent = std::thread::available_parallelism()
    .map(|n| n.get())
    .unwrap_or(4);

let segment_ranges: Vec<TimeRange> = /* the same begin/end walk, collected instead of looped-over inline */;

let partitions: Vec<SourceDataBlocksInMemory> = stream::iter(segment_ranges)
    .map(|range| {
        let lakehouse = lakehouse.clone();
        let process = process.clone();
        async move {
            generate_process_jit_partitions_segment(
                config,
                lakehouse,
                blocks_view,
                &segment_source_partitions,
                &range,
                process,
                stream_tag,
            )
            .await
        }
    })
    .buffered(max_concurrent)
    .try_collect::<Vec<_>>()
    .await?
    .into_iter()
    .flatten()
    .collect();
```

`buffered` (not `buffer_unordered`) preserves the original chronological
ordering of segments in the output even though up to `max_concurrent` run at
once — required because callers build call trees / ordered spans assuming
blocks arrive in insert-time order.

## Implementation Steps

1. In `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Change `generate_process_jit_partitions_segment`'s signature to take
     `partitions: &PartitionCache` instead of fetching internally; replace its
     `PartitionCache::fetch_overlapping_insert_range_for_view(...)` call with
     `partitions.filter_insert_range(*insert_time_range).partitions`.
   - In `generate_process_jit_partitions`, after computing `insert_time_range`,
     add the single `PartitionCache::fetch_overlapping_insert_range_for_view`
     call covering the whole range.
   - Replace the sequential `while end_segment <= insert_time_range.end { ... }`
     loop with: build the list of segment `TimeRange`s, then run them through
     `futures::stream::iter(...).map(...).buffered(max_concurrent).try_collect()`
     as shown above, flattening the per-segment `Vec<SourceDataBlocksInMemory>`
     results in order.
   - Add `use futures::{StreamExt, TryStreamExt};` (or extend the existing
     `futures::stream` import) as needed.
2. Run `cargo fmt` from `rust/`.
3. Run `cargo clippy --workspace -- -D warnings` from `rust/`.
4. Run `cargo test` from `rust/`.
5. Manually verify against a real process with blocks spread over more than a
   few hours of insert time (e.g. a long-running process queried over a
   multi-day window) that a view depending on `generate_process_jit_partitions`
   (`log_view` is the simplest) still returns identical rows before/after, and
   that Postgres query counts for `lakehouse_partitions` drop accordingly
   (visible via `process_spans` tracing or `pg_stat_statements`).

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — only file changed;
  `generate_process_jit_partitions` and `generate_process_jit_partitions_segment`.
  No caller signatures change (`log_view.rs`, `metrics_view.rs`,
  `async_events_view.rs`, `images_view.rs`, `net_spans_view.rs`,
  `otel/spans_view.rs` all call `generate_process_jit_partitions` with its
  existing signature, which is unchanged).

## Trade-offs

- **Not merging the two partition fetches in `generate_process_jit_partitions`
  into one.** The first fetch (via `LivePartitionProvider`, bound by
  `query_time_range` on `min_event_time`/`max_event_time`) and the new one
  (via `PartitionCache`, bound by `insert_time_range` on
  `begin_insert_time`/`end_insert_time`) use different overlap predicates and
  can legitimately return different partition sets when insert time and event
  time diverge (e.g. late-arriving data). Conflating them risks silently
  dropping partitions that a segment needs. Two fetches instead of N+1 is
  still the overwhelming majority of the win; unifying them is not worth the
  correctness risk for this change.
- **`buffered` bounded by `available_parallelism()` vs. a fixed constant.**
  Matches the existing precedent in `process_spans_table_function.rs` rather
  than inventing a new tuning knob. Since `generate_process_jit_partitions` is
  itself already called concurrently across many streams/processes in a
  single query (the same fan-out pattern that motivates
  `jit_update_metadata_lookup.md`), an unbounded concurrency level here would
  compound that; bounding it locally keeps the added concurrency from making
  contention worse.
- **Scope**: `generate_stream_jit_partitions_segment`
  (`jit_partitions.rs:102-191`), used only by `thread_spans_view`, has the
  identical redundant-fetch-per-segment shape (line 110-116). Left out of
  this plan since the user asked specifically about the process path; the
  same fix pattern applies there and is a natural, low-risk follow-up.

## Testing Strategy

- `cargo test` from `rust/` — confirms compilation and that any existing
  behavioral tests covering process-scoped views (`log_view`, `metrics_view`,
  `async_events_view`) still pass unchanged, since output ordering and content
  are preserved by construction (`buffered` + flatten in segment order).
- No new unit test file needed: the change is a pure refactor of how
  partitions are fetched and scheduled, not a change in query logic or
  results. Existing integration tests exercising these views are the
  regression net.
- Manual check (per Implementation Steps #5) that Postgres round-trips to
  `lakehouse_partitions` for a single `generate_process_jit_partitions` call
  drop from ~1-per-hour-segment to 2 total.

## Open Questions

- Is `max_concurrent = available_parallelism()` an acceptable default here, or
  should process-scoped JIT generation use a smaller fixed cap (e.g. 4-8)
  given it's already invoked concurrently per-stream/per-process by the
  caller? Recommend starting with the existing precedent
  (`available_parallelism()`) and revisiting if profiling shows it needs
  tightening.
