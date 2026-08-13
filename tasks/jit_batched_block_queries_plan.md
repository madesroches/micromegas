# Faster JIT Partition Generation: Adaptive Batched Block Queries + Streaming Specs

Issue: [#1474](https://github.com/madesroches/micromegas/issues/1474)

## Overview

The driving workload is **sparse view instances queried over long ranges** — for example OTLP
metrics from a single process, where the protocol's inefficiency means a month of data is a few
tens of thousands of tiny blocks. JIT (just-in-time) lakehouse views resolve their source blocks in
`rust/analytics/src/lakehouse/jit_partitions.rs` by slicing the insert-time range into 1-hour
segments and running **one DataFusion query per segment, sequentially** — ~256ms per
`collect_partition_blocks` span, back-to-back, regardless of how little data the hour holds. A
30-day range costs 720 queries and ~3 minutes of pure per-query overhead to find almost nothing.
On top of that, the functions return a `Vec<SourceDataBlocksInMemory>` holding **every block of the
whole range in RAM at once**.

Three coupled changes:

1. **Query count scales with data, not with time** — replace the per-hour loop with batched
   queries whose width is derived from observed block density (a `COUNT(*)` piggybacked on the
   existing pre-query). A sparse instance collapses a month into one query; a dense one falls back
   to hour-scale batches. Hour bucketing is recovered client-side from each row's `insert_time`.
2. **Rows are narrow enough for a row-count target to mean something** — the process-scoped query
   stops projecting stream-level metadata onto every block row, fetching it once per stream
   instead. This is what makes (1)'s target a real memory bound rather than a guess.
3. **Bounded memory** — the generators return a **stream of partition specs** instead of a `Vec`,
   consuming each batch query via `execute_stream()`. Peak memory is one hour bucket's blocks plus
   the one spec currently being materialized.

**The grouping algorithm is not touched.** Each hour bucket's blocks are still handed to the
existing `group_blocks_into_partitions` (`jit_partitions.rs:227`) exactly as today, so emitted specs
are byte-identical and every cached JIT partition stays valid. All the risk in this area lives in
the bucketing/cut-point rules, and this plan changes neither.

This supersedes the earlier draft of this file (`tasks/jit_single_query_plan.md`, renamed), which
predated #1429/#1440 and proposed replacing the segmentation logic with a streaming
`JitPartitionAccumulator` — see *Superseded Design*.

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

1. **MIN/MAX pre-query** — the insert-time range of blocks whose *event* time overlaps the query
   range (`generate_process_jit_partitions` inlines it at `:658-702`; the stream variant uses
   `get_insert_time_range`, `:353-403`). Truncated to slice boundaries:
   `[trunc(min), trunc(max) + slice)`.
2. **Postgres fetch** — `PartitionCache::fetch_overlapping_insert_range_for_view` loads the
   blocks-view partition list for that whole insert range (`:493-502`, `:704-713`) — already
   hoisted out of the loop by #1335.
3. **Sequential per-hour loop** (`:504-523`, `:715-735`) — per 1h window, call
   `generate_*_jit_partitions_segment`: filter the partition cache (`filter_insert_range`,
   `partition_cache.rs:248`), build SQL scoped to
   `insert_time >= begin AND insert_time < end ORDER BY insert_time, block_id`, run
   `query_partitions(...).collect()`, parse rows into `PartitionSourceBlock`s, call
   `group_blocks_into_partitions`. All results accumulate into one output vec.

The segment queries filter by **insert_time only** (not event time): a JIT partition must cover its
whole hour bucket regardless of the triggering query's event range, so cached partitions are
query-independent. That must be preserved.

### What's wasteful

- **Per-query overhead × N segments, serialized.** Each hour pays `SessionContext` setup, object
  store round trips, and Parquet decode (~256ms observed) even for an empty hour. This is the
  dominant cost for the sparse workload, where most of those queries find nothing.
- **Redundant re-reads.** `filter_insert_range` keeps any blocks-view partition *overlapping* the
  hour, so a merged daily partition is scanned by all 24 hour-queries.
- **Unbounded output.** The full range's `Vec<SourceDataBlocksInMemory>` sits in RAM while
  partitions are materialized one at a time. Pre-existing problem the new API fixes.
- **Stream metadata duplicated onto every block row** (process variant only). The query projects
  `streams.dependencies_metadata`, `streams.objects_metadata`, `streams.tags`,
  `streams.properties`, `streams.format` — stream-level columns the blocks-view join repeats per
  block — and then rebuilds a fresh `Arc<StreamMetadata>` **per row** (`:577-616`), CBOR-decoding
  both blobs each time. Measured payload for the in-repo Rust stream types:

  | stream | `objects_metadata` | `dependencies_metadata` | total per row |
  |---|---|---|---|
  | `log` | 842 B | 912 B | 1.75 KB |
  | `metrics` | 1,138 B | 908 B | 2.05 KB |
  | `cpu` (thread) | 2,584 B | 908 B | 3.49 KB |

  against ~150 B of actual block columns — 12–23× the useful payload, sorted and buffered on every
  row, decoded once per block instead of once per stream. OTLP-ingested streams are exempt: they
  store the 1-byte empty-CBOR sentinel for both blobs and empty properties
  (`web_ingestion_service.rs:19-32`, `:310-322`), so the motivating workload never sees this. It
  bites the dense case — see *Why the lean projection is in scope*.

### What this plan does not fix

For a sparse instance the remaining per-query cost after this change is **per-spec, not per-hour**:
30 days of OTLP metrics at a 60s push interval yields ~720 non-empty hour buckets → ~720 tiny
partitions, each costing one `is_jit_partition_up_to_date` Postgres round trip on every query (and
one parquet file on first materialization). Batching removes 720 block queries and leaves those 720
sequential round trips. See *Follow-ups*.

### Invariants that must be preserved

- **Bucketing must be reproducible across runs.** `is_jit_partition_up_to_date` (`:756`) matches
  each spec against `lakehouse_partitions` by its blocks' `[min_insert_time, max_insert_time]`, so
  cache reuse depends on a given block set producing the same specs it produced last run. Buckets
  are `duration_trunc(max_insert_time_slice)`-aligned and specs never span a bucket boundary;
  `row_bucket = insert_time.duration_trunc(slice)` reproduces today's `[begin, end)` windows exactly
  because those windows are themselves `duration_trunc`-aligned — so segmentation needs no
  query-per-bucket.
- **Tie order inside a bucket must be preserved.** Under `BlockOrder::InsertTime`,
  `group_blocks_into_partitions` cuts the list greedily in arrival order, so *which* equal-
  `insert_time` blocks land on either side of a cut depends on the SQL tie order. The batched query
  keeps `ORDER BY insert_time, block_id` verbatim. (This is also why the sort-elision follow-up is
  not free.)
- **Grouping stays per hour bucket, never per batch.** `View::get_scan_output_ordering`'s docs
  (`view.rs:186`) list "an insert-time inversion straddling a JIT *segment* boundary (segments are
  still grouped independently)" as a known, loudly-backstopped residual caveat. Widening the
  *query* window does not touch that; widening the *grouping* window would change which inversions
  `group_blocks_into_partitions` sees.

