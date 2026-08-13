# Thread-Spans Partition Event-Bound Overlap Plan (sort-key bounds fix)

Tracked by https://github.com/madesroches/micromegas/issues/1478.

## Overview

`thread_spans` queries that span a JIT segment (1-hour insert-time bucket) boundary fail loudly
with `declared scan ordering violated`: the two partitions on either side of the boundary get
event-time bounds that overlap by a few dozen microseconds, violating `ThreadSpansView`'s declared
`ScanOrdering::Concatenated` invariant. Retiring and rebuilding the affected partitions does not
fix it — the same seam reproduces the same overlap on every rebuild.

The root cause is deeper than the segment seam. `micromegas_tracing`'s flush path stamps the
replacement block's `begin` *before* closing the outgoing block (`rust/tracing/src/dispatch.rs:824-834`;
`EventBlock::new` stamps `begin: DualTime::now()` first, `rust/tracing/src/event/block.rs:61-68`),
and because `begin` is stamped at flush time an idle thread produces no tick gap either. So **every
consecutive block pair in a healthy thread stream strictly overlaps in ticks** — the stream is one
unbroken overlapping chain from process start to exit, broken only by dropped blocks.

This overlap is a producer bug relative to the design intent, not a contract. The point of making
consecutive blocks share a boundary timestamp is to *clearly mark* them as consecutive so call
trees can be merged seamlessly across blocks — and the reference implementation of that intent is
the Unreal producer, whose `FlushThreadStream` takes a single `DualTime::Now()` and uses it for
both the replacement block's `begin` and the outgoing block's `Close`
(`unreal/MicromegasTracing/Private/Dispatch.cpp:197-212`), so its blocks *touch exactly*. The Rust
flush paths stamp two separate times where Unreal stamps one; the ~40µs gap between the two stamps
is the overlap. Several rustdocs (`jit_partitions.rs`, `view.rs`, `1429`) currently describe the
Rust overlap as expected producer behavior — they describe the bug, not the design.
`tasks/completed/1429_jit_event_time_block_ordering_plan.md` (Implementation Status, "Steps 16-17")
already stated this precisely: "Any cut between adjacent blocks can trip
`sort_and_check_non_overlapping`'s strict `prev_max > next_min`" — the hour-bucket seam is just the
cut that happens on every stream; a `max_nb_objects`-forced cut inside a bucket (~5,800 events/s
sustained on one thread for an hour, well within the ~100k events/s per process the README
advertises) trips the identical failure.

**The key insight: the overlap is a metadata artifact, not a data one.** A partition's declared
event bounds come from its blocks' `begin_ticks`/`end_ticks` (`thread_spans_view.rs:201-214`), and
the check compares the previous partition's `max_event_time` (max block *end*) against the next's
`min_event_time` (min block *begin*). But the actual rows never overlap on the sort column:

- Thread streams are single-writer — every flush path reaches `flush_thread_buffer` through the
  `thread_local!` `LOCAL_THREAD_STREAM` (`dispatch.rs:215-243, 396-409`), so the owning thread
  cannot push events during the swap. Every `begin` event in the closing block precedes the
  replacement block's `begin` stamp.
- The call-tree builder never synthesizes a row `begin` beyond a real event timestamp: events
  outside the chain range are dropped, not clamped in (`rust/analytics/src/call_tree.rs:139-144,
  164-169`); a span open at chain end keeps its real `begin` (`:145-151`); a span open at chain
  start has `begin` clamped *down* to the chain begin (`:194-201`).
- Rows are emitted in preorder (`span_table.rs:126-146, 171-186`), verified non-decreasing on
  `begin` at write time (`ensure_begin_non_decreasing`, `thread_spans_view.rs:131-152`, called at
  `:218`).

So the true maximum `begin` of any partition is strictly less than the next partition's first
block's `begin_ticks` — the concatenation is already correct; only the declared bound lies.

**The fix has two halves:**

- **Part A — producer**: align the Rust flush paths with the design (and with Unreal): take one
  `DualTime::now()` per flush and use it for both the outgoing block's close and the replacement
  block's `begin`, so consecutive blocks touch exactly. Correct data going forward; chain
  detection (`group_contiguous_block_chains` keeps touching blocks connected) is unaffected.
