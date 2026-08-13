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
  167-172`); a span open at chain end keeps its real `begin` (`:147-153` — only its `end` is clamped
  up); a span open at chain start has `begin` clamped *down* to the chain begin (`:194-201`), as
  does the synthetic root (`:106-112`). Those three are the only assignments to `begin` in the file.
- Rows are emitted in preorder (`rust/analytics/src/span_table.rs:126-146, 172-187`), verified
  non-decreasing on `begin` at write time (`ensure_begin_non_decreasing`,
  `thread_spans_view.rs:131-148`, called at `:218`).

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
  hour-seam cuts *and* forced intra-bucket cuts uniformly. `ThreadSpansView::SCHEMA_VERSION` also
  bumps 2 → 3 (the same lever `1429`'s v0.29.0 entry used on this exact view), so every existing
  `thread_spans` JIT partition is stale by schema hash after deploy and rebuilds automatically —
  carrying `max_sort_key_time` — on its first query, with no admin `retire_partitions` call
  required (see Trade-offs).

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
`thread_spans_degenerate_range_retires_stale_partition` (`:697`, SQL at `:738`) and five other
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
`OrderingBounds::EventTime` is `ThreadSpansView` (`thread_spans_view.rs:434-442`). `net_spans`
declares **no** scan ordering (`net_spans_view.rs:349-351` documents this explicitly), and the
only `Concatenated` merge path is `BlocksView`'s, with `OrderingBounds::InsertTime`
(`blocks_view.rs:81-87`, `merge.rs:276-295`). So exactly one view needs to populate the new
metadata; every other writer can leave it NULL.

### Where partitions get their bounds and rows

`ThreadSpansView`'s `write_partition` (`thread_spans_view.rs:150-256`) builds one call tree per
unbroken block chain (`group_contiguous_block_chains`), appends them in preorder into a
`SpanRecordBuilder`, verifies the resulting batch's `begin` column is non-decreasing
(`ensure_begin_non_decreasing`, `:131-148`, called at `:218`), and sets `rows_time_range` from the
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
(`list_partitions_table_function.rs:52-93`) and two SELECTs (`:113-129`, `:131-146`).

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
block with the same value. For the log/metrics/image paths this stamp **must be taken after the
stream mutex is acquired** — mirroring `FlushLogStreamImpl`/`FlushMetricStreamImpl`, which are
entered with the lock already held and take `DualTime::Now()` inside it
(`unreal/MicromegasTracing/Private/Dispatch.cpp:105-123, 125-…, 252-272`) — i.e. `now` is taken
right where the current code's `is_empty()` early return sits in `flush_log_buffer` (`:800-817`),
`flush_metrics_buffer` (`:695-712`), and `flush_image_buffer` (`:553-572`), not at function entry:
a stamp taken before the lock would let another thread push an event through `log()`/`metrics()`
(which take the same mutex) between the stamp and the acquisition, producing `block.end_ticks`
before that event's own tick. The thread path is unaffected by this ordering concern — it is
lock-free and runs only on the owning thread. Mechanically:

- `DualTime` (`rust/tracing/src/time.rs:4-8`) currently derives only `Debug`. Handing one stamp to
  two places, and asserting equality in the new test, both need more: add
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`. Both fields (`i64`, `DateTime<Utc>`) are `Copy`,
  and adding derives is purely additive for downstream crates.
- `EventBlock::close` (`rust/tracing/src/event/block.rs:18-20`) gains a sibling
  `close_at(&mut self, end: DualTime)` used by the four flush paths. The existing self-stamping
  `close()` is **retained** (not removed) because it stays the right call for the standalone-block
  callers in benches and tests that swap no stream and so have no shared timestamp to pass — about
  twenty call sites across `rust/analytics/benches/parse_block.rs`, the nine `rust/analytics/tests/`
  files below, and `rust/telemetry-sink/tests/http_event_sink_transport_tests.rs:169, 176, 183`. Note
  there is no *shutdown* path that needs it — `shutdown_telemetry` (`guards.rs:66-70`) goes through
  the ordinary flush paths and `Dispatch::shutdown` (`dispatch.rs:488-496`) never touches a block.