### Which knobs may vary between runs

This distinction is load-bearing and easy to get wrong:

- **Batch width is free to be adaptive.** It only decides how many buckets one SQL statement
  covers. Output is width-independent (batch edges are bucket-aligned, grouping is per bucket), so
  two runs picking different widths produce identical specs.
- **`max_insert_time_slice` must stay a stable constant.** Bucket boundaries are baked into every
  cached partition's recorded insert range. A density-derived slice would silently invalidate the
  entire JIT cache for a view instance whenever its volume changed. Changing it is a migration, not
  a tuning knob.

### Existing streaming precedent

`SourceDataBlocks::get_blocks_stream` (`partition_source_data.rs:121`) — the global-view analog —
already consumes a blocks query via `df.execute_stream()` inside `async_stream::try_stream!`,
hoisting column accessors once per batch. `query_partitions` (`query.rs:79-91`) returns a
`DataFrame` precisely to leave streaming open. `async-stream` and `futures` are already
dependencies of the analytics crate.

## Design

### New shape (both variants)

```
pre-queries (run before the stream is returned):
  1. MIN/MAX/COUNT insert-time query         -> [trunc(min), trunc(max) + slice), row estimate
  2. fetch_overlapping_insert_range_for_view -> PartitionCache
  3. per-stream metadata query (process variant only, see below)

batch width: derived from (1)'s count, slice-aligned  (see Adaptive batch width)

returned stream (async_stream::try_stream!):
  for each batch window:
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
              pending.push(block)   // stream metadata looked up, not rebuilt
  yield each spec from group_blocks_into_partitions(config, take(pending))   // tail bucket
```