- **Part B — server resilience**: record each partition's true maximum leading-sort-column value
  (`max_sort_key_time`, = the last row's `begin`, since rows are verified non-decreasing) as new
  nullable partition metadata, and change the `Concatenated` non-overlap check to compare
  `prev.max_sort_key_time` (falling back to `max_event_time` when NULL, i.e. for partitions
  written before this change) against `next.min_event_time`. The
  `[min_event_time, max_event_time]` pair keeps its `[min begin, max end]` semantics for partition
  pruning, untouched. The JIT grouping layer (`jit_partitions.rs`) is not modified at all: hourly
  buckets, `max_nb_objects` cuts, and freshness behavior stay exactly as they are — this fixes
  hour-seam cuts *and* forced intra-bucket cuts uniformly, and after a retire, rebuilt partitions
  genuinely stop failing (unlike today).

Part A alone is not enough: every block already ingested keeps its overlapping tick geometry until
it ages out of retention (rebuilding partitions from it reproduces the failure identically), and
instrumented applications ship with pinned `micromegas-tracing` versions, so the analytics service
will keep receiving strictly-overlapping blocks from older clients long after the crate is fixed.
Part B is what makes the server robust to both. Conversely, Part B alone would leave the data
wrong relative to its own design intent and the docs describing a bug as a contract.

A previous draft of this plan fixed only the segment seam by merging hour buckets whose boundary
fell inside a strict tick overlap. Review showed that approach degenerates for the actual producer
— see Trade-offs, "Rejected alternative".

## Repro Steps

### Against a running local stack (what surfaced this)

1. `python3 local_test_env/ai_scripts/start_services.py` (split mode; monolith works too).
2. `python3 local_test_env/ai_scripts/run_generator.py` — runs `telemetry-generator` with CPU
   tracing enabled (`MICROMEGAS_ENABLE_CPU_TRACING`), which is what emits the `thread_spans`
   source data.
3. Run it again a few times over more than an hour of wall-clock, or wait for an existing
   `thread_spans` stream's data to span an hour boundary (JIT partitions are built lazily per
   query, so the boundary only matters once a query range crosses it).
4. Query across the boundary, e.g. via the `micromegas` Python client or `micromegas-query`:
   ```sql
   SELECT stream_id, "streams.tags", nb_objects FROM blocks
   WHERE array_has("streams.tags", 'cpu') ORDER BY nb_objects DESC LIMIT 1;
   -- then, with that stream_id:
   SELECT target, name, duration, begin, "end"
   FROM view_instance('thread_spans', '<stream_id>')
   ORDER BY duration DESC LIMIT 10;
   ```
   (This is exactly `python/micromegas/tests/test_queries.py::test_spans`.)
5. Observe `FlightInternalError: ... declared scan ordering violated: partition ...(range ending
   HH:59:24.6159xx) overlaps partition ...(range starting HH:59:24.6159yy)` — a ~40µs overlap
   straddling the `:00` hour mark, naming two `views/thread_spans/<stream_id>/...` files.
6. Confirm retiring doesn't help today: as admin,
   `SELECT * FROM retire_partitions('thread_spans', '<stream_id>', '<day>T00:00:00Z', '<day+1>T00:00:00Z')`
   (requires `is_admin`; the CLI's default connection is anonymous, so drive it via the Python
   client directly, or an admin-configured profile). Re-running the query in step 4 reproduces the
   identical failure shape against freshly-rebuilt partitions — the block tick geometry is
   reproduced identically on every rebuild. (After this plan lands, the same retire *does* fix it:
   rebuilt partitions carry `max_sort_key_time`.)

### Deterministic, no live generator needed (basis for the regression test)

The `rust/analytics/tests/thread_spans_ordering_db_test.rs` harness already has everything needed:
push blocks into one `ThreadStream` via `push_and_insert_block` (`:73`), then directly
`UPDATE blocks SET begin_ticks = $1, end_ticks = $2, insert_time = $3 WHERE stream_id = $4 AND
object_offset = $5` to place them precisely — the pattern used by
`thread_spans_degenerate_range_retires_stale_partition` (`:697`, SQL at `:738`) and six other
tests in that file. Minimal repro shape:

- Block A: `begin_ticks = 0, end_ticks = 1000`, `insert_time` a few seconds before an hour mark
  (e.g. `hour_mark - 2s`).
- Block B: `begin_ticks = 999, end_ticks = 2000` (one tick of *strict* overlap with A — the
  buffer-swap signature), `insert_time` a few seconds after the same hour mark (e.g.
  `hour_mark + 2s`).
- Drive `generate_stream_jit_partitions` + `update_partition` (both `pub` for exactly this kind of
  test) over a range spanning both hours.
- Today: two partitions are emitted (one per hour bucket), and `partition(A).max_event_time` >
  `partition(B).min_event_time` because the bounds are block-tick-derived — the live
  `view_instance('thread_spans', ...)` query fails. After the fix: the same two partitions are
  emitted, but partition A's recorded `max_sort_key_time` (its last row's `begin`, ≤ tick 999's
  time) no longer exceeds partition B's `min_event_time`, and the query succeeds.

## Current State

### Where the check reads its bounds

