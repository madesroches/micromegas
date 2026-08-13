# Faster JIT Partition Generation: Batched Block Queries + Batched Freshness Checks

Issue: [#1474](https://github.com/madesroches/micromegas/issues/1474)

## Overview

The driving workload is **sparse view instances queried over long ranges** — for example OTLP
metrics from a single process, where the protocol's inefficiency means a month of data is a few
tens of thousands of tiny blocks. JIT (just-in-time) lakehouse views resolve their source blocks in
`rust/analytics/src/lakehouse/jit_partitions.rs` by slicing the insert-time range into 1-hour
segments and running **one DataFusion query per segment, sequentially** — ~256ms per
`collect_partition_blocks` span, back-to-back, regardless of how little data the hour holds. A
30-day range costs 720 queries and ~3 minutes of pure per-query overhead to find almost nothing.
The caller loops then pay a second per-hour cost: one `is_jit_partition_up_to_date` Postgres round
trip per emitted spec, on **every** query over the view, not just the first.

Two coupled changes, plus one enabling cleanup:

1. **Block-query count scales with data, not with time** — replace the per-hour loop with batched
   queries whose width is derived from observed block density (a `COUNT(*)` piggybacked on the
   existing pre-query). A sparse instance collapses a month into one query; a dense one falls back
   to hour-scale batches. Hour bucketing is recovered client-side from each row's `insert_time`.
2. **Freshness-check count scales the same way** — the five `BlockOrder::InsertTime` callers stop
   issuing one `is_jit_partition_up_to_date` round trip per spec; one candidates query per
   `generate_*` call feeds the same matching logic, refactored into a pure function.
3. **Lean projection** (enabling cleanup) — the process-scoped query stops projecting stream-level
   metadata onto every block row, fetching it once per stream. This is what makes (1)'s row-count
   target a real memory bound rather than a guess, and it deletes a per-row CBOR decode plus a
   hand-inlined copy of an existing helper.

**The grouping algorithm is not touched, and no public signature changes.** Each hour bucket's
blocks are still handed to the existing `group_blocks_into_partitions` (`jit_partitions.rs:227`)
exactly as today, so emitted specs are byte-identical and every cached JIT partition stays valid.
All the risk in this area lives in the bucketing/cut-point rules, and this plan changes neither.

Two earlier drafts of this plan proposed more machinery (a streaming `JitPartitionAccumulator`,
then a streaming-specs API change across seven views) — see *Superseded Design* for why both were
dropped.

## Current State

### Call graph

Seven views call into this file:

- `generate_process_jit_partitions` (`jit_partitions.rs:635`) — `log_view.rs:172`,
  `metrics_view.rs:174`, `net_spans_view.rs:356`, `images_view.rs:129`,
  `async_events_view.rs:157`, `otel/spans_view.rs:133`. The five `BlockOrder::InsertTime` callers
  share an identical loop: `is_jit_partition_up_to_date` → `write_partition_from_blocks` per spec.
  `net_spans_view` instead calls its own `update_partition` (`net_spans_view.rs:256`), which
  performs the freshness check internally, interleaved with retire logic.
- `generate_stream_jit_partitions` (`jit_partitions.rs:465`) — `thread_spans_view.rs:375`; caller
  loops its own `update_partition` (`thread_spans_view.rs:270`), same internal-check shape as
  `net_spans`.

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
- **Per-spec freshness round trips, serialized.** ~720 tiny specs for a sparse month means ~720
  sequential `is_jit_partition_up_to_date` Postgres queries (`:756`) from the caller loop — paid on
  every query over the view, even when every partition is up to date.
- **Redundant re-reads.** `filter_insert_range` keeps any blocks-view partition *overlapping* the
  hour, so a merged daily partition is scanned by all 24 hour-queries.
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

- **Tiny-partition fan-out.** A sparse month still yields ~720 non-empty hour buckets → ~720 tiny
  parquet files (written once, but *opened by every scan*) and as many `lakehouse_partitions`
  rows. That fan-out is set by `max_insert_time_slice`, and shrinking it is a migration — see
  *Follow-ups*.
- **Whole-range spec vec in RAM.** The generators keep returning `Vec<SourceDataBlocksInMemory>`.
  Status quo, and strictly lighter than today after the lean projection (blocks share one
  `Arc<StreamMetadata>` per stream instead of carrying one each). A previous revision of this plan
  streamed the specs instead; see *Superseded Design* for why that was cut.
- **`EventTime` freshness checks.** `net_spans`/`thread_spans` keep their per-spec checks inside
  `update_partition` — see *Follow-ups*.

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

## Design

### Batched block queries, bucketed client-side

The three-step shape survives; only step 3 changes:

```
pre-queries (unchanged position):
  1. MIN/MAX/COUNT insert-time query         -> [trunc(min), trunc(max) + slice), row estimate
  2. fetch_overlapping_insert_range_for_view -> PartitionCache
  3. per-stream metadata query (process variant only, see below)

batch width: derived from (1)'s count, slice-aligned  (see Adaptive batch width)

for each batch window (sequential, ascending):
    filter cache to the batch window
    SQL: insert_time >= batch_begin AND insert_time < batch_end
         ORDER BY insert_time, block_id          -- predicate and order verbatim from today
    collect(), parse rows into PartitionSourceBlocks (stream metadata looked up, not rebuilt)
    split the row list on changes of insert_time.duration_trunc(slice)   -- rows are sorted,
                                                                          -- so buckets are runs
    group_blocks_into_partitions per bucket, append to the output vec
```

Batch edges are bucket-aligned, so a bucket never straddles two batches; batches run in ascending
time order, so the concatenated spec sequence is exactly today's. Empty buckets contribute no rows,
hence no specs — same as today's empty per-hour query. Return types and function signatures are
**unchanged** (`Vec<SourceDataBlocksInMemory>`), so the generation half of this plan needs no
call-site edits at all.

### Adaptive batch width

Pre-query 1 gains `COUNT(*)` — the same scan and the same predicate it already runs, so it costs no
extra round trip (unlike MIN/MAX it cannot short-circuit on parquet row-group stats, but on a
blocks-view scan that delta is noise):

```sql
SELECT MIN(insert_time) as min_insert_time,
       MAX(insert_time) as max_insert_time,
       COUNT(*)         as nb_blocks
FROM source WHERE <unchanged predicate>
```

Then, with `TARGET_ROWS_PER_QUERY: i64 = 250_000` (a rustdoc'd const — promote to config only if a
real need appears):

```
total_buckets    = insert_range / slice
rows_per_bucket  = max(1, ceil(nb_blocks / total_buckets))
buckets_per_batch= clamp(TARGET_ROWS_PER_QUERY / rows_per_bucket, 1, total_buckets)
batch_width      = buckets_per_batch * slice
```

- Sparse OTLP metrics, 30 days, ~43k blocks → `rows_per_bucket ≈ 60`, `buckets_per_batch` clamps to
  `total_buckets` → **one query for the whole month**.
- A dense `cpu`-tagged process → `rows_per_bucket` in the thousands → hour- to few-hour-scale
  batches, i.e. roughly today's behavior, with no memory regression.

`nb_blocks` counts blocks whose *event* time overlaps the query range, while the batch queries
filter on *insert* time, so it is a lower bound: blocks of the same process/tag inserted in the same
window but with event times outside the range are returned and not counted. It is a density
estimate, not a bound — acceptable because the buffered unit is one batch's collected rows
(~150 B each after the lean projection, so ~37 MB at target), and the width only tunes overhead,
never correctness.

### Why the lean projection is in scope

A row *count* target only bounds memory if rows have a predictable width. With stream blobs in the
projection they vary 150 B → 3.5 KB, so a 250k-row target means anywhere from 37 MB to 875 MB — and
`make_runtime_env` installs an `UnboundedMemoryPool` unless
`MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` is set (`runtime.rs:40-43`), so nothing spills. Stripping
the blobs makes every row ~150 B, so the target means what it says for every view.

The process variant therefore gains **pre-query 3**, run once over the whole insert range (stream
metadata is immutable after registration, so one fetch covers every batch):

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

### Batched freshness checks (`InsertTime` callers)

With the block queries batched, the next serial cost is the caller loop: one
`is_jit_partition_up_to_date` round trip per spec, on every query over the view.

**One matcher, one fetch shape.** `is_jit_partition_up_to_date` is split so the three
`BlockOrder`-dependent matching semantics (`:773-843`) live in exactly one pure function:

```rust
/// One lakehouse_partitions candidate row:
/// (begin_insert_time, end_insert_time, file_schema_hash, source_data_hash).
struct PartitionFreshnessRow { ... }

/// Filters `candidates` down to the rows the per-spec SQL returns today (exact range equality for
/// EventTime, exact match for a degenerate InsertTime range, inclusive overlap otherwise), then
/// applies today's rows.len() == 1 / file_schema_hash / object-count checks verbatim.
fn spec_is_up_to_date(view_meta, spec, block_order, candidates: &[PartitionFreshnessRow]) -> Result<bool>
```

Candidates are fetched by **inclusive insert-range overlap**
(`begin_insert_time <= $max AND end_insert_time >= $min`) — a superset of what each of the three
variant-specific queries returns (an exact-equality row necessarily overlaps the spec's range), with
the variant predicate re-applied in Rust, so per-spec verdicts are unchanged.
`is_jit_partition_up_to_date` keeps its public signature (fetch the spec's own range, run the
matcher) for the `EventTime` callers, which stay per-spec — their checks are interleaved with
retire logic inside their own `update_partition`, and they are not the motivating workload.

**One query per generate call.** New helper next to it:

```rust
/// One candidates fetch over [specs.first().min, specs.last().max] (specs have ascending,
/// non-overlapping insert ranges), then the matcher per spec. Returns up-to-date flags parallel
/// to `specs`.
pub async fn find_up_to_date_partitions(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    block_order: BlockOrder,
    specs: &[SourceDataBlocksInMemory],
) -> Result<Vec<bool>>
```

The five `InsertTime` call sites (`log_view.rs:190`, `metrics_view.rs:192`, `images_view.rs:146`,
`async_events_view.rs:184`, `otel/spans_view.rs:157`) call it once before their loop and skip
flagged specs — a sparse month becomes 1 freshness query instead of ~720. For that month the
candidate set is ~720 small rows, well under any concern.

**Verdicts reflect pre-run state.** Today spec *i*'s check runs after specs `0..i`'s writes;
batched, all verdicts are computed first. A this-run write can only match a later spec's overlap
predicate when two specs' ranges *touch* (equal boundary insert-times straddling a cut) — a corner
where today's interleaved check misbehaves in both directions (it can double-match or wrongly adopt
the neighbor). Deciding from pre-run state only is the saner semantics and changes nothing outside
that corner.

**Race window.** The check→write pair was never atomic: a concurrent `jit_update` of the same view
instance can commit a partition between one spec's check and its write today. Checking up front
widens that window from one write to one run's writes and changes nothing else — the conflict
outcomes (a redundant rewrite, or a write that trips `lakehouse_partitions_no_overlap`) are exactly
the ones the narrow window already permits, and concurrent `jit_update` of the same process + view +
time range is the rare case to begin with.

### Equivalence argument (keep in the PR description)

For a fixed set of source blocks, the new code feeds `group_blocks_into_partitions` *the same list,
in the same order, for the same buckets* as the old code:

- Same bucket set: batch windows tile `[trunc(min), trunc(max) + slice)` with slice-aligned edges,
  the same range the old loop walked.
- Same membership: the old per-bucket predicate `insert_time >= b AND insert_time < b+slice` is
  exactly `insert_time.duration_trunc(slice) == b` for rows inside the batch window.
- Same order: `ORDER BY insert_time, block_id` within a batch, batches ascending, specs appended
  batch-by-batch — the concatenation is today's.
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
the batched path gets the lean one.

### Instrumentation

Keep `instrument_named!` spans; replace the per-segment `collect_partition_blocks` span (`:568`)
with one per batch, log the derived batch width and row estimate once per call so the adaptive
decision is visible in traces, and give the candidates fetch in `find_up_to_date_partitions` its own
span with the spec count.

## Implementation Steps

1. `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Add `COUNT(*)` to both MIN/MAX pre-queries; add `TARGET_ROWS_PER_QUERY` and
     `fn batch_windows(insert_range, slice, nb_blocks) -> impl Iterator<Item = TimeRange>`
     (slice-aligned, last window clamped to the range end).
   - Add the per-stream metadata pre-query for the process variant, decoded via
     `metadata::stream_metadata_from_batch_row`, into
     `HashMap<Uuid, (Arc<StreamMetadata>, Arc<String>)>`.
   - Rewrite the two generators' step-3 loops: iterate `batch_windows`, collect each batch, split
     rows into buckets on `duration_trunc` change, `group_blocks_into_partitions` per bucket.
     Signatures unchanged; both `*_segment` functions left in place.
   - Split `is_jit_partition_up_to_date` into a candidates fetch (inclusive insert-range overlap)
     plus the pure matcher `spec_is_up_to_date`; keep its public signature and behavior. Add
     `find_up_to_date_partitions`.
2. Update the five `InsertTime` call sites (`log_view.rs`, `metrics_view.rs`, `images_view.rs`,
   `async_events_view.rs`, `otel/spans_view.rs`) to call `find_up_to_date_partitions` once and skip
   flagged specs. `net_spans_view.rs` / `thread_spans_view.rs` are untouched.
3. Add `rust/analytics/tests/jit_batch_windows_tests.rs` and
   `rust/analytics/tests/jit_freshness_tests.rs` (see Testing Strategy).
4. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.
5. Manual verification — see Testing Strategy.

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — main change.
- `rust/analytics/src/lakehouse/{log_view,metrics_view,images_view,async_events_view}.rs`,
  `rust/analytics/src/lakehouse/otel/spans_view.rs` — one `find_up_to_date_partitions` call each.
- `rust/analytics/tests/jit_batch_windows_tests.rs`, `rust/analytics/tests/jit_freshness_tests.rs`
  — new, pure logic.
- `rust/analytics/tests/thread_spans_ordering_db_test.rs` — unchanged (segment functions kept).

## Trade-offs

- **Adaptive width vs. a fixed `TimeDelta`.** A constant is wrong at both ends: a day still costs 30
  queries for a month of near-empty OTLP metrics, while for a busy many-threaded process it buffers
  far more than intended. Deriving it from a free `COUNT(*)` makes query count scale with data
  volume instead of wall-clock range. No pin/override knob: if the derivation is wrong, fix the
  derivation.
- **Estimate vs. hard bound.** The derived width is a heuristic (the count is a lower bound, see
  above). Acceptable: the buffered unit is one batch's lean rows (~37 MB at target), and width
  cannot affect output.
- **Vec return vs. streaming specs.** The previous revision of this plan changed both generators to
  return a `BoxStream` of specs. Cut: it rippled owned/`Arc` captures through seven views, replaced
  a simple loop with a bucket-flush state machine inside `async_stream`, and held a DataFusion /
  object-store stream open — paused for potentially minutes — across partition writes. The
  motivating sparse workload's whole spec vec is a few MB, and the lean projection makes the dense
  case *lighter* than today (shared per-stream `Arc`s). Client-side bucketing keeps the door open:
  streaming can be reintroduced later without touching the grouping logic. See *Follow-ups*.
- **Lean projection vs. leaving the join as-is.** Not needed for the motivating workload (OTLP
  streams carry empty metadata sentinels), but it is the precondition for a row-count-based width to
  bound memory, it removes a per-block CBOR decode, and it lets the JIT path share
  `stream_metadata_from_batch_row` instead of duplicating it. Cost: one extra small pre-query.
- **Batched freshness vs. per-spec checks.** Widens the pre-existing (non-atomic) check→write race
  window from one spec to one run, with unchanged failure modes; verdicts decided from pre-run
  state are saner in the touching-ranges corner (see Design). The alternative batching shape —
  `WHERE (begin_insert_time, end_insert_time) IN (...)` — cannot express the `InsertTime`
  overlap/degenerate semantics, hence the superset fetch + Rust matcher.
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

## Superseded Design

Two earlier drafts; do not resurrect either:

- **Streaming `JitPartitionAccumulator`** (`tasks/jit_single_query_plan.md`, the original draft):
  predated #1429/#1440, which replaced the inline chunking it targeted with
  `group_blocks_into_partitions` (`jit_partitions.rs:227`). Under `BlockOrder::EventTime`, grouping
  stable-sorts a whole segment and uses a suffix-minimum over the entire list, so cut decisions
  cannot be made from a per-block streaming `push()`; its accepted one-time cache-regeneration cost
  is also no longer worth paying.
- **Streaming-specs API** (the previous revision of this file): both generators returned
  `BoxStream<'static, Result<SourceDataBlocksInMemory>>`, consuming batches via `execute_stream()`
  with an in-stream bucket-flush state machine, and all seven call sites were rewritten for owned
  captures. Dropped for the reasons in *Trade-offs*; the batched-queries core, the adaptive width,
  and the lean projection survive unchanged from that draft.

## Follow-ups (not in scope)

**Revisit the hour slice for sparse instances.** ~720 tiny partitions for a month of OTLP metrics is
a lot of parquet files and `lakehouse_partitions` rows for very few events — and after this plan it
is the dominant remaining per-query cost (every scan opens every file). The blanket "invalidates
every cached JIT partition" objection weighs much less when the partitions are few and cheap to
regenerate, but the slice must remain a stable per-view-instance constant (see *Which knobs may vary
between runs*), so this is a migration with a compatibility story, not a config change.

**Batch the `EventTime` freshness checks.** `net_spans`/`thread_spans` keep per-spec
`is_jit_partition_up_to_date` calls inside their `update_partition`, interleaved with retire logic
and the `same_run_ranges` accumulator. The same matcher applies; threading batch verdicts through
that path is the work. Not the motivating workload.

**Streaming specs.** If a dense instance over a very long range ever shows a real memory problem,
the generators can return a stream of specs without touching grouping (bucketing is already
client-side). Reintroduce it against the *Superseded Design* notes — including the
paused-open-stream risk it carries.

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

- **Batch-window unit tests** (`rust/analytics/tests/jit_batch_windows_tests.rs`, pure logic — no
  DB or object store):
  - `batch_windows` tiles a slice-aligned range with no gaps or overlaps, edges on bucket
    boundaries, last window ending exactly at the range end;
  - sparse density (few blocks over many buckets) collapses to a single window;
  - dense density yields multiple windows, each ≥ one bucket;
  - zero blocks / a single-bucket range yield one window.
- **Freshness matcher unit tests** (`rust/analytics/tests/jit_freshness_tests.rs`, pure logic):
  table-driven over the three variants — no candidates, exact match, wider overlapping row,
  multiple rows, degenerate range, schema-hash mismatch, object count below/equal/above — asserting
  `spec_is_up_to_date` reproduces the per-variant SQL semantics at `:773-843`.
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
  `collect_partition_blocks` chain is replaced by one (or very few) per-batch spans, that the
  per-spec `sql_select_matching_partitions` chain collapses to one candidates fetch, and measure
  end-to-end wall clock before/after.
- **Memory check — the dense case**: an `async_events` (tag `"cpu"`) query on a many-threaded
  process over a long range, watching process RSS (system_monitor gauges, #1330). No regression
  versus the per-hour code (expected: slightly lighter, from the shared per-stream `Arc`s).

## Documentation

No mkdocs page covers JIT partition internals (verified by grep). Rustdoc on `TARGET_ROWS_PER_QUERY`
and the batch-window derivation, the per-stream metadata pre-query, the lean-projection invariant,
the freshness matcher split, and the rewritten loop bodies suffices.