Rows arrive ordered by `insert_time`, so a bucket is complete as soon as a later-bucket row appears:
`pending` never holds more than one hour bucket's blocks, independent of batch width. Batches run
sequentially in ascending time order, so the concatenated bucket sequence is exactly today's. Empty
buckets contribute no rows, hence no specs — same as today's empty per-hour query.

### Adaptive batch width

Pre-query 1 gains `COUNT(*)` — the same scan and the same predicate it already runs, so it is free:

```sql
SELECT MIN(insert_time) as min_insert_time,
       MAX(insert_time) as max_insert_time,
       COUNT(*)         as nb_blocks
FROM source WHERE <unchanged predicate>
```

Then:

```
total_buckets    = insert_range / slice
rows_per_bucket  = max(1, ceil(nb_blocks / total_buckets))
buckets_per_batch= clamp(target_rows_per_query / rows_per_bucket, 1, total_buckets)
batch_width      = buckets_per_batch * slice
```

- Sparse OTLP metrics, 30 days, ~43k blocks → `rows_per_bucket ≈ 60`, `buckets_per_batch` clamps to
  `total_buckets` → **one query for the whole month**.
- A dense `cpu`-tagged process → `rows_per_bucket` in the thousands → hour- to few-hour-scale
  batches, i.e. roughly today's behavior, with no memory regression.

`nb_blocks` counts blocks whose *event* time overlaps the query range, while the batch queries
filter on *insert* time, so it is a lower bound: blocks of the same process/tag inserted in the same
window but with event times outside the range are returned and not counted. It is a density
estimate, not a bound — which is fine, because the target is a heuristic and the hard memory bound
is one bucket of `pending`, not one batch.

Config gains two fields (`JitPartitionConfig` stays `Copy`):

```rust
/// Rows a single batch query should aim to return; drives the derived batch width.
pub target_rows_per_query: i64,                       // default 250_000
/// Pins the batch width, bypassing the density-derived value. `None` = adaptive.
pub max_query_insert_time_range: Option<TimeDelta>,   // default None
```

A pinned width shorter than the slice, or not a whole multiple of it, is rounded up to whole
buckets so batch edges stay bucket-aligned.

### Why the lean projection is in scope

A row *count* target only bounds memory if rows have a predictable width. With stream blobs in the
projection they vary 150 B → 3.5 KB, so `target_rows_per_query = 250_000` means anywhere from 37 MB
to 875 MB — and `make_runtime_env` installs an `UnboundedMemoryPool` unless
`MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` is set (`runtime.rs:40-43`), so nothing spills. Stripping
the blobs makes every row ~150 B, so the target means what it says (~37 MB) for every view.

The process variant therefore gains **pre-query 3**, run once over the whole insert range (stream
metadata is immutable after registration, so it does not need refetching per batch):

```sql
SELECT stream_id,
       first_value("process_id")                    as process_id,
       first_value("streams.dependencies_metadata") as dependencies_metadata,
       first_value("streams.objects_metadata")      as objects_metadata,
       first_value("streams.tags")                  as tags,
       first_value("streams.properties")            as properties,
       first_value("streams.format")                as format
FROM source
WHERE process_id = '{process_id}' AND array_has("streams.tags", '{stream_tag}')
  AND insert_time >= '{begin}' AND insert_time < '{end}'
GROUP BY stream_id
```

`streams_view.rs:27-37` already uses exactly this shape over the same source. The aliases are
deliberately unprefixed so the result rows can be read by the **existing shared helper**
`metadata::stream_metadata_from_batch_row` (`metadata.rs:128`) — which the JIT path does not use
today, having a hand-inlined copy at `:577-616` written against the `streams.`-prefixed names. That
copy is deleted; `format` (not part of `StreamMetadata`) is kept alongside in the map:

```rust
HashMap<Uuid, (Arc<StreamMetadata>, Arc<String> /* format */)>
```

The batch query then keeps `array_has("streams.tags", ...)` in its `WHERE` (filtering needs no
projection) and selects only block columns plus `stream_id`. A `stream_id` missing from the map is a
hard error — same predicate, same range, so it cannot happen unless something is wrong.

The stream-scoped variant is already lean (it projects only `streams.format`, a short string) and is
left alone.

> **Invariant, to be stated in the rustdoc of the batch SQL:** the batch query must not project
> stream-level columns. Re-adding one would reintroduce the memory hazard with no test failing —
> see the projection guard in Testing Strategy.

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
setup errors still surface at the call site. Call sites change from `for part in all_partitions` to
`while let Some(part) = specs.try_next().await? { ... }`; each spec is materialized and **dropped**
before the next is pulled, so the query stream backpressures naturally while
`write_partition_from_blocks` / `update_partition` runs. Callers already build a fresh
`BlocksView::new()` per call — wrapping it in `Arc` is a one-line change at each site.

`net_spans_view` and `thread_spans_view` thread a `same_run_ranges: Vec<TimeRange>` accumulator
through their loop; that stays as-is (it accumulates ranges, not blocks).

### Equivalence argument (keep in the PR description)

For a fixed set of source blocks, the new code feeds `group_blocks_into_partitions` *the same list,
in the same order, for the same buckets* as the old code:

- Same bucket set: batch windows tile `[trunc(min), trunc(max) + slice)` with slice-aligned edges,
  the same range the old loop walked.
- Same membership: the old per-bucket predicate `insert_time >= b AND insert_time < b+slice` is
  exactly `insert_time.duration_trunc(slice) == b` for rows inside the batch window.
- Same order: `ORDER BY insert_time, block_id` within a batch, batches ascending.
- Width-independent: batch edges are bucket-aligned and grouping is per bucket, so the derived width
  — which may differ between runs as density changes — cannot affect the output.

The only representational change is that `PartitionSourceBlock::stream` now points at one
`Arc<StreamMetadata>` per *stream* rather than one per *block*; the contents are identical and
consumers treat it as read-only. Emitted specs — `block_ids_hash` and every cut point, under both
`BlockOrder` variants — are byte-identical, and every cached JIT partition still reports up to date.
There is no one-time regeneration cost.

### Keeping the segment functions

`generate_stream_jit_partitions_segment` (`:406`) and `generate_process_jit_partitions_segment`
(`:528`) are **kept, not deleted**: `rust/analytics/tests/thread_spans_ordering_db_test.rs` drives
`generate_stream_jit_partitions_segment` from ~10 sites (single query range, no bucket subdivision,
asserting on returned specs). Deleting them would mean rewriting a 2000-line DB test suite for no
benefit. They keep their current `collect()`-based bodies and their current (fat) projection; only
the streaming path gets the lean one.

### Instrumentation

Keep `instrument_named!` spans; replace the per-segment `collect_partition_blocks` span (`:568`)
with one per batch (`stream_partition_blocks`), and log the derived batch width and row estimate
once per call so the adaptive decision is visible in traces.

## Implementation Steps

