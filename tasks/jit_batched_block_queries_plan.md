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
   queries whose width is derived from observed block density (a per-bucket `COUNT(*) ... GROUP BY`
   over the insert range, under the same predicate the batch queries use). A sparse instance
   collapses a month into one query; a dense one falls back to hour-scale batches. Hour bucketing is
   recovered client-side from each row's `insert_time`.
2. **Freshness-check count scales the same way** — the five `BlockOrder::InsertTime` callers stop
   issuing one `is_jit_partition_up_to_date` round trip per spec; one candidates query per
   `generate_*` call feeds the same matching logic, refactored into a pure function.
3. **Lean projection** (enabling cleanup) — the process-scoped query stops projecting stream-level
   metadata onto every block row, fetching it once per stream. This is what makes (1)'s row-count
   target a real memory bound rather than a guess, and it deletes a per-row CBOR decode plus a
   hand-inlined copy of an existing helper.

**The grouping algorithm is not touched, and no function signature changes.** (The one public-API
change is additive: a new `pub` field, `JitPartitionConfig::target_rows_per_query` — see
*Implementation Steps* for the existing test-literal updates it requires.) Each hour bucket's
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
pre-queries (unchanged position, one new step):
  1. MIN/MAX insert-time query               -> [trunc(min), trunc(max) + slice)
  2. fetch_overlapping_insert_range_for_view -> PartitionCache
  2b. per-bucket COUNT over that insert range, batch predicate, against (2)'s cache -> one
      (bucket_start, nb_blocks) row per non-empty bucket (see Adaptive batch width)
  3. per-stream metadata query (process variant only, see below)

batch windows: (2b)'s per-bucket counts packed greedily up to target_rows_per_query, slice-aligned
               (see Adaptive batch width)

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

Pre-query 1 (MIN/MAX, unchanged predicate) determines `insert_time_range`. Pre-query 2
(`fetch_overlapping_insert_range_for_view`) then fetches the `PartitionCache` covering that insert
range — the count query below can only be answered from that cache (pre-query 1's own partition set
is selected by *event*-time overlap against the query range, a different axis; see *Design → Batched
block queries*), so it must run after, not before, pre-query 2. A third, new query then counts
blocks **per bucket** over that same insert range, under the full batch-query predicate — the
identity filter (`process_id = ... AND array_has("streams.tags", ...)` for the process variant,
`stream_id = ...` for the stream variant) *and* `insert_time >= trunc(min) AND insert_time <
trunc(max) + slice` — not the event-time predicate of pre-query 1:

```sql
SELECT date_bin('{slice}', insert_time) as bucket, COUNT(*) as nb_blocks
FROM source
WHERE process_id = '{process_id}' AND array_has("streams.tags", '{stream_tag}')
  AND insert_time >= '{begin}' AND insert_time < '{end}'
GROUP BY bucket
```

(stream variant: `WHERE stream_id = '{stream_id}' AND insert_time >= '{begin}' AND insert_time <
'{end}'`). Omitting the identity filter would count every block in the shared `blocks` view for that
window, not just this view instance's — on a shared lake, the counts would then reflect every
process/stream, not the sparse instance actually being queried. Grouping by bucket costs nothing
extra over a scalar `COUNT(*)`: still one round trip, returning one small row per *non-empty*
bucket (a sparse month is at most a few hundred rows) — but it replaces an average-density estimate
with per-bucket ground truth.

This is an extra round trip (unlike piggybacking on pre-query 1, it cannot reuse that scan), but it
is cheap — a count-only aggregate over the same blocks-view slice the batch queries themselves will
read — and it is the only predicate that makes the resulting per-bucket counts match what the
batches actually return. Counting under pre-query 1's event-time predicate instead would make every
bucket's count an unbounded lower bound: a block's `insert_time` is independent of its event span,
so a single long-lived, late-flushed block can pin `max_insert_time` far from a narrow query range
while nothing else in that range matches the event-time filter, collapsing every counted bucket to
(at most) 1 row — the packing below would then merge the whole range into one batch, which then
collects every block of that process/tag over the whole insert range, regardless of size.