- `TracingBlock` (`block.rs:30-47`) gains a **required**
  `new_at(buffer_size, process_id, stream_id, object_offset, begin: DualTime) -> Self`, and its
  existing `new(..)` becomes a **provided default** that delegates with `DualTime::now()` (both need
  `where Self: Sized` for the default body to return `Self` by value). `EventBlock`'s impl
  (`block.rs:50-69`) moves its body into `new_at` and stops stamping internally.

  Shaping it this way keeps the ripple to the four dispatch flush sites and nothing else:
  `EventStream::new` (`event/stream.rs:52`, whose first block has no predecessor),
  `rust/analytics/benches/parse_block.rs`, the nine `rust/analytics/tests/` files that build blocks
  directly (`span_tests.rs`, `parse_block_tests.rs`, `parse_corrupt_block_tests.rs`,
  `parse_alloc_test.rs`, `log_tests.rs`, `metrics_test.rs`, `image_tests.rs`,
  `jit_process_batch_db_test.rs`, `thread_spans_ordering_db_test.rs`) and
  `rust/telemetry-sink/tests/http_event_sink_transport_tests.rs` all keep calling `new(..)`
  unchanged. Together with the retained `close()`, that leaves roughly fifty call sites across
  twelve files untouched that a plain signature change would have had to edit by hand for no
  behavioral gain — worth avoiding in code this subtle. `EventBlock<Q>` (`block.rs:50`) is the
  trait's only implementor in the repo, so making `new_at` required costs nothing internally.
- Only the thread path affects `thread_spans` ordering, but all four paths get the same treatment
  for consistency — the log/metrics/images views are `InsertTime`-bounded and indifferent.

Semantics are preserved: `group_contiguous_block_chains` already treats *touching* blocks as
chain-connected (it breaks only on `begin_ticks > running_end`), so call trees still merge
seamlessly across the seam — which is the entire point of the shared boundary stamp. The
analytics-side equivalence test's synthetic data already touches exactly, matching the new
producer behavior.