1. `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Add `target_rows_per_query: i64` (default `250_000`) and
     `max_query_insert_time_range: Option<TimeDelta>` (default `None`) to `JitPartitionConfig`;
     derive `Clone, Copy`.
   - Add `COUNT(*)` to both MIN/MAX pre-queries.
   - Add `fn batch_windows(insert_range, slice, nb_blocks, config) -> impl Iterator<Item = TimeRange>`
     implementing the derivation above (slice-aligned, last window clamped to the range end).
   - Add the per-stream metadata pre-query for the process variant, decoded via
     `metadata::stream_metadata_from_batch_row`, into
     `HashMap<Uuid, (Arc<StreamMetadata>, Arc<String>)>`.
   - Rewrite `generate_stream_jit_partitions` to return a `BoxStream`: keep the pre-queries, then
     `async_stream::try_stream!` over batch windows, consuming each via `df.execute_stream()`,
     flushing a bucket through `group_blocks_into_partitions` on bucket change and at the end.
   - Same for `generate_process_jit_partitions`, with the lean projection and the metadata map.
   - Leave both `*_segment` functions in place.
2. Update the seven call sites to consume the stream (`futures::TryStreamExt::try_next` loop,
   `Arc::new(BlocksView::new()?)`, owned `stream_tag`): `log_view.rs`, `metrics_view.rs`,
   `net_spans_view.rs`, `images_view.rs`, `async_events_view.rs`, `otel/spans_view.rs`,
   `thread_spans_view.rs`.
3. Add `rust/analytics/tests/jit_batch_windows_tests.rs` (see Testing Strategy).
4. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.
5. Manual verification — see Testing Strategy.

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — main change.
- `rust/analytics/src/lakehouse/{log_view,metrics_view,net_spans_view,images_view,async_events_view,thread_spans_view}.rs`,
  `rust/analytics/src/lakehouse/otel/spans_view.rs` — consume the stream.
- `rust/analytics/tests/jit_batch_windows_tests.rs` — new.
- `rust/analytics/tests/thread_spans_ordering_db_test.rs` — unchanged (segment functions kept).

## Trade-offs

- **Adaptive width vs. a fixed `TimeDelta`.** A constant is wrong at both ends: a day still costs 30
  queries for a month of near-empty OTLP metrics, while for a busy many-threaded process it buffers
  far more than intended. Deriving it from a free `COUNT(*)` makes query count scale with data
  volume instead of wall-clock range, and leaves one pinnable override for when a specific instance
  needs it.
- **Estimate vs. hard bound.** The derived width is a heuristic (the count is a lower bound, see
  above). That is acceptable because the real memory bound is one bucket of `pending` plus the spec
  being written — the batch is a sort buffer, not a retention window.
- **Lean projection vs. leaving the join as-is.** Not needed for the motivating workload (OTLP
  streams carry empty metadata sentinels), but it is the precondition for a row-count-based width to
  bound memory, it removes a per-block CBOR decode, and it lets the JIT path share
  `stream_metadata_from_batch_row` instead of duplicating it. Cost: one extra small pre-query.
- **Streaming API vs. returning a Vec.** The Vec is the current unbounded-memory hazard; all callers
  already process specs one at a time. Cost: a signature change across seven views and owned
  (`Arc`/cloned) captures.
- **Reusing `group_blocks_into_partitions` per bucket vs. a streaming accumulator.** A per-block
  accumulator cannot express `BlockOrder::EventTime`, which stable-sorts a whole segment by event
  time and computes a suffix-minimum over the full list to find insert-safe cut points. Buffering
  one bucket costs the same memory the old per-hour `collect()` already paid, and buys
  byte-identical output.
- **Keeping the MIN/MAX pre-query.** Insert-time bounds are needed *before* the blocks queries (they
  scope the Postgres partition fetch and the batch windows), and an event-time filter on the blocks
  query would break the "JIT partition covers its whole hour" invariant.
- **Not widening `max_insert_time_slice`.** Wider buckets would cut the tiny-partition count for
  sparse instances, but bucket boundaries are baked into cached partitions — it is a migration. See
  *Follow-ups*.
- **Sequential batches vs. prefetching the next batch.** Prefetch (`buffered(2)`) would overlap one
  batch's materialization with the next query, at the cost of holding two batches' rows. Start
  sequential.

## Superseded Design

The earlier version of this plan proposed a streaming `JitPartitionAccumulator` with a tie-atomic
soft cap, replacing the inline chunking that existed at the time. Do not resurrect it:

- #1429/#1440 already replaced that inline chunking with `group_blocks_into_partitions`
  (`jit_partitions.rs:227`), which handles the determinism concern and adds `BlockOrder::EventTime`.
- Under `EventTime`, grouping stable-sorts a whole segment by `(begin_ticks, end_ticks)` and uses a
  suffix-minimum over the entire list to pick insert-safe cut points, so cut decisions cannot be
  made from a per-block streaming `push()`.
- Its accepted one-time cache-regeneration cost is no longer worth paying, since this plan achieves
  the same performance with byte-identical output.

## Follow-ups (not in scope)

**Batch the freshness checks.** After this change, a sparse instance's remaining serial cost is one
`is_jit_partition_up_to_date` round trip per spec (~720 for a 30-day OTLP metrics range) — issued
one at a time from each caller's loop. A single query per batch (`WHERE (begin_insert_time,
end_insert_time) IN (...)`) would collapse them, but it has to preserve the three `BlockOrder`-
dependent matching semantics at `:773-843`.

**Revisit the hour slice for sparse instances.** ~720 tiny partitions for a month of OTLP metrics is
a lot of parquet files and `lakehouse_partitions` rows for very few events. The blanket "invalidates
every cached JIT partition" objection weighs much less when the partitions are few and cheap to
regenerate, but the slice must remain a stable per-view-instance constant (see *Which knobs may vary
between runs*), so this is a migration with a compatibility story, not a config change.

**Sort elision.** The original blocker is gone: #1336 shipped as #1340. `BlocksView` now carries an
`ordered_merger` (`blocks_view.rs:74-88`) and records `sort_order = ["insert_time"]`
(`blocks_view.rs:143`); `make_partitioned_execution_plan` supports `ScanOrdering::PerFile { columns }`,
gated on `certifies_sort_order` and degrading silently to `Unordered` otherwise
(`partitioned_execution_plan.rs:266-275`). Declaring it on the JIT blocks query would let
`SortPreservingMergeExec` replace the per-batch `SortExec`. **But it is not a drop-in:** the recorded
guarantee is `insert_time` only, so a k-way merge orders equal-`insert_time` rows by file arrival,
not by `block_id` — changing tie order inside a bucket, which under `BlockOrder::InsertTime` moves
greedy cut points and invalidates cached specs. It also requires a data migration: pre-#1340
partitions have `sort_order` NULL and never certify, and one uncertified file disables the
declaration for the whole scan (`SELECT count(*) FROM list_partitions() WHERE view_set_name =
'blocks' AND sort_order IS NULL AND file_size > 0`). Low marginal value once query count scales with
data volume.

## Testing Strategy

- **Unit tests** (`rust/analytics/tests/jit_batch_windows_tests.rs`, pure logic — no DB or object
  store):
  - `batch_windows` tiles a slice-aligned range with no gaps or overlaps, edges on bucket
    boundaries, last window ending exactly at the range end;
  - sparse density (few blocks over many buckets) collapses to a single window;
  - dense density yields multiple windows, each ≥ one bucket;
  - a pinned `max_query_insert_time_range` overrides the derivation and is rounded up to whole
    buckets;
  - zero blocks / a single-bucket range yield one window.
- **Projection guard**: assert the process-variant batch SQL builder emits no `streams.` column in
  its `SELECT` list (the `WHERE` clause still may). One small string assertion — it guards the exact
  regression the rustdoc warns about.
- **Existing suites**: `cargo test` from `rust/` must pass unchanged — notably
  `thread_spans_ordering_db_test.rs`, which pins `BlockOrder::EventTime` cut-point behavior through
  the retained segment functions.
- **Manual cache-stability check** (the load-bearing regression test): start services
  (`python3 local_test_env/ai_scripts/start_services.py` or monolith), run a JIT query against the
  **old** binary, then the **new** one. Because the design is output-identical, the new binary must
  log `partition up to date` for *every* partition — any regeneration is a bug, not an accepted
  cost. Repeat for a `BlockOrder::EventTime` view (`thread_spans` / `net_spans`).
- **Performance check — the motivating workload**: a sparse OTLP metrics view instance for a single
  process over a multi-week range via `micromegas-query`. Confirm in the trace that the per-hour
  `collect_partition_blocks` chain is replaced by one (or very few) `stream_partition_blocks` spans,
  and measure end-to-end wall clock before/after.
- **Memory check — the dense case**: an `async_events` (tag `"cpu"`) query on a many-threaded
  process over a long range, watching process RSS (system_monitor gauges, #1330). Flat, with no
  growth proportional to range length, and no regression versus the per-hour code.

## Documentation

No mkdocs page covers JIT partition internals (verified by grep). Rustdoc on the new config fields,
the batch-window derivation, the per-stream metadata pre-query, the lean-projection invariant, and
the rewritten functions suffices.