Then, with `JitPartitionConfig::target_rows_per_query` (a new `i64` field alongside `max_nb_objects`,
defaulting to `250_000` and rustdoc'd — a config field rather than a bare const precisely so
DB-gated tests can lower it to force a multi-batch run, the same way `thread_spans_ordering_db_test.rs`
already drives a lowered `max_nb_objects`; see *Testing Strategy*), `batch_windows` packs consecutive
buckets greedily, closing a batch just before it would exceed the target:

```
running = 0
batch_begin = first bucket's begin
for each bucket in ascending order (empty buckets count as 0, so they never trigger a close):
    if running > 0 && running + bucket.nb_blocks > target_rows_per_query:
        close batch [batch_begin, bucket.begin);  running = 0;  batch_begin = bucket.begin
    running += bucket.nb_blocks
close the final batch [batch_begin, insert_range.end)
```

- Sparse OTLP metrics, 30 days, ~43k blocks spread evenly → every bucket's count is far below the
  target, so the running total never forces a close → **one query for the whole month**.
- A dense, evenly busy `cpu`-tagged process packs several to a few dozen buckets per batch before
  the running total reaches `target_rows_per_query`, buffering up to that many rows at once: a real
  per-batch memory increase over today's per-hour loop (which buffers only that hour's rows), traded
  for far fewer queries.
- A burst concentrated in a handful of buckets amid many near-empty ones is *not* diluted by the
  buckets around it: the batch containing the burst closes as soon as its own running total would
  exceed `target_rows_per_query`, regardless of how many empty buckets happen to be adjacent. This is
  what makes the bound hold under skewed density, not just on average (see *Trade-offs*).
- The one case the packing cannot bound further: a single bucket whose own count already exceeds
  `target_rows_per_query`. The loop never splits a bucket, so it still forms one batch on its own and
  that query returns the bucket's full count — the same per-bucket behavior as today's per-hour loop,
  not a new hazard.

Because counts are per bucket rather than a range-wide average, each batch's row count is bounded by
`target_rows_per_query` — except the single-oversized-bucket case above, where the bound is that
bucket's own count instead. Buffered memory per batch is therefore bounded by that same row count,
each row a predictable width — ~150 B of Arrow columns after the lean projection, plus ~200 B for the
parsed `Arc<PartitionSourceBlock>` (`BlockMetadata` + the `format` `String` + two `Arc`s) held
simultaneously — so ~350 B/row, ~90 MB at the default target of 250k in the common case. Output does
not depend on batch width.

### Why the lean projection is in scope

A row *count* target only bounds memory if rows have a predictable width — which is also why the
per-bucket counts must be taken under the batch queries' own full predicate (see *Adaptive batch
width*): an
accurate count times an unpredictable row width still doesn't bound memory. With stream blobs in the
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
HashMap<Uuid, (Arc<StreamMetadata>, String /* format */)>
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
pub struct PartitionFreshnessRow { ... }

/// Filters `candidates` down to the rows the per-spec SQL returns today (exact range equality for
/// EventTime, exact match for a degenerate InsertTime range, inclusive overlap otherwise), then
/// applies today's rows.len() == 1 / file_schema_hash / object-count checks verbatim.
///
/// `pub`, with `PartitionFreshnessRow`'s fields `pub` too (or a `pub` constructor), so
/// `analytics/tests/jit_freshness_tests.rs` can build rows and call this directly.
pub fn spec_is_up_to_date(view_meta, spec, block_order, candidates: &[PartitionFreshnessRow]) -> Result<bool>
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
/// non-overlapping insert ranges), then the matcher per spec, run to a fixpoint. Round 1: run
/// `spec_is_up_to_date` for every spec against the full candidate set. Each later round: for any
/// spec `i` newly verdicted **not** up to date this round, drop from every other spec `j`'s
/// candidate set any row entirely contained in `i`'s insert range, and re-run the matcher for those
/// affected `j`s (see *Verdicts reflect pre-run state* below) -- such a row is a
/// `RetireMatch::Containment` match for spec `i` and will be gone once `i`'s write runs this call,
/// so it must not count towards `j`'s freshness. Repeat until a round flips no verdict: dropping a
/// row can only turn a spec from up-to-date to stale, never the reverse, so verdicts are monotone
/// and the loop terminates in at most `specs.len()` rounds (one, in the common case). A row is
/// dropped only when its containing spec is itself stale; specs whose containing spec is up to date
/// (hence not rewritten) are unaffected. Returns up-to-date flags parallel to `specs`.
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
batched, all verdicts are computed first. The touching-ranges corner (equal boundary insert-times
straddling a cut between specs *i* and *j*) has two directions, and only one is benign:

- *A this-run write matching a later spec.* Spec *i*'s write can satisfy spec *j*'s overlap
  predicate — a corner where today's interleaved check already misbehaves (it can double-match or
  wrongly adopt the neighbor). Deciding from pre-run state only is the saner semantics here.
- *A this-run retirement removing a row an earlier verdict depended on.* `RetireMatch::Containment`
  (all five `InsertTime` callers, `write_partition.rs:219-240`) deletes every partition contained in
  spec *i*'s range when *i* is written. If an existing partition touches spec *j*'s boundary and is
  also contained in spec *i*'s range, a pre-run verdict can judge spec *j* up to date against that
  partition, and later in the same run spec *i*'s write retires it — leaving spec *j*'s rows missing
  until the next `jit_update`. Today's interleaved order avoids this because spec *j*'s check runs
  after spec *i*'s write/retire and observes the removal. This direction is *not* benign, but the
  hazard only exists when spec *i* is itself rewritten this run: specs have non-overlapping,
  non-decreasing insert ranges (`group_blocks_into_partitions` docs, `:198-217`), so a candidate row
  entirely inside a *different* spec's range can only arise when that spec is degenerate
  (`min == max`) and sits on a boundary — and if *i* is already up to date, nothing retires the row,
  so dropping it from *j*'s candidate set unconditionally would report *j* stale for no reason, on
  every run. `find_up_to_date_partitions` therefore computes verdicts once against the full candidate
  set, then for any spec *i* newly verdicted **not** up to date, drops from every other spec's
  candidate set any row entirely contained in *i*'s range and re-evaluates those specs (see its
  rustdoc above) — such a row is guaranteed to be retired this run precisely because *i* is being
  rewritten, so it must not count towards a sibling spec's freshness, while a row contained in an
  already-up-to-date spec is left alone. A drop can itself flip a sibling spec *j* from up to date
  to stale — and *j*'s write would then retire rows a third spec *k*'s verdict depended on — so the
  drop-and-re-evaluate step repeats until a round changes no verdict; verdicts are monotone
  (up-to-date → stale only), so this fixpoint terminates in at most `specs.len()` rounds, one in the
  common case. "Never a missed one" holds only at that fixpoint: stopping after a single pass would
  let *k* be reported up to date while its supporting partition is retired in the same run. The cost
  is a handful of unnecessary rewrites in this corner, never a missed one, and never a permanent
  rewrite loop.

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

### Keeping (and dropping) the segment functions

`generate_stream_jit_partitions_segment` (`:406`) is **kept, not deleted**:
`rust/analytics/tests/thread_spans_ordering_db_test.rs` drives it directly from ~10 sites (single
query range, no bucket subdivision, asserting on returned specs). Deleting it would mean rewriting a
2000-line DB test suite for no benefit. It keeps its current `collect()`-based body as-is; its
projection is already lean (block columns plus `"streams.format"` only, `:422-429`) — there is no
lean/fat distinction on the stream side, so the batched stream path (`generate_stream_jit_partitions`,
rewritten below) uses the same projection, not a divergent one. The lean-projection work is
process-variant only (see *Why the lean projection is in scope*).

`generate_stream_jit_partitions` (the outer, `:465`, called from `thread_spans_view.rs`) is rewritten
to batch, same as the process variant — after the rewrite it no longer calls the `_segment` function
per bucket, so the `_segment` function's only remaining callers are the `_segment`-specific DB tests.
That leaves the production `BlockOrder::EventTime` batch-then-split path (the riskiest new logic —
grouping must stay strictly per bucket to preserve cut points) with no coverage from the existing
suite. See Testing Strategy for the added DB-gated test that drives the outer function directly and
asserts equivalence against the per-segment path.