`sort_and_check_non_overlapping`
(`rust/analytics/src/lakehouse/partitioned_execution_plan.rs:63-92`) sorts the scan's non-empty
partitions by their leading bound and fails if any adjacent pair has `prev_max > next_min`
(strict). Both bounds come from `partition_bounds` (`:36-44`), which for
`OrderingBounds::EventTime` returns `(min_event_time, max_event_time)`. The same pair also feeds
`attach_ordering_statistics` (`:99-115`), which attaches `Precision::Inexact` min/max column
statistics for the leading sort column (`begin`) — note `max_event_time` (max span *end*) is
already a loose stand-in for the `begin` column's max there.

`sort_and_check_non_overlapping` has exactly one caller (`:289`, the
`ScanOrdering::Concatenated` arm of `make_partitioned_execution_plan`), reached from two places:
`MaterializedView::scan` (`materialized_view.rs:86-95`) and `PartitionedTableProvider::scan`
(`partitioned_table_provider.rs:82`). The only view declaring `Concatenated` with
`OrderingBounds::EventTime` is `ThreadSpansView` (`thread_spans_view.rs:434-441`). `net_spans`
declares **no** scan ordering (`net_spans_view.rs:349-351` documents this explicitly), and the
only `Concatenated` merge path is `BlocksView`'s, with `OrderingBounds::InsertTime`
(`blocks_view.rs:81-87`, `merge.rs:276-295`). So exactly one view needs to populate the new
metadata; every other writer can leave it NULL.

### Where partitions get their bounds and rows

`ThreadSpansView`'s `write_partition` (`thread_spans_view.rs:150-256`) builds one call tree per
unbroken block chain (`group_contiguous_block_chains`), appends them in preorder into a
`SpanRecordBuilder`, verifies the resulting batch's `begin` column is non-decreasing
(`ensure_begin_non_decreasing`, `:131-152`, called at `:218`), and sets `rows_time_range` from the
partition's block ticks — `[min begin_ticks, max end_ticks]` (`:201-214`). That range flows through
`PartitionRowSet` (`write_partition.rs:53-66`) into `write_rows_and_track_times`
(`write_partition.rs:626-675`, min/max fold at `:638-647`), `PartitionWriteResult` (`:617-623`),
`finalize_partition_write` (`:678-772`), the `Partition` literal (`:884-894`), and the INSERT
(`:529-546`) — landing in `lakehouse_partitions.min_event_time`/`max_event_time`.

### The persistence layer

`lakehouse_partitions` currently has 14 columns (base table `migration.rs:116-129`; `num_rows`
added v3, `partition_format_version` v5, `sort_order TEXT[]` v7). Schema version lives in
`lakehouse_migration` (single row), `LATEST_LAKEHOUSE_SCHEMA_VERSION = 7` (`migration.rs:8`),
applied by an incremental ladder (`migration.rs:53-106`) under advisory lock. The v6→v7 migration
(`migration.rs:435-517`) is the exact precedent for this plan's column: nullable, no backfill,
"NULL means 'no guarantee', which is automatically correct for every partition written before this
column existed" (`:436-441`).

`Partition` (`partition.rs:8-30`) is built from DB rows in exactly three functions / four SELECTs
(`partition_cache.rs:66-83`, `:148-165`, `:359-379`, `:394-412`; struct builds at `:116-126`,
`:200-210`, `:446-456`) and written in one INSERT (`write_partition.rs:529-546` — a bare positional
`VALUES($1,...,$12, 2, $13)` with the literal `2` being `partition_format_version`).
`list_partitions()` additionally mirrors the table to SQL with its own Arrow schema
(`list_partitions_table_function.rs:52-89`) and two SELECTs (`:113-129`, `:131-146`).

The existing `sort_order` column cannot carry this value: it is a `TEXT[]` of column *names*
consumed only by `certifies_sort_order`/the `PerFile` gate (`partition.rs:65-82`,
`partitioned_execution_plan.rs:266-275`) — and `thread_spans` writes it as NULL anyway
(`thread_spans_view.rs:179`). A new column is required.

### What is explicitly NOT touched

`jit_partitions.rs` — bucketing (`split_into_buckets`), grouping
(`group_blocks_into_partitions`), batching (`batch_windows`), and freshness
(`spec_is_up_to_date`/`is_jit_partition_up_to_date`) are all unchanged. Partition specs stay
hourly and query-range-independent, so there is no new rebuild churn and the existing equivalence
test (`thread_spans_batched_generation_matches_per_segment`,
`thread_spans_ordering_db_test.rs:2120`) is unaffected by construction.

## Design

### Part A — producer: one timestamp per flush (micromegas_tracing)

Mirror Unreal's `FlushThreadStream` shape in the four Rust flush paths (`dispatch.rs` —
thread `:820-835`, log `:800-817`, metrics `:~703`, image `:~561`): take `let now =
DualTime::now();` once, pass it to the replacement block's constructor, and close the outgoing
block with the same value. Mechanically:

- `EventBlock::close` (`rust/tracing/src/event/block.rs:18-20`) gains a time parameter (e.g.
  `close_at(&mut self, end: DualTime)`, keeping a self-stamping `close()` for the
  shutdown/no-replacement paths if convenient).