Add a `rust/tracing/tests/` unit test (crate convention: tests live under `tests/`, never inline),
modelled on `rust/tracing/tests/image_tests.rs:9-42` — `init_in_memory_tracing()`, emit, flush, then
read the blocks out of `InMemorySink`'s `MemSinkState`, `#[serial]` because the dispatch is
process-wide. The sink only ever receives *closed* blocks and the live replacement stays in the
(thread-local) stream, so the test must flush **twice** and assert
`blocks[0].end == Some(blocks[1].begin)` (ticks and wall-clock both — they come from one
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
  `max_sort_key_time: Option<DateTime<Utc>>`. Keep `new()` two-argument (sets `None`) so the seven
  `::new` call sites (`sql_partition_spec.rs:169`, `merge.rs:409`, `metrics_block_processor.rs:68`,
  `metadata_partition_spec.rs:105`, `otel/spans_block_processor.rs:318`,
  `otel/logs_block_processor.rs:243`, `otel/metrics_block_processor.rs:399`) are untouched. Five
  sites build the struct as a literal instead: it's set at `thread_spans_view.rs:221-224`, and
  `max_sort_key_time: None` must be added at the four mechanical sites in `net_spans_view.rs:208`,
  `async_events_block_processor.rs:157`,
  `log_block_processor.rs:68`, and `image_block_processor.rs:81` (or have them switch to
  `PartitionRowSet::new`).
- `write_rows_and_track_times` (`:626-675`) folds a running value alongside the existing min/max
  fold. Soundness rule: the partition-level value is `Some(max)` **only if every** received row
  set carried `Some`; any `None` poisons the whole partition to `None`. It must be a running
  `max`, never "last row set wins" — not because of `thread_spans`, which sends exactly one row set,
  but because this function is **shared**: `BlockPartitionSpec` (log/metrics/images/async_events/
  otel-spans) streams row sets out of order via `buffer_unordered`
  (`block_partition_spec.rs:144`, sends at `:154-156`). Say so in the rustdoc so the rule is not
  later "simplified" away on the grounds that the thread_spans path is single-shot. The function is
  `pub` and used by
  `rust/analytics/tests/write_partition_tests.rs:34`; widen its return type to a small struct and
  fix that test.
- Thread through `PartitionWriteResult` (`:617-623`; three construction sites at `:712-717`,
  `:727-732`, `:765-770`) into the `Partition` literal (`:884-894`) and the INSERT. The two
  empty-partition branches (`:712-717`'s zero-row-file case and `:765-770`'s no-`event_time_range`
  case) must set `max_sort_key_time: None` alongside their existing `event_time_range: None`, so an
  empty partition can never carry a non-NULL bound. While touching the INSERT (`:531`), convert it
  to an **explicit column list** — the bare positional `VALUES` with a hardcoded `2` mis-binds
  silently if a column is ever added anywhere but last.

### 3. Populate it in `thread_spans_view`

In `write_partition`, after `ensure_begin_non_decreasing` passes, the max `begin` is simply the
last row's value (rows are verified non-decreasing) — read it from the `begin` column and set it
on the emitted `PartitionRowSet`. `write_partition` sends its `PartitionRowSet` unconditionally, so
guard the read: when `rows.num_rows() == 0` (e.g. an empty call-tree chain), set
`max_sort_key_time` to `None` instead of indexing the `begin` column.

Also bump `SCHEMA_VERSION` (`:38`) 2 → 3 in the same file — the lever `1429` used on this exact
view. `get_file_schema_hash` changes, so every pre-existing `thread_spans` JIT partition becomes
stale by schema hash and rebuilds automatically on its next query, carrying `max_sort_key_time`,
with no admin `retire_partitions` call required (see Trade-offs).

### 4. Read it back

- `Partition` (`partition.rs:8-30`) gains `max_sort_key_time: Option<DateTime<Utc>>` with an
  accessor. Follow the `sort_order` read pattern — `r.try_get("max_sort_key_time")?`, NULL → `None`
  (`partition_cache.rs:125/209/455`); do **not** copy `file_path`'s `.ok()` pattern, which swallows
  decode errors.
- Add the column to the four `Partition`-building SELECTs (`partition_cache.rs:66`, `:148`,
  `:359`, `:394` — only three `try_get` sites, since `:359`/`:394` are the two query-range branches
  of `LivePartitionProvider::fetch` sharing one row-decoding loop) and to `list_partitions()`'s
  Arrow schema + both SELECTs (`list_partitions_table_function.rs:52-93`, `:113`, `:131`). Its
  `rows_to_record_batch` is generic **and strictly positional** — `sql_arrow_bridge.rs:371-396`
  builds one reader per `rows[0].columns()` and indexes the struct builder by the same ordinal — so
  append the new column **last** in both SELECTs and **last** in the schema `vec!`, and declare it
  nullable. No `sql_arrow_bridge.rs` change is needed: `TIMESTAMPTZ` already maps to a nullable
  `TimestampColumnReader` (`:330-337`, `:165-194`), the same path `min_event_time` uses.
- Extend `Partition::validate` (`partition.rs:90-117`) with the cheap invariant: when both are
  present, `min_event_time <= max_sort_key_time <= max_event_time`; also reject a non-NULL
  `max_sort_key_time` when `min_event_time`/`max_event_time` are NULL (an empty partition has no
  sort-key bound to record).

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

1. **Producer fix (Part A)**: derive `Clone, Copy, PartialEq, Eq` on `DualTime` (`tracing/src/time.rs`);
   add `EventBlock::close_at` and the `TracingBlock::new_at` required method with `new` as a
   delegating provided default (`event/block.rs`) — no other constructor call site changes; then a
   single `DualTime::now()` per flush in the four `dispatch.rs` flush paths, taken after the stream
   mutex is acquired (log/metrics/image — at the current `is_empty()` early-return point) or at
   function entry (thread path only, which is lock-free). New `rust/tracing/tests/` test that flushes
   twice and asserts the first closed block's `end` == the second's `begin`.
2. **Migration**: add `upgrade_v7_to_v8` in `rust/analytics/src/lakehouse/migration.rs` (nullable
   `max_sort_key_time TIMESTAMPTZ`, no backfill, comment stating NULL = "not recorded", modeled on
   `upgrade_v6_to_v7`'s column-add); bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 8.
3. **`Partition` + reads**: new field/accessor + `validate` invariant (`partition.rs`); add the
   column to the four SELECTs and three struct builds in `partition_cache.rs`; extend
   `list_partitions_table_function.rs` (Arrow schema + both SELECTs).
4. **Write path**: `PartitionRowSet` field; `write_rows_and_track_times` all-Some running-max fold
   + widened return struct (fix `rust/analytics/tests/write_partition_tests.rs`, adding a case that
   feeds several out-of-order `PartitionRowSet`s — some `Some`, one `None` — through the existing
   hand-built channel and asserts both the running max and the `None`-poisoning rule; note the
   existing channel is `channel(1)` and the test sends before the consumer runs, so the new case
   needs a larger channel or a spawned sender to avoid deadlocking on the second send, and each
   `RecordBatch` must match `make_arrow_writer`'s one-column `x: Int32` schema);
   `PartitionWriteResult`, with both empty-partition branches of `finalize_partition_write` setting
   `max_sort_key_time: None`; explicit-column-list INSERT with the new bind.
5. **Populate**: `thread_spans_view.rs::write_partition` sets `max_sort_key_time` from the last
   row's `begin` after `ensure_begin_non_decreasing`, or `None` when `rows.num_rows() == 0`. Bump
   `SCHEMA_VERSION` 2 → 3 in the same file so every pre-existing `thread_spans` partition self-heals
   on its next query instead of requiring an admin `retire_partitions` call (see Trade-offs).
6. **Check**: change `partition_bounds`'s `EventTime` arm per Design Part B §5.
7. **Docs**:
   - `view.rs:166-187` (`Concatenated` contract): rewrite the residual-caveats paragraph — the
     block-boundary tick overlap (old caveat 1) no longer trips the check for partitions carrying
     `max_sort_key_time`; note that the `SCHEMA_VERSION` 2 → 3 bump self-heals every pre-existing
     `thread_spans` partition on its next query, so legacy-NULL partitions are only a residual until
     first query, not an admin-retire dependency; remaining residuals are the insert-time inversion
     (2) and TSC drift (3).
   - `partitioned_execution_plan.rs`: `sort_and_check_non_overlapping` rustdoc + runtime error
     string (`:52-59`, `:82-88`) — the "retire to fix" advice becomes a "this heals itself on next
     query" note for the buffer-swap cause, accurate for the remaining causes as-is. `partition_bounds`
     and `attach_ordering_statistics` doc updates.
   - `jit_partitions.rs` module doc (`:17-23`): amend the "says nothing about event-time ranges"
     note to point at `max_sort_key_time` as the mechanism that makes block-tick overlap harmless
     to the scan check.
   - **Reframe "overlap by design" as "legacy producer bug"** everywhere the Rust overlap is
     described as expected behavior: `group_contiguous_block_chains`'s rustdoc
     (`jit_partitions.rs:127-141` — the chain rule itself stays tolerant, since legacy blocks
     exist), the producer comparison in `view.rs:170-179`, and the `jit_partitions.rs` module doc.
     After Part A, both producers touch exactly by design; strict overlap identifies data from
     pre-fix `micromegas-tracing` versions. `view.rs:171` names `TracingBlock::new -> DualTime::now()`
     as the mechanism and goes stale on both counts once `new_at` carries the shared stamp.
   - `partition.rs` / `write_partition.rs`: field and fold-rule rustdoc (including why any-None
     poisons the partition value).
   - `doc/how_to_query/README.md`'s `#### list_partitions` Returns table (`:475-488`): add
     `max_sort_key_time`, and, while there, the three columns it's already missing (`num_rows`,
     `partition_format_version`, `sort_order`) — see Documentation.
   - `mkdocs/docs/admin/functions-reference.md`'s `list_partitions()` Returns table (`:52-67`): add
     `max_sort_key_time`, and the already-missing `partition_format_version` — see Documentation.
8. **Unit tests (no DB)**: extend `rust/analytics/tests/thread_spans_ordering_tests.rs`
   (`make_partition` helper at `:36-51`): (a) two partitions whose `[min,max_event_time]` overlap
   but whose `max_sort_key_time` clears the next `min_event_time` → accepted; (b) same shape with
   NULL `max_sort_key_time` → still rejected (legacy fallback preserved); (c) genuine overlap in
   `max_sort_key_time` → rejected. Update the other four test files with `Partition` literals
   (`per_file_scan_ordering_tests.rs`, `blocks_view_merge_ordering_tests.rs`,
   `sql_batch_view_merge_ordering_tests.rs`, `log_stats_ordering_tests.rs` — one or two helpers
   each) for the new field. Also add the running-max/`None`-poisoning fold test to
   `write_partition_tests.rs` (see Testing Strategy) — a no-DB test too, just against
   `write_rows_and_track_times` directly rather than the check.

   Also add a **cut-position test** to `rust/analytics/tests/call_tree_tests.rs`, pinning the
   "Why this is sound" claim directly and with no DB. That file already drives `CallTreeBuilder`
   through the `ThreadBlockProcessor` trait with synthetic events, and already models the
   overlapping-seam shape (`:6-40`). Everything else needed is public too: `SpanRecordBuilder`
   (`span_table.rs:35`, `with_capacity` `:87`, `append_call_tree` `:126`, `finish` `:150`, already
   used no-DB by `dictionary_key_overflow_tests.rs`), `ensure_begin_non_decreasing`, and
   `ConvertTicks::from_meta_data`. No new `pub` surface is required. Shape: build two
   `CallTreeBuilder`s over the two sides of a cut between strictly-overlapping adjacent blocks
   (P's range ends at block `k-1`'s `end_ticks`, Q's begins at block `k`'s smaller `begin_ticks`),
   run each through a `SpanRecordBuilder`, and assert P's last row `begin` — its
   `max_sort_key_time` — is strictly below Q's `min_event_time`. Parameterize the cut position so
   one test covers the hour-seam cut and the mid-bucket forced cut identically, which is exactly
   the plan's claim that the cut *cause* is irrelevant.
9. **DB regression test** (a new `#[ignore]`d `#[tokio::test]` in
   `thread_spans_ordering_db_test.rs`, following `thread_spans_degenerate_range_retires_stale_partition`'s
   `push_and_insert_block` + `UPDATE blocks` pattern) — **hour-seam**: the two-block deterministic
   repro above; assert two partitions are emitted, each row's stored `max_sort_key_time` is
   non-NULL, and a live `view_instance('thread_spans', ...)` query across the boundary succeeds;
   also assert row completeness (union of partition rows covers both blocks' spans). This is the
   only DB test the change needs: it is what proves `update_partition` persists the new column and
   that the whole scan path reads it back. The test builds the *legacy* strictly-overlapping
   geometry via direct `UPDATE blocks` — exactly right, since after Part A only legacy producers
   emit it and Part B must keep handling it.

   A *second* DB test for the forced intra-bucket cut is deliberately **not** included. It would
   re-exercise the same write-and-check path the hour-seam test already covers, differing only in
   why the cut happened — which the soundness argument says is irrelevant, and which Step 8's
   parameterized cut-position test now pins directly. It could not even assert through a live query:
   `ThreadSpansView::jit_update` (`thread_spans_view.rs:371-374`) runs with the default
   `max_nb_objects` (`JitPartitionConfig { block_order: EventTime, ..Default::default() }`, so
   `20 * 1024 * 1024`) ahead of any scan, and would see small-`max_nb_objects` partitions as stale
   by source hash and rewrite them as one partition before the `Concatenated` check ever ran.
   Working around that needs a bespoke `PartitionCache` + `make_partitioned_execution_plan`
   assertion path, in an `#[ignore]`d test that only runs when someone remembers — a lot of
   machinery for coverage that a plain `cargo test` unit test delivers better. (Grouping itself is
   untouched by this plan, and mid-bucket cuts are already covered by
   `thread_spans_interrupted_run_reconverges` (`:1244`) and
   `thread_spans_cross_run_regrouping_replaces_stale_partition` (`:1512`), both `max_nb_objects: 4`.)
10. **Sanity**: run `thread_spans_batched_generation_matches_per_segment` (`#[ignore]`d) — grouping
   is untouched so it must pass unmodified; run
   `python/micromegas/tests/test_queries.py::test_spans` against a live stack per the Repro Steps
   and confirm the cross-hour query succeeds; confirm
   `doc/how_to_query/README.md`'s and `mkdocs/docs/admin/functions-reference.md`'s `list_partitions`
   Returns tables (Step 7) match the Arrow schema column-for-column.
11. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`,
    `python3 build/python_ci.py`.
12. Add `CHANGELOG.md` entries under `## Unreleased`: `**Analytics:**` (schema migration + scan
    check fix) and `**Tracing:**` (block boundary timestamps now shared, matching the Unreal
    producer), following the repo's established convention. Neither `rust/tracing/Cargo.toml` nor
    `rust/analytics/Cargo.toml` sets `publish = false`, so both entries need a **Minor breaking
    change** clause, matching every comparable entry already in the file (e.g. `## Unreleased` line
    8, v0.29.0 lines 13-15/18-20/24/26). The `**Tracing:**` entry's clause is narrow by construction:
    `TracingBlock::new` keeps its signature (it becomes a provided default), so only the trait's new
    required `new_at` method breaks — and solely for out-of-tree implementors of `TracingBlock`, of
    which there are none in this repo; mention `EventBlock::close_at` as an addition alongside the
    retained `close()`. The `**Analytics:**` entry
    must name `PartitionRowSet` and `Partition` (both all-public-field structs, so downstream struct
    literals break) plus `write_rows_and_track_times`'s widened return type. The `**Analytics:**`
    entry also needs an **Operational note**, following the shape of `1429`'s v0.29.0 entry
    (`CHANGELOG.md:13`, which bumped both views 1 → 2; this one bumps `thread_spans` alone):
    `ThreadSpansView::SCHEMA_VERSION` bumps 2 → 3, so every existing `thread_spans` JIT partition is
    stale after deploy and rebuilds automatically on first query — no admin action, but expect a
    one-off latency bump on the first query per stream.

## Files to Modify

| File | Change |
|---|---|
| `rust/tracing/src/time.rs` | Derive `Clone, Copy, PartialEq, Eq` on `DualTime` (one stamp must reach two places; the new test compares them) |
| `rust/tracing/src/dispatch.rs` | One `DualTime::now()` per flush (4 sites); construct the replacement with it and close the outgoing block with the same stamp |
| `rust/tracing/src/event/block.rs` | `EventBlock::close_at`; `TracingBlock::new_at` required, `new` becomes a delegating provided default |
| `rust/tracing/tests/` | New boundary-stamp unit test (flush twice, compare `end`/`begin` via `InMemorySink`) |
| `rust/analytics/src/lakehouse/migration.rs` | v7→v8: nullable `max_sort_key_time TIMESTAMPTZ`; bump version constant |
| `rust/analytics/src/lakehouse/partition.rs` | New field + accessor; `validate` invariant |
| `rust/analytics/src/lakehouse/partition_cache.rs` | Add column to 4 SELECTs / 3 struct builds (`sort_order` read pattern, not `file_path`'s) |
| `rust/analytics/src/lakehouse/list_partitions_table_function.rs` | Arrow schema + both SELECTs |
| `rust/analytics/src/lakehouse/write_partition.rs` | `PartitionRowSet` field; all-Some running-max fold in `write_rows_and_track_times`; `PartitionWriteResult`; explicit-column-list INSERT |
| `rust/analytics/src/lakehouse/thread_spans_view.rs` | Set `max_sort_key_time` from last row's `begin`; bump `SCHEMA_VERSION` 2 → 3 for self-healing rebuild |
| `rust/analytics/src/lakehouse/{net_spans_view,async_events_block_processor,log_block_processor,image_block_processor}.rs` | Mechanical `max_sort_key_time: None` addition to each `PartitionRowSet` literal |
| `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` | `partition_bounds` EventTime arm; rustdoc + error string |
| `rust/analytics/src/lakehouse/view.rs` | Rewrite `Concatenated` residual-caveats note (`:166-187`) |
| `rust/analytics/src/lakehouse/jit_partitions.rs` | Module-doc amendment only (`:17-23`) — **no code change** |
| `rust/analytics/tests/thread_spans_ordering_tests.rs` | New no-DB check tests; helper gains field |
| `rust/analytics/tests/{per_file_scan_ordering,blocks_view_merge_ordering,sql_batch_view_merge_ordering,log_stats_ordering}_tests.rs` | One-line helper updates for the new field |
| `rust/analytics/tests/write_partition_tests.rs` | Adapt to `write_rows_and_track_times`'s widened return |
| `rust/analytics/tests/call_tree_tests.rs` | New no-DB cut-position test (P's last row `begin` < Q's `min_event_time`), parameterized over the cut |
| `rust/analytics/tests/thread_spans_ordering_db_test.rs` | One new DB regression test (hour-seam, end-to-end) |
| `doc/how_to_query/README.md` | Add `max_sort_key_time` (and the already-missing `num_rows`, `partition_format_version`, `sort_order`) to the `list_partitions` Returns table |
| `mkdocs/docs/admin/functions-reference.md` | Add `max_sort_key_time` (and the already-missing `partition_format_version`) to the `list_partitions()` Returns table |
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
- **Nullable, no backfill for the column — but self-healing via a `SCHEMA_VERSION` bump.** The new
  `lakehouse_partitions.max_sort_key_time` column is itself nullable with no backfill: reading
  every existing parquet file just to populate it would be wasteful when the view can simply
  rebuild. Left at only that, every partition written before v8 would carry NULL forever and could
  still fail at a seam until an admin explicitly ran `retire_partitions` (`is_admin`-gated) — not
  a fix for the reported production failure by deploy alone. Instead, `ThreadSpansView::SCHEMA_VERSION`
  bumps 2 → 3 alongside the migration, exactly the lever `1429`'s v0.29.0 entry used on this same
  view: `get_file_schema_hash` changes, so every existing `thread_spans` JIT partition is stale
  after deploy and rebuilds automatically — carrying `max_sort_key_time` — on its first query, no
  admin action needed. (`spec_is_up_to_date` compares the stored `file_schema_hash`,
  `jit_partitions.rs:1171-1176`, and the `RetireMatch::Overlap` retire has no hash predicate, so the
  stale file is replaced in the same transaction; reads are hash-filtered independently,
  `partition_cache.rs:377/410`. Healing is per-query-range: a stale partition no later query touches
  is never rewritten, it just stays invisible until retention reclaims it.) Cost: a one-off
  rebuild-latency spike on the first query per stream,
  identical in kind to `1429`'s. That cost is worth paying here because the plan's stated goal is
  fixing a *production* failure that otherwise stays broken until an operator notices and manually
  retires — a bump that heals itself on deploy serves that goal; a fix that ships inert until an
  admin acts does not. (`net_spans` needs no equivalent bump: it never reads `max_sort_key_time`.)
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

Rustdoc: the items listed in Implementation Step 7. The migration comment documents the column's
meaning and NULL semantics at the schema level.

`doc/how_to_query/README.md`'s `#### list_partitions` section (`:467-488`) *does* describe these
internals, user-facing: it has a "Returns" table of `list_partitions()`'s columns. That table is
already stale — it's missing `num_rows`, `partition_format_version`, and `sort_order`, all three of
which `list_partitions_table_function.rs`'s Arrow schema already carries (`:52-93`) — and this
plan's new `max_sort_key_time` column would make it stale in one more way if left alone. Fix the
whole table in the same pass: add all four missing rows (`num_rows Int64`, `partition_format_version
Int32`, `sort_order List<Utf8>`, `max_sort_key_time Timestamp(Nanosecond)`) so the doc matches the
schema exactly.

`mkdocs/docs/admin/functions-reference.md`'s `list_partitions()` Returns table (`:52-67`) is the
published docs-site equivalent and has its own copy of the same column list — already missing
`partition_format_version`, and about to go stale on `max_sort_key_time` too if left alone. Fix it
in the same pass: add `partition_format_version` (`Int32`) and `max_sort_key_time`
(`Timestamp(Nanosecond)`) so it matches the schema exactly, same as the `doc/how_to_query` table.

## Testing Strategy

- **Producer boundary test** (`rust/tracing/tests/`): flush twice and assert the first closed
  block's `end` equals the second's `begin` (one shared `DualTime`). Two flushes, not one: the sink
  only ever sees closed blocks, so the live replacement is unreachable from it.
- **No-DB check tests** (`thread_spans_ordering_tests.rs`): recorded-bound accepted / NULL
  fallback rejected / genuine overlap rejected — the check-level contract.
- **No-DB cut-position test** (`call_tree_tests.rs`, Implementation Step 8): P's last row `begin`
  is strictly below Q's `min_event_time` for a cut between strictly-overlapping adjacent blocks,
  parameterized over the cut position — the soundness claim itself, covering the hour-seam and the
  forced intra-bucket cut in one place, in plain `cargo test`.
- **DB regression test**: the hour-seam repro (Implementation Step 9), asserting the live
  `view_instance` query succeeds and rows are complete — the end-to-end contract, and the only
  place that proves `update_partition` persists the new column and the scan path reads it back.
  One DB test is enough: see Step 9 for why a second, forced-cut DB test is not included.
- **Running-max / `None`-poisoning fold** (`write_partition_tests.rs`): feed
  `write_rows_and_track_times` several out-of-order `PartitionRowSet`s — some `Some`, one `None` —
  over its hand-built channel (widen it past `channel(1)`, or spawn the sender, so the second send
  doesn't deadlock) and assert the result is a running `max` (not "last row set
  wins") and that the single `None` poisons the partition-level value to `None`. This is the only
  genuinely new logic in the write path and the only place it is exercised, since
  `thread_spans_view` sends exactly one row set.
- **Existing suites**: `thread_spans_batched_generation_matches_per_segment` unmodified (grouping
  untouched); `write_partition_tests.rs` adapted (plus the new fold test above); the six
  `Partition`-literal helpers in the four other ordering test files updated mechanically.
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