`generate_process_jit_partitions_segment` (`:528`) has exactly one caller —
`generate_process_jit_partitions`, which this plan rewrites to batch inline — so its grouping call is
inlined into the batch loop and the function itself is **deleted**, taking its hand-inlined
`StreamMetadata` copy (`:577-616`) and per-row CBOR decode with it. Its fetch-and-parse half (SQL,
`block_from_batch_row`, the per-stream metadata lookup) survives as a new `pub async fn
fetch_process_blocks(...) -> Result<Vec<Arc<PartitionSourceBlock>>>`: the rewritten generator's
per-batch-window loop calls it once per window before `group_blocks_into_partitions`, and
`jit_process_batch_db_test.rs` calls it once over the whole test range to compute expected specs
without re-implementing the batch SQL and parsing under test (see *Testing Strategy*). Nothing in the
existing test suite calls the deleted segment function directly (the current DB test only drives the
stream variant), so deleting it requires no changes to existing tests.

### Instrumentation

Keep `instrument_named!` spans; replace the per-segment `collect_partition_blocks` span (`:568`)
with one per batch, log the derived batch width and row estimate once per call so the adaptive
decision is visible in traces, and give the candidates fetch in `find_up_to_date_partitions` its own
span with the spec count.

## Implementation Steps

1. `rust/analytics/src/lakehouse/jit_partitions.rs`:
   - Add a per-bucket `COUNT(*) ... GROUP BY date_bin(slice, insert_time)` query over the
     insert range, run after `fetch_overlapping_insert_range_for_view` resolves the `PartitionCache`
     (it can only be answered from that cache — see *Adaptive batch width*), using the same full
     predicate as the batch queries; add `target_rows_per_query: i64` to `JitPartitionConfig`
     (default `250_000`, rustdoc'd) and
     `pub fn batch_windows(insert_range, slice, bucket_counts: &[(DateTime<Utc>, i64)],
     target_rows_per_query) -> impl Iterator<Item = TimeRange>` (`bucket_counts` holds one
     `(bucket_start, nb_blocks)` pair per *non-empty* bucket from the `GROUP BY` query, ascending,
     empty buckets implicitly zero; greedily packs consecutive buckets up to
     `target_rows_per_query`, slice-aligned, last window clamped to the range end — see *Adaptive
     batch width*) — `pub` so `analytics/tests/jit_batch_windows_tests.rs` can call it directly, and
     `target_rows_per_query` threaded through `JitPartitionConfig` so DB-gated tests can lower it to
     force a multi-batch run (see *Testing Strategy*). Adding this field is technically a public-API
     change (any out-of-repo full struct literal for this `pub` struct breaks), and it breaks the 7
     existing exhaustive `JitPartitionConfig` literals in-repo: the `config()` helper in
     `analytics/tests/jit_partition_grouping_tests.rs` (`:72-76`) and the 6 literals in
     `analytics/tests/thread_spans_ordering_db_test.rs` (`:812, 1025, 1240, 1507, 1763, 2030`) — add
     the field (or switch each to `..Default::default()`) so the test crate keeps compiling.
   - Add the per-stream metadata pre-query for the process variant, decoded via
     `metadata::stream_metadata_from_batch_row`, into
     `HashMap<Uuid, (Arc<StreamMetadata>, String)>` (the `format` `String` is cloned per block off
     this map — `PartitionSourceBlock::format` is a plain `String`, so an `Arc<String>` here would
     only add an indirection, not save a clone).
   - Extract the process-variant batch SQL construction into
     `pub fn process_batch_sql(process_id, stream_tag, range) -> String` so the projection guard
     test can call it directly instead of reaching into a private `format!`.
   - Rewrite the two generators' step-3 loops: iterate `batch_windows`, collect each batch, split
     rows into buckets on `duration_trunc` change, `group_blocks_into_partitions` per bucket.
     Signatures unchanged. Extract the process variant's fetch-and-parse logic into a new `pub
     async fn fetch_process_blocks`, delete `generate_process_jit_partitions_segment` (dead after
     the rewrite, its grouping call now inlined; see *Keeping (and dropping) the segment
     functions*); `generate_stream_jit_partitions_segment` is left in place for the DB test.
   - Split `is_jit_partition_up_to_date` into a candidates fetch (inclusive insert-range overlap)
     plus the pure matcher `pub fn spec_is_up_to_date` operating on `pub struct
     PartitionFreshnessRow`; keep `is_jit_partition_up_to_date`'s public signature and behavior.
     Both are `pub` so `analytics/tests/jit_freshness_tests.rs` can call them directly. Add
     `find_up_to_date_partitions`.
2. Update the five `InsertTime` call sites (`log_view.rs`, `metrics_view.rs`, `images_view.rs`,
   `async_events_view.rs`, `otel/spans_view.rs`) to call `find_up_to_date_partitions` once and skip
   flagged specs. `net_spans_view.rs` / `thread_spans_view.rs` are untouched.
3. Add `rust/analytics/tests/jit_batch_windows_tests.rs` and
   `rust/analytics/tests/jit_freshness_tests.rs`, and add DB-gated, `#[ignore]`d tests covering
   `generate_process_jit_partitions` and the outer `generate_stream_jit_partitions` (see Testing
   Strategy), constructing `JitPartitionConfig` with a lowered `target_rows_per_query` so those runs
   are forced to split into more than one batch, not just more than one bucket within a single batch.
4. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, then
   `cargo test -- --ignored` with `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`
   set, to actually run the DB-gated suite (see Testing Strategy).
5. Manual verification — see Testing Strategy.

## Files to Modify

- `rust/analytics/src/lakehouse/jit_partitions.rs` — main change.
- `rust/analytics/src/lakehouse/{log_view,metrics_view,images_view,async_events_view}.rs`,
  `rust/analytics/src/lakehouse/otel/spans_view.rs` — one `find_up_to_date_partitions` call each.
- `rust/analytics/tests/jit_batch_windows_tests.rs`, `rust/analytics/tests/jit_freshness_tests.rs`
  — new, pure logic.
- `rust/analytics/tests/jit_process_batch_db_test.rs` — new, DB-gated (`#[ignore]`d) test driving
  `generate_process_jit_partitions` over a multi-bucket range under both `BlockOrder` variants,
  covering the batched process path (including the `EventTime` batch-then-split path used in
  production by `net_spans`) and the lean projection.
- `rust/analytics/tests/thread_spans_ordering_db_test.rs` — gains one new DB-gated test driving the
  outer `generate_stream_jit_partitions` over a multi-bucket range, asserting equivalence against the
  per-segment path (see Testing Strategy); existing per-segment tests are otherwise unchanged
  (`generate_stream_jit_partitions_segment` kept for those).

## Trade-offs

- **Adaptive width vs. a fixed `TimeDelta`.** A constant is wrong at both ends: a day still costs 30
  queries for a month of near-empty OTLP metrics, while for a busy many-threaded process it buffers
  far more than intended. Deriving it from a `COUNT(*)` scoped to the batch queries' own predicate
  makes query count scale with data volume instead of wall-clock range. Costs one extra round trip
  (it cannot piggyback on pre-query 1, whose predicate differs — see *Adaptive batch width*). No
  pin/override knob: if the derivation is wrong, fix the derivation.
- **Per-batch bound vs. an average-density estimate.** An earlier version of this design derived one
  width from a single range-wide `COUNT(*)`, which is only a *mean* density and does not hold under a
  skewed one: a burst concentrated inside a wide, mean-derived window can return far more than
  `target_rows_per_query`. Counting per bucket and packing greedily (see *Adaptive batch width*)
  bounds each batch's row count directly, by construction, regardless of how unevenly blocks are
  distributed within the counted range — a burst closes its own batch as soon as its running count
  would exceed the target, rather than being diluted by the empty buckets around it. The one residual
  case is a single bucket whose own count exceeds the target: it still forms its own batch, matching
  today's per-bucket behavior rather than introducing a new hazard. The buffered unit is bounded by
  `target_rows_per_query` (~90 MB at the default target, see *Adaptive batch width*) — higher than
  today's per-hour peak for a mid-density instance, but a fixed, predictable ceiling — and width
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

- **Streaming `JitPartitionAccumulator`** (the original draft, removed by commit `878836979` when
  this plan was rewritten against current code): predated #1429/#1440, which replaced the inline
  chunking it targeted with
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
- **Projection guard**: call the `pub fn process_batch_sql` builder (added in Implementation Steps)
  and assert its `SELECT` list contains no `streams.` column (the `WHERE` clause still may). One
  small string assertion — it guards the exact regression the rustdoc warns about.
- **Existing suites**: `cargo test` from `rust/` must pass, once the 7 existing exhaustive
  `JitPartitionConfig` struct literals are updated for the new field (see *Implementation Steps*) —
  that update is required for the test crate to compile at all, not merely for behavior. No test's
  *assertions* change. `thread_spans_ordering_db_test.rs`
  drives only `generate_stream_jit_partitions_segment` directly (single-segment, no bucket
  subdivision) — grep confirms nothing in `rust/analytics/tests/` calls the outer
  `generate_stream_jit_partitions` or `generate_process_jit_partitions`. After this plan's rewrite,
  the production stream path (`thread_spans_view.rs`) routes through the batched
  `generate_stream_jit_partitions`, not the segment function, so that suite pins cut-point behavior
  for a function the production stream path no longer calls, and the new client-side
  `duration_trunc` bucket-splitting under `BlockOrder::EventTime` — the riskiest new logic, since
  grouping must stay strictly per bucket to preserve cut points — is exercised by nothing. Every
  test in `thread_spans_ordering_db_test.rs` is `#[ignore]`d and needs a live
  `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`; run explicitly with
  `cargo test -- --ignored`. Add `jit_process_batch_db_test.rs`, a DB-gated test (same `#[ignore]`
  gating) driving `generate_process_jit_partitions` directly over a multi-bucket range, using a
  `JitPartitionConfig` with a lowered `target_rows_per_query` so the run is forced to span more than
  one batch (not just more than one bucket within a single batch), under **both** `BlockOrder`
  variants (`InsertTime` and `EventTime`) — e.g. a `view_instance('log_entries', <process>)` query
  for `InsertTime`, plus a second run with `block_order: BlockOrder::EventTime` in the
  `JitPartitionConfig` — so the batched process path and the lean projection get automated coverage
  under both orderings. `BlockOrder::EventTime` is production-reachable for the process variant too
  (`net_spans_view.rs:351-364` builds `JitPartitionConfig { block_order: BlockOrder::EventTime, .. }`
  and passes it to `generate_process_jit_partitions`), so the client-side `duration_trunc`
  batch-then-split path under `EventTime` is the same riskiest-new-logic case as the stream side and
  needs the same coverage; unlike the stream side, `generate_process_jit_partitions_segment` is
  deleted by this plan (see *Keeping (and dropping) the segment functions*), so the test instead
  calls the new `pub fetch_process_blocks` helper — the same fetch/parse code the rewritten
  generator's per-batch loop calls — once over the whole test range, and computes expected specs by
  running `group_blocks_into_partitions` per bucket over its result. This shares the fetch/parse SQL
  path with the code under test instead of duplicating it, so the assertion is not circular.
  Additionally, add a DB-gated test driving `generate_stream_jit_partitions` (not the `_segment`
  function) over a multi-bucket range under `BlockOrder::EventTime`, again with a lowered
  `target_rows_per_query` to force more than one batch, asserting its emitted specs are identical to
  running `generate_stream_jit_partitions_segment` once per bucket — this is the only coverage of
  the new batch-then-split path for the `EventTime` variant, and it can live alongside the existing
  per-segment tests in `thread_spans_ordering_db_test.rs`.
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
  process over a long range, watching process RSS (system_monitor gauges, #1330). Expected: peak
  buffered memory per batch bounded by `target_rows_per_query` (~90 MB at the default target, see
  *Adaptive batch width*), except the residual single-oversized-bucket case, which still buffers that
  bucket's full count in one query — today's per-bucket behavior, not a new hazard. This matches (or
  improves on, from the shared per-stream `Arc`s) today's per-hour peak only when the instance is
  dense enough that most individual buckets' own counts are at or above `target_rows_per_query` (so
  batches collapse to one bucket each); for a mid-density instance below that, the batched path
  buffers more per query than today's per-hour loop — an accepted, bounded trade for far fewer
  queries, not a regression to chase down.

## Documentation

No mkdocs page covers JIT partition internals (verified by grep). Rustdoc on
`JitPartitionConfig::target_rows_per_query` and the batch-window derivation, the per-stream metadata
pre-query, the lean-projection invariant, the freshness matcher split, and the rewritten loop bodies
suffices.