- `TracingBlock::new` / `EventBlock::new` (`block.rs:33-38, 55-69`) gain a `begin: DualTime`
  parameter instead of stamping internally. The ripple is mechanical: the four dispatch flush
  sites, initial block creation in `EventStream::new` (`event/stream.rs:52`, which keeps stamping
  its own now — a stream's first block has no predecessor), and test/bench constructors — concretely
  `ThreadBlock::new`/`LogBlock::new`/`MetricsBlock::new`/`ImageBlock::new` call sites in
  `rust/analytics/benches/parse_block.rs`; nine files under `rust/analytics/tests/` (`span_tests.rs`,
  `parse_block_tests.rs`, `parse_corrupt_block_tests.rs`, `parse_alloc_test.rs`, `log_tests.rs`,
  `metrics_test.rs`, `image_tests.rs`, `jit_process_batch_db_test.rs`,
  `thread_spans_ordering_db_test.rs`); and `rust/telemetry-sink/tests/http_event_sink_transport_tests.rs`
  (which also calls `block.close()` at `:169, :176, :183` and needs the `close_at`/`close` update).
- Only the thread path affects `thread_spans` ordering, but all four paths get the same treatment
  for consistency — the log/metrics/images views are `InsertTime`-bounded and indifferent.

Semantics are preserved: `group_contiguous_block_chains` already treats *touching* blocks as
chain-connected (it breaks only on `begin_ticks > running_end`), so call trees still merge
seamlessly across the seam — which is the entire point of the shared boundary stamp. The
analytics-side equivalence test's synthetic data already touches exactly, matching the new
producer behavior.

Add a `rust/tracing` unit test: push events across a forced flush and assert the closed block's
`end` equals the replacement block's `begin` (ticks and wall-clock both — they come from one
`DualTime`).

### Part B — server: record the true sort-key bound

### 1. Schema: `max_sort_key_time` column (v7 → v8)

Add a nullable `max_sort_key_time TIMESTAMPTZ` column to `lakehouse_partitions` — the maximum
value of the view's declared `Concatenated` leading sort column across the partition's rows
(`begin` for `thread_spans`). NULL means "not recorded" (any partition written before v8, and any
view that doesn't declare a `Concatenated` event-time ordering). No backfill, following the v6→v7
precedent verbatim; bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 8.

### 2. Carry it through the write path

- `PartitionRowSet` (`write_partition.rs:53-66`) gains
  `max_sort_key_time: Option<DateTime<Utc>>`. Keep `new()` two-argument (sets `None`) so the nine
  `::new` call sites are untouched. Five sites build the struct as a literal instead: it's set at
  `thread_spans_view.rs:221-224`, and `..None`/`max_sort_key_time: None` must be added at the four
  mechanical sites in `net_spans_view.rs:208`, `async_events_block_processor.rs:157`,
  `log_block_processor.rs:68`, and `image_block_processor.rs:81` (or have them switch to
  `PartitionRowSet::new`).
- `write_rows_and_track_times` (`:626-675`) folds a running value alongside the existing min/max
  fold. Soundness rule: the partition-level value is `Some(max)` **only if every** received row
  set carried `Some`; any `None` poisons the whole partition to `None`. (It must be a running
  `max`, never "last row set wins": `block_partition_spec.rs:141` streams row sets out of order
  via `buffer_unordered`.) The function is `pub` and used by
  `rust/analytics/tests/write_partition_tests.rs:34`; widen its return type to a small struct and
  fix that test.
- Thread through `PartitionWriteResult` (`:617-623`; three construction sites at `:712-717`,
  `:727-732`, `:765-770`) into the `Partition` literal (`:884-894`) and the INSERT. While touching
  the INSERT (`:531`), convert it to an **explicit column list** — the bare positional `VALUES`
  with a hardcoded `2` mis-binds silently if a column is ever added anywhere but last.

### 3. Populate it in `thread_spans_view`

In `write_partition`, after `ensure_begin_non_decreasing` passes, the max `begin` is simply the
last row's value (rows are verified non-decreasing) — read it from the `begin` column and set it
on the emitted `PartitionRowSet`. An empty batch emits no row set, so the empty case never arises.

### 4. Read it back

- `Partition` (`partition.rs:8-30`) gains `max_sort_key_time: Option<DateTime<Utc>>` with an
  accessor. Follow the `sort_order` read pattern — `r.try_get("max_sort_key_time")?`, NULL → `None`
  (`partition_cache.rs:125/209/455`); do **not** copy `file_path`'s `.ok()` pattern, which swallows
  decode errors.
- Add the column to the four `Partition`-building SELECTs (`partition_cache.rs:66`, `:148`,
  `:359`, `:394`) and to `list_partitions()`'s Arrow schema + both SELECTs
  (`list_partitions_table_function.rs:52-89`, `:113`, `:131`) — its `rows_to_record_batch` is
  generic, so schema and SELECT must stay in lockstep.
- Extend `Partition::validate` (`partition.rs:90-117`) with the cheap invariant: when both are
  present, `min_event_time <= max_sort_key_time <= max_event_time`.

### 5. Change what the check compares

In `partition_bounds` (`partitioned_execution_plan.rs:36-44`), the `OrderingBounds::EventTime` arm
returns the upper bound as `max_sort_key_time` when present, falling back to `max_event_time` when
NULL:

```rust
OrderingBounds::EventTime => p
    .min_event_time()
    .zip(p.max_sort_key_time().or(p.max_event_time())),
```

This is a single change point that upgrades all three consumers coherently:
- the sort key stays `min_event_time` (unchanged);
- `sort_and_check_non_overlapping`'s `prev_max > next_min` now compares the previous partition's
  *true* max `begin` against the next's block-derived min — which the swap-window argument in the
  Overview shows can never strictly overlap for cuts at block boundaries of this producer;
- `attach_ordering_statistics` attaches a *tighter and actually-correct* max statistic for the
  `begin` column (both values are legal under `Precision::Inexact`; the recorded one is exact).

The fallback preserves today's behavior bit-for-bit for legacy partitions and for any view that
never records the value — and, unlike today, retiring a failing legacy `thread_spans` partition
now genuinely fixes it, because the rebuild records `max_sort_key_time`.

### Why this is sound

- **Hour-seam cuts and forced intra-bucket cuts are the same case.** Any cut between adjacent
  blocks of a thread stream puts blocks `[..k]` in partition P and `[k..]` in partition Q. Q's
  `min_event_time` is block `k`'s `begin_ticks` (stamped at the swap that closed block `k-1`).
  Every `begin` event in P's blocks was pushed before that stamp (single-writer, see Overview),
  and `make_call_tree` never moves a `begin` later than a real event — so P's `max_sort_key_time`
  < Q's `min_event_time`, always, regardless of *why* the cut happened.
- **Both producers are covered.** The Unreal instrumentation also feeds `thread_spans` (its
  `ThreadStream`s are `cpu`-tagged, `unreal/MicromegasTracing/Private/Dispatch.cpp:222-225`) but
  cannot produce the overlap: `FlushThreadStream` uses a single `DualTime::Now()` for both the
  replacement block's `begin` and the outgoing block's `Close` (`Dispatch.cpp:197-212`), so
  consecutive blocks touch exactly and the strict `>` check passes. The single-writer premise
  holds there too: `FlushMonitor::Flush` only `MarkFull()`s other threads' streams
  (`unreal/MicromegasTelemetrySink/Private/FlushMonitor.cpp:45-54`); the actual swap always runs
  on the owning thread at its next event push (`QueueThreadEvent`, `Dispatch.cpp:396-411`), so no
  event in a closing block is stamped after the close and `max_sort_key_time <=
  next.min_event_time` holds with equality at worst. (Unreal's lazily-created thread streams can
  timestamp their *first* event before the first block's `begin` — `Dispatch.cpp:449-453` — but
  `make_call_tree` drops out-of-range events rather than clamping them in, `call_tree.rs:139-141`,
  so no row escapes its partition's bounds.)
- **Conservative.** The check can only get *stricter* facts, never falser ones: the recorded value
  is exact, the fallback is today's over-wide bound. A genuine row-level inversion — cause (2)
  below — still fails loudly.
- **Residual caveats (unchanged in kind, now two instead of three):** (2) an insert-time inversion
  straddling a segment boundary can produce *real* row overlap across partitions, which this check
  correctly still rejects; (3) TSC-frequency re-estimation drift across materialization epochs
  (`tsc_frequency == 0` processes) can skew bounds written under different converters — both
  values in the new comparison are converter-derived, so drift behaves exactly as it does today
  and retiring (rebuilding under one converter) remains the fix.

### Data flow after the change

```
thread_spans write_partition
   rows (preorder, verified non-decreasing on `begin`)
        │
        ├── rows_time_range = [min block begin_ticks, max block end_ticks]   -- unchanged (pruning)
        └── max_sort_key_time = last row's `begin`                           -- NEW (exact)
        ▼
PartitionRowSet → write_rows_and_track_times (all-Some running max) → INSERT
        ▼
lakehouse_partitions.max_sort_key_time (NULL for legacy rows / other views)
        ▼
partition_bounds(EventTime) = (min_event_time, max_sort_key_time OR max_event_time)
        ▼
sort_and_check_non_overlapping: prev_max > next_min  → never fires for buffer-swap seams
```

## Implementation Steps

1. **Producer fix (Part A)**: single `DualTime::now()` per flush in the four `dispatch.rs` flush
   paths; `close_at`/`begin` parameter plumbing in `event/block.rs` (+ the mechanical ripple in
   `event/stream.rs` and test/bench block constructors); new `rust/tracing` unit test asserting
   closed `end` == replacement `begin`.
2. **Migration**: add `upgrade_v7_to_v8` in `rust/analytics/src/lakehouse/migration.rs` (nullable
   `max_sort_key_time TIMESTAMPTZ`, no backfill, comment stating NULL = "not recorded", modeled on
   `upgrade_v6_to_v7`'s column-add); bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 8.
3. **`Partition` + reads**: new field/accessor + `validate` invariant (`partition.rs`); add the
   column to the four SELECTs and three struct builds in `partition_cache.rs`; extend
   `list_partitions_table_function.rs` (Arrow schema + both SELECTs).
4. **Write path**: `PartitionRowSet` field; `write_rows_and_track_times` all-Some running-max fold
   + widened return struct (fix `rust/analytics/tests/write_partition_tests.rs`);
   `PartitionWriteResult`; explicit-column-list INSERT with the new bind.
5. **Populate**: `thread_spans_view.rs::write_partition` sets `max_sort_key_time` from the last
   row's `begin` after `ensure_begin_non_decreasing`.
6. **Check**: change `partition_bounds`'s `EventTime` arm per Design Part B §5.
7. **Docs**:
   - `view.rs:166-187` (`Concatenated` contract): rewrite the residual-caveats paragraph — the
     block-boundary tick overlap (old caveat 1) no longer trips the check for partitions carrying
     `max_sort_key_time`; remaining residuals are legacy-NULL partitions (retire to fix), the
     insert-time inversion (2), and TSC drift (3).
   - `partitioned_execution_plan.rs`: `sort_and_check_non_overlapping` rustdoc + runtime error
     string (`:52-59`, `:82-88`) — the "retire to fix" advice becomes accurate for all remaining
     causes; drop the buffer-swap cause except as a legacy-partition note. `partition_bounds` and
     `attach_ordering_statistics` doc updates.
   - `jit_partitions.rs` module doc (`:17-23`): amend the "says nothing about event-time ranges"
     note to point at `max_sort_key_time` as the mechanism that makes block-tick overlap harmless
     to the scan check.
   - **Reframe "overlap by design" as "legacy producer bug"** everywhere the Rust overlap is
     described as expected behavior: `group_contiguous_block_chains`'s rustdoc
     (`jit_partitions.rs:127-141` — the chain rule itself stays tolerant, since legacy blocks
     exist), the producer comparison in `view.rs:170-179`, and the `jit_partitions.rs` module doc.
     After Part A, both producers touch exactly by design; strict overlap identifies data from
     pre-fix `micromegas-tracing` versions.
   - `partition.rs` / `write_partition.rs`: field and fold-rule rustdoc (including why any-None
     poisons the partition value).
8. **Unit tests (no DB)**: extend `rust/analytics/tests/thread_spans_ordering_tests.rs`
   (`make_partition` helper at `:36-51`): (a) two partitions whose `[min,max_event_time]` overlap
   but whose `max_sort_key_time` clears the next `min_event_time` → accepted; (b) same shape with
   NULL `max_sort_key_time` → still rejected (legacy fallback preserved); (c) genuine overlap in
   `max_sort_key_time` → rejected. Update the other four test files with `Partition` literals
   (`per_file_scan_ordering_tests.rs`, `blocks_view_merge_ordering_tests.rs`,
   `sql_batch_view_merge_ordering_tests.rs`, `log_stats_ordering_tests.rs` — one helper each) for
   the new field.
9. **DB regression tests** (new `#[ignore]`d `#[tokio::test]`s in
   `thread_spans_ordering_db_test.rs`, following `thread_spans_degenerate_range_retires_stale_partition`'s
   `push_and_insert_block` + `UPDATE blocks` pattern):
   - **Hour-seam**: the two-block deterministic repro above; assert two partitions are emitted,
     each row's stored `max_sort_key_time` is non-NULL, and a live
     `view_instance('thread_spans', ...)` query across the boundary succeeds; also assert row
     completeness (union of partition rows covers both blocks' spans).
   - **Forced cut**: same stream, blocks overlapping pairwise within ONE hour bucket, driven
     through `generate_stream_jit_partitions` + `update_partition` with a custom
     `JitPartitionConfig { max_nb_objects: <small>, .. }` (the file already does this for the
     batched-equivalence test) so the cut lands inside the bucket. This test cannot assert through
     a live `view_instance('thread_spans', ...)` query: `ThreadSpansView::jit_update`
     (`thread_spans_view.rs:371-374`) always runs `JitPartitionConfig::default()`
     (`max_nb_objects = 20 * 1024 * 1024`) ahead of any scan, so it would see the small-`max_nb_objects`
     partitions as stale by source hash and rewrite them as one partition before the query ever
     ran the `Concatenated` check — passing vacuously without exercising the intra-bucket cut.
     Instead, read the written partitions back from `PartitionCache` and assert directly against
     `make_partitioned_execution_plan`'s `ScanOrdering::Concatenated` arm (the pattern
     `thread_spans_ordering_tests.rs` already uses), confirming it accepts the small partitions
     produced by the forced cut. This pins the second reachable instance of the bug, which the
     previous draft left open. (The test builds the *legacy* strictly-overlapping geometry via
     direct `UPDATE blocks` — exactly right, since after Part A only legacy producers emit it and
     Part B must keep handling it.)
10. **Sanity**: run `thread_spans_batched_generation_matches_per_segment` (`#[ignore]`d) — grouping
   is untouched so it must pass unmodified; run
   `python/micromegas/tests/test_queries.py::test_spans` against a live stack per the Repro Steps
   and confirm the cross-hour query succeeds; check whether any Python test asserts
   `list_partitions()`'s column list (the new column appears there).
11. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`,
    `python3 build/python_ci.py`.
12. Add `CHANGELOG.md` entries under `## Unreleased`: `**Analytics:**` (schema migration + scan
    check fix) and `**Tracing:**` (block boundary timestamps now shared, matching the Unreal
    producer), following the repo's established convention. Neither `rust/tracing/Cargo.toml` nor
    `rust/analytics/Cargo.toml` sets `publish = false`, so both entries need a **Minor breaking
    change** clause, matching every comparable entry already in the file (e.g. `## Unreleased` line
    8, v0.29.0 lines 13-20/24/26): the `**Tracing:**` entry must name `TracingBlock::new` and
    `EventBlock::close` (now `close_at`) and what call sites must change; the `**Analytics:**` entry
    must name `PartitionRowSet` and `Partition` (both all-public-field structs, so downstream struct
    literals break) plus `write_rows_and_track_times`'s widened return type.

## Files to Modify

| File | Change |
|---|---|
| `rust/tracing/src/dispatch.rs` | One `DualTime::now()` per flush (4 sites); close outgoing block with the same stamp |
| `rust/tracing/src/event/block.rs` | `close_at`/`begin`-parameter plumbing on `EventBlock`/`TracingBlock` |
| `rust/tracing/src/event/stream.rs` + tracing tests | Mechanical constructor ripple; new boundary-stamp unit test |
| `rust/analytics/benches/parse_block.rs` | Mechanical `TracingBlock::new`/`EventBlock::close` constructor ripple |
| `rust/analytics/tests/{span_tests,parse_block_tests,parse_corrupt_block_tests,parse_alloc_test,log_tests,metrics_test,image_tests,jit_process_batch_db_test}.rs` | Mechanical `TracingBlock::new`/`EventBlock::close` constructor ripple |
| `rust/telemetry-sink/tests/http_event_sink_transport_tests.rs` | Mechanical constructor ripple + `block.close()` → `close_at` update (`:169, :176, :183`) |
| `rust/analytics/src/lakehouse/migration.rs` | v7→v8: nullable `max_sort_key_time TIMESTAMPTZ`; bump version constant |
| `rust/analytics/src/lakehouse/partition.rs` | New field + accessor; `validate` invariant |
| `rust/analytics/src/lakehouse/partition_cache.rs` | Add column to 4 SELECTs / 3 struct builds (`sort_order` read pattern, not `file_path`'s) |
| `rust/analytics/src/lakehouse/list_partitions_table_function.rs` | Arrow schema + both SELECTs |
| `rust/analytics/src/lakehouse/write_partition.rs` | `PartitionRowSet` field; all-Some running-max fold in `write_rows_and_track_times`; `PartitionWriteResult`; explicit-column-list INSERT |
| `rust/analytics/src/lakehouse/thread_spans_view.rs` | Set `max_sort_key_time` from last row's `begin` |
| `rust/analytics/src/lakehouse/{net_spans_view,async_events_block_processor,log_block_processor,image_block_processor}.rs` | Mechanical `max_sort_key_time: None` addition to each `PartitionRowSet` literal |
| `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` | `partition_bounds` EventTime arm; rustdoc + error string |
| `rust/analytics/src/lakehouse/view.rs` | Rewrite `Concatenated` residual-caveats note (`:166-187`) |
| `rust/analytics/src/lakehouse/jit_partitions.rs` | Module-doc amendment only (`:17-23`) — **no code change** |
| `rust/analytics/tests/thread_spans_ordering_tests.rs` | New no-DB check tests; helper gains field |
| `rust/analytics/tests/{per_file_scan_ordering,blocks_view_merge_ordering,sql_batch_view_merge_ordering,log_stats_ordering}_tests.rs` | One-line helper updates for the new field |
| `rust/analytics/tests/write_partition_tests.rs` | Adapt to `write_rows_and_track_times`'s widened return |
| `rust/analytics/tests/thread_spans_ordering_db_test.rs` | Mechanical constructor ripple; two new DB regression tests (hour-seam, forced cut) |
| `CHANGELOG.md` | Entry under `## Unreleased` → `**Analytics:**` |

## Trade-offs

- **Producer fix alone (Part A only) is insufficient.** Already-ingested blocks keep their
  overlapping tick geometry until retention expires them — every rebuild reproduces the failure —
  and instrumented applications ship with pinned `micromegas-tracing` versions, so old producers
  keep sending overlapping blocks long after the crate is fixed. The server must tolerate that
  data; Part B is that tolerance. Part A is still worth shipping: it makes the data match its own
  design intent (the shared boundary stamp *is* the consecutive-blocks marker), matching the
  Unreal producer.
- **Rejected alternative: chain-safe bucket merging (this plan's previous draft).** Merging hour
  buckets whose boundary falls inside a strict tick overlap looks minimal but degenerates on the
  real producer: since *every* consecutive block pair of a `micromegas_tracing` thread stream
  strictly overlaps, the merge fires at every hour boundary, collapsing the whole queried range
  into one monolithic partition; exact-equality freshness then retires and rewrites that monolith
  whenever the queried range changes or grows (query-range-dependent specs, quadratic cumulative
  rewrite cost for long-running processes); and any range exceeding `max_nb_objects` still fails
  identically at the forced cut, where no tick-safe cut point can exist. The bounds fix keeps
  hourly, bounded, query-independent partitions and covers both cut kinds.
- **New column vs. reusing existing metadata.** `sort_order` is a column-name list (and NULL for
  thread_spans partitions); parquet footer statistics are not available to the planner's
  partition-metadata check. A dedicated nullable column is the smallest honest carrier, and the
  v6→v7 migration is exact precedent for the nullable/no-backfill shape.
- **Nullable, no backfill.** Legacy partitions keep today's (over-conservative) check and can
  still fail at a seam — but retiring them now genuinely fixes the failure, which the updated
  error message states. Backfilling would require reading every parquet file; not worth it for a
  self-healing path.
- **Asymmetric comparison.** Only the *previous* side of the pair gets the exact bound; the next
  side keeps block-derived `min_event_time` (≤ its actual min row `begin`). That is the
  conservative direction — it can only reject more than a fully-exact comparison would — and it is
  sufficient for both reproduced cut cases, so a `min_sort_key_time` twin column is not justified.
- **`net_spans` untouched.** It declares no scan ordering today (`net_spans_view.rs:349-351`) and
  has no row-monotonicity check; if it ever declares `Concatenated`, it must first add an
  `ensure_begin_non_decreasing` equivalent and populate `max_sort_key_time` — worth a one-line
  note in its existing "no ordering declared" comment.
- **Views that never record the value pay nothing.** The `InsertTime` bounds arm and the
  `PerFile`/`Unordered` paths never read it; their writers pass `None` implicitly via
  `PartitionRowSet::new`.

## Documentation

Rustdoc only (no `doc/` page describes these internals, matching `1429`'s precedent): the items
listed in Implementation Step 6. The migration comment documents the column's meaning and NULL
semantics at the schema level.

## Testing Strategy

- **Producer boundary test** (`rust/tracing`): flush across a block boundary and assert the closed
  block's `end` equals the replacement's `begin` (one shared `DualTime`).
- **No-DB check tests** (`thread_spans_ordering_tests.rs`): recorded-bound accepted / NULL
  fallback rejected / genuine overlap rejected — the check-level contract.
- **DB regression tests**: hour-seam repro and forced-cut repro (Implementation Step 9). Hour-seam
  asserts the live `view_instance` query succeeds and rows are complete — the end-to-end contract,
  including that `update_partition` actually persists the new column. Forced-cut cannot go through
  a live query (`jit_update` would rewrite the small forced-cut partitions before the scan ran), so
  it asserts directly against the written partitions via `PartitionCache` and
  `make_partitioned_execution_plan`'s `Concatenated` arm instead.
- **Existing suites**: `thread_spans_batched_generation_matches_per_segment` unmodified (grouping
  untouched); `write_partition_tests.rs` adapted; the four `Partition`-literal helpers updated
  mechanically.
- **Manual verification against a live stack**: re-run this plan's Repro Steps (generator +
  cross-hour query) and `python/micromegas/tests/test_queries.py::test_spans`; verify the
  migration runs cleanly on an existing local database (v7 → v8) and that pre-existing partitions
  (NULL column) still scan.
- `cargo test` (no-DB tests run in plain `cargo test`; DB-backed tests stay `#[ignore]`d per the
  file's convention) and `python3 build/rust_ci.py` / `python3 build/python_ci.py` for the full
  gate.

## Open Questions

None. (The GitHub issue `1429` called for is filed: #1478, linked at the top of this plan —
reference it from the `CHANGELOG.md` entries and commit messages.)
