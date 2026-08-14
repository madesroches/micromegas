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
  replacement block's `begin` stamp. The one place holding raw `*mut ThreadStream` for *other*
  threads, `Dispatch::for_each_thread_stream` (`dispatch.rs:601-606`), has a single non-test caller,
  `FlushMonitor::tick` (`flush_monitor.rs:32-35`), which only calls `set_full()` — an atomic store
  (`event/stream.rs:74-76`). No cross-thread flush path exists, so nothing can interleave between
  the two stamps.
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

`rust/analytics/tests/thread_spans_ordering_db_test.rs` already contains this exact scenario:
`thread_spans_ordering_across_partitions` (`:208-437`) ingests two real thread blocks into one
`ThreadStream` via `push_and_insert_block` (`:73`), forces them into different 1-hour JIT
insert-time segments by pushing block 1's `insert_time`/`begin_time`/`end_time` back two hours
(`:271-280`), and then queries `view_instance('thread_spans', ...)` across the boundary,
asserting no `SortExec` and non-decreasing `begin` across the scan (`:400-433`).

Crucially, **that test already documents this plan's bug and works around it** (`:239-254`):
`replace_block` captures the replacement block's `begin` before the outgoing block's `close()`
runs, "a hairline overlap that … is enough to trip the §3 non-overlap guard", so the test sleeps
200 ms and then discards a throwaway "spacer" block to manufacture a real gap between block 1's
`end` and block 2's `begin`. Deleting that sleep and that spacer reproduces this plan's failure
deterministically, with no live generator and no hour of wall-clock — and after this plan lands,
the same test passes *without* the workaround. That is the regression test (Step 9). (Its comment
attributes the trip partly to `tsc_frequency == 0` in that environment forcing estimated tick
conversion; that affects the *magnitude* of the overlap, not its existence — `block[k].end_ticks >
block[k+1].begin_ticks` holds before any conversion, and tick→time conversion is monotone, so the
strict `>` check trips either way.)

Note what does **not** work, since it is the obvious-looking approach and it silently produces a
vacuous test: fabricating block ticks with `UPDATE blocks SET begin_ticks = $1, end_ticks = $2, …`
(the pattern used by `thread_spans_degenerate_range_retires_stale_partition` (`:697`, SQL at
`:738`) and six other tests in that file). Event ticks are stamped from a real `now()` relative to
`process.start_ticks` (`:95-104`), while `make_call_tree`'s chain range comes from the *rewritten*
`begin_ticks`/`end_ticks` (`thread_spans_view.rs:107-113`). Fabricated ticks therefore put every
event outside `[begin_range_ns, end_range_ns]`, where `call_tree.rs:142-144` drops them: the
partition ends up with zero rows, `finalize_partition_write` writes `event_time_range: None`
(`write_partition.rs:711-717`), and `make_partitioned_execution_plan` filters the empty partition
out (`partitioned_execution_plan.rs:260-261`) *before* the non-overlap check ever runs. This is
precisely why every tick-rewriting test in that file asserts on partition **metadata** only, never
on rows, and why the test's own note at `:267-270` documents deliberately leaving
`begin_ticks`/`end_ticks` alone.

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

`lakehouse_partitions` currently has 14 columns (the base table at `migration.rs:116-129` creates
11; `num_rows` added v3, `partition_format_version` v5, `sort_order TEXT[]` v7). Schema version
lives in
`lakehouse_migration` (single row), `LATEST_LAKEHOUSE_SCHEMA_VERSION = 7` (`migration.rs:8`),
applied by an incremental ladder (`migration.rs:53-106`) under advisory lock. The v6→v7 migration
(`migration.rs:435-517`) is the exact precedent for this plan's column: nullable, no backfill,
"NULL means 'no guarantee', which is automatically correct for every partition written before this
column existed" (`:436-438`, with the `ALTER` at `:439`).

`Partition` (`partition.rs:8-30`) is built from DB rows in exactly three functions / four SELECTs
(`partition_cache.rs:66-83`, `:148-165`, `:359-379`, `:394-412`; struct builds at `:116-126`,
`:200-210`, `:446-456`) and written in one production INSERT (`write_partition.rs:529-546` — a bare
positional `VALUES($1,...,$12, 2, $13)` with the literal `2` being `partition_format_version`). The
only other INSERT anywhere is in a test, `net_spans_retire_overlap_db_test.rs:53-57`, which already
uses an explicit 12-column list and therefore needs no edit — and which is the precedent for making
the production one explicit too.
`list_partitions()` additionally mirrors the table to SQL with its own Arrow schema
(`list_partitions_table_function.rs:52-93`) and two SELECTs (`:113-129`, `:131-146`).

The existing `sort_order` column cannot carry this value: it is a `TEXT[]` of column *names*, read
by `certifies_sort_order` (`partition.rs:65-82`, called from the `PerFile` gate
`partitioned_execution_plan.rs:266-275` and `sql_batch_view.rs:229`) and directly by
`blocks_view.rs:41-46` — and `thread_spans` writes it as NULL anyway
(`thread_spans_view.rs:180`, the `sort_order` argument of `write_partition_from_rows`). A new column
is required.

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
thread `:820-835`, log `:800-817`, metrics `:695-712`, image `:553-573`): use **one** timestamp for
both the replacement block's `begin` and the outgoing block's close, instead of today's two separate
`DualTime::now()` stamps.

No new stamp needs to be taken at all. Each flush path already builds the replacement block — and
therefore already stamps its `begin` — *before* closing the outgoing one (`dispatch.rs:561`, `:703`,
`:808`, `:826`, then `:570`, `:710`, `:815`, `:833`). So the shared value is simply the replacement
block's own `begin`, read back off it:

```rust
let new_block = Arc::new(ThreadBlock::new(buffer_size, self.process_id, stream_id, next_offset));
let begin = new_block.begin;                    // Copy: must be read before the Arc moves
let mut old_block = stream.replace_block(new_block);
assert!(!stream.is_full());
Arc::get_mut(&mut old_block).unwrap().close_at(begin);
```

That makes `block[k].end == block[k+1].begin` true *by construction* rather than by passing one
value to two places, and it needs no new constructor (see below).

It also satisfies, for free, the ordering constraint this design would otherwise have to arrange by
hand. For the log/metrics/image paths the stamp **must be taken after the stream mutex is
acquired** — mirroring `FlushLogStreamImpl`/`FlushMetricStreamImpl`, which are entered with the lock
already held and take `DualTime::Now()` inside it
(`unreal/MicromegasTracing/Private/Dispatch.cpp:105-123, 125-143`; the wrappers that take the lock
are `:252-272`). A stamp taken before the lock would let another thread push an event through
`log()`/`metrics()` (which take the same mutex — `dispatch.rs:725/758/776` vs `:801`,
`:621/635/654/678` vs `:696`, `:540` vs `:554`) between the stamp and the acquisition, producing
`block.end_ticks` before that event's own tick. Because the replacement block is already constructed
inside the flush's mutex guard in all three lock-held paths, its `begin` is already stamped under
the lock and **no code motion is required**. The thread path is unaffected by this concern either
way — it is lock-free and runs only on the owning thread.

Scope that claim precisely, because it is easy to over-read: taking the stamp under the lock closes
the **close side** of the window (no event already in the outgoing block can post-date its `end`).
It does *not* close the open side. `log()`, `int_metric()`, and `send_image()` each compute
`let time = now();` *before* acquiring the mutex (`dispatch.rs:723`, `:620`, `:539`), and for logs
an entire `on_log` sink callback runs in between (`:724`) — so an event can still land in the
*replacement* block carrying a tick earlier than that block's `begin`. This is pre-existing, Part A
neither causes nor fixes it, and it is harmless here: the log/metrics/images views are
`InsertTime`-bounded and declare no `Concatenated` event-time ordering, so no scan check reads
those blocks' event-time geometry. The thread path is immune to this half too (single-owner,
lock-free), which is why `thread_spans` — the only view that *does* declare the ordering — is fully
covered. Mechanically:

- `DualTime` (`rust/tracing/src/time.rs:4-8`) currently derives only `Debug`. Reading `begin` back
  out of an `Arc<Block>` needs `Copy`, and asserting equality in the new test needs `PartialEq`: add
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`. Both fields (`i64`, `DateTime<Utc>`) are `Copy` and
  `Eq`, and adding derives is purely additive for downstream crates.
- `EventBlock::close` (`rust/tracing/src/event/block.rs:18-20`) gains a sibling
  `close_at(&mut self, end: DualTime)` used by the four flush paths. The existing self-stamping
  `close()` is **retained** (not removed) because it stays the right call for the standalone-block
  callers in benches and tests that swap no stream and so have no shared timestamp to pass. Worth
  stating outright so a reviewer does not flag it as dead API: after this change `close()` has
  **zero** callers inside `micromegas_tracing` itself — all four flush paths move to `close_at` —
  and survives entirely for those out-of-crate callers, 23
  call sites across `rust/analytics/benches/parse_block.rs`, the nine `rust/analytics/tests/`
  files (`span_tests.rs`, `parse_block_tests.rs`, `parse_corrupt_block_tests.rs`,
  `parse_alloc_test.rs`, `log_tests.rs`, `metrics_test.rs`, `image_tests.rs`,
  `jit_process_batch_db_test.rs`, `thread_spans_ordering_db_test.rs`), and
  `rust/telemetry-sink/tests/http_event_sink_transport_tests.rs:169, 176, 183`. Note
  there is no *shutdown* path that needs it: `Dispatch::shutdown` (`dispatch.rs:488-496`) only swaps
  the sink and never touches a block, and `shutdown_telemetry` (`guards.rs:66-70`) calls the
  ordinary `flush_log_buffer`/`flush_metrics_buffer` (thread buffers are flushed by
  `TracingThreadGuard::drop`, `guards.rs:85-89`; images are not flushed at shutdown at all, which is
  pre-existing and out of scope here).
- **No new constructor is needed, and `TracingBlock` is not touched.** `EventBlock`'s `begin` is
  already a public field (`block.rs:8`), so the flush paths read the shared stamp off the
  replacement block they already build. `TracingBlock::new` keeps its signature and its
  self-stamping body (`block.rs:55-69`), the trait gains no method, and all 24 of `new`'s
  out-of-crate call sites across 11 files — plus `EventStream::new` (`event/stream.rs:52`, whose
  first block has no predecessor) — stay untouched. An earlier draft of this plan added an inherent
  `EventBlock::new_at` taking the stamp as a parameter; that is strictly more API surface for the
  same result, and it makes `end == next.begin` a property of two call sites agreeing rather than a
  property of the code's shape. Dropping it also removes any need to reason about inherent-vs-trait
  method resolution or about which impl block carries which `Q` bound.
- Only the thread path affects `thread_spans` ordering, but all four paths get the same treatment
  for consistency — the log/metrics/images views are `InsertTime`-bounded and indifferent.

Semantics are preserved: `group_contiguous_block_chains` already treats *touching* blocks as
chain-connected (it breaks only on `begin_ticks > running_end`), so call trees still merge
seamlessly across the seam — which is the entire point of the shared boundary stamp. The
analytics-side equivalence test's synthetic data already touches exactly, matching the new
producer behavior.

Add a `rust/tracing/tests/` unit test (crate convention: tests live under `tests/`, never inline),
modelled on `rust/tracing/tests/image_tests.rs:9-42` — `init_in_memory_tracing()` (defined in
`rust/tracing/src/test_utils.rs:63`, and enabling CPU tracing via the final `true` argument at
`:29`), then read the blocks out of `InMemorySink`'s `MemSinkState`
(`rust/tracing/src/event/in_memory_sink.rs:15-34`),
`#[serial]` because the dispatch is process-wide. The sink only ever receives *closed* blocks, so
the sequence must be **emit → flush → emit → flush**, then assert
`blocks[0].end == Some(blocks[1].begin)` (ticks and wall-clock both — they come from one
`DualTime`; `Copy` is load-bearing here, since `blocks[1].begin` cannot be moved out of an `Arc`).

Two traps worth stating, because both silently produce a broken test:

- **Flushing twice with no second emit yields only one block.** All four flush paths early-return
  on `is_empty()` (`dispatch.rs:555`, `:697`, `:802`, `:821`), so `emit → flush → flush` leaves the
  sink holding a single block and `blocks[1]` panics on an out-of-bounds index.
- The live replacement block is unreachable from the sink in every case, but it lives in the
  *thread-local* stream only for the thread path; the log/metrics/image replacements live in
  `Dispatch`'s mutex-guarded streams.

The thread stream is the one Part A actually cares about and is already exercised this way:
`TracingThreadGuard::new()` + `span_scope!` + `flush_thread_buffer()`, reading `state.thread_blocks`
(`rust/tracing/tests/async_depth_tracking_tests.rs:119-142`). The image stream (`send_image` +
`flush_image_buffer`) is an equally valid target and needs no thread guard.

### Part B — server: record the true sort-key bound

### 1. Schema: `max_sort_key_time` column (v7 → v8)

Add a nullable `max_sort_key_time TIMESTAMPTZ` column to `lakehouse_partitions` — the maximum
value of the view's declared `Concatenated` leading sort column across the partition's rows
(`begin` for `thread_spans`). NULL means "not recorded" (any partition written before v8, and any
view that doesn't declare a `Concatenated` event-time ordering). No backfill, following the v6→v7
precedent verbatim; bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 8.

### 2. Carry it through the write path

- `PartitionRowSet` (`write_partition.rs:53-66`) gains
  `max_sort_key_time: Option<DateTime<Utc>>`, and **`new()` takes it as a third argument** — so
  every construction site is enumerated by the compiler. Twelve sites total: the seven `::new`
  callers (`sql_partition_spec.rs:169`, `merge.rs:409`, `metrics_block_processor.rs:68`,
  `metadata_partition_spec.rs:105`, `otel/spans_block_processor.rs:318`,
  `otel/logs_block_processor.rs:243`, `otel/metrics_block_processor.rs:399`) all pass `None`, and
  the five struct literals — set at `thread_spans_view.rs:221-224`, `None` at
  `net_spans_view.rs:208`, `async_events_block_processor.rs:157`, `log_block_processor.rs:68`,
  `image_block_processor.rs:81`.

  Widening `new()` rather than keeping it two-argument is deliberate. `None` happens to be correct
  for all seven callers today — none declares a `Concatenated` event-time ordering, so falling back
  to `max_event_time` is right — but a defaulted `new()` would hand that `None` out *silently*, and
  a future view that does declare the ordering could be added through `::new` and get a wrong bound
  with no signal at all. The failure mode of a silent default here is a scan check that trusts a
  bound nobody computed; the cost of avoiding it is passing `None` at seven call sites. This is a
  published crate, so it is a minor breaking change — recorded in `CHANGELOG.md` (Step 12), not
  designed around.
- `write_rows_and_track_times` (`:626-675`) folds a running value alongside the existing min/max
  fold. Soundness rule: the partition-level value is `Some(max)` **only if every** received row
  set carried `Some`; any `None` poisons the whole partition to `None`. It must be a running
  `max`, never "last row set wins" — not because of `thread_spans`, which sends exactly one row set,
  but because this function is **shared** by four multi-row-set senders: `BlockPartitionSpec`
  (log/metrics/images/async_events/otel-spans) streams row sets *out of order* via
  `buffer_unordered` (`block_partition_spec.rs:144`, sends at `:154-156`), and
  `sql_partition_spec.rs:163-171`, `merge.rs:404-412`, and `metadata_partition_spec.rs:90-108`
  (per chunk) each send many in order. `max` is correct for all four; "last wins" is wrong for the
  first and merely accidental for the rest. Say so in the rustdoc so the rule is not
  later "simplified" away on the grounds that the thread_spans path is single-shot. It returns
  `Result<Option<TimeRange>>` today (`:631`); widen that to a small struct carrying both the range
  and the folded `max_sort_key_time`. That struct must be `pub`: `write_rows_and_track_times` is
  itself `pub` and is called from `rust/analytics/tests/`, which compiles as an external crate
  (unlike the private `PartitionWriteResult`, which is only ever internal). Two call sites follow:
  the load-bearing one is internal —
  `write_partition.rs:846`, whose `Ok(range) => range` arm and `Err` arm (`:847-866`) destructure
  the return value and whose `finalize_partition_write(event_time_range, …)` call (`:869-878`) must
  now also forward the new value. The other is `rust/analytics/tests/write_partition_tests.rs:34`,
  which only asserts `is_err()` and so may need nothing beyond recompiling.
- Thread through `PartitionWriteResult` (`:617-623`; three construction sites at `:712-717`,
  `:727-732`, `:765-770`) into the `Partition` literal (`:884-894`) and the INSERT. The two
  empty-partition branches (`:712-717`'s zero-row-file case and `:765-770`'s no-`event_time_range`
  case) must set `max_sort_key_time: None` alongside their existing `event_time_range: None`, so an
  empty partition can never carry a non-NULL bound. While touching the INSERT (`:531`), convert it
  to an **explicit column list**. Note precisely what this does and does not buy: Postgres's
  `ALTER TABLE ... ADD COLUMN` always appends, so the current bare positional
  `VALUES($1,…,$12, 2, $13)` cannot mis-bind from an ordinary migration — and *that* is the real
  hazard here, since adding column 15 without touching this statement compiles, runs, and silently
  stores NULL forever. The explicit list makes the statement's dependency on the table's shape
  visible at the call site (and pins the hardcoded `2` to `partition_format_version` by name rather
  than by ordinal), which is what makes the omission catchable in review. The persistence
  round-trip test in Step 9 is what actually catches it mechanically.

### 3. Populate it in `thread_spans_view`

In `write_partition`, after `ensure_begin_non_decreasing` passes, the max `begin` is simply the
last row's value (rows are verified non-decreasing) — read it from the `begin` column and set it
on the emitted `PartitionRowSet`. One `SpanRecordBuilder` accumulates every chain and `finish()`
emits exactly one `RecordBatch` (`span_table.rs:150-169`), with the check running on that whole
batch at `:218`, so "the last row" really is the partition-global max, with no per-chain scoping
hole. The column is `Timestamp(Nanosecond, Some("+00:00"))` (`span_table.rs:56-60`), read via
`typed_column_by_name::<TimestampNanosecondArray>` exactly as `ensure_begin_non_decreasing` already
does; convert the resulting `i64` with `DateTime::from_timestamp_nanos` to get the
`Option<DateTime<Utc>>` the field wants.

`write_partition` sends its `PartitionRowSet` unconditionally (`:230`), so guard the read: when
`rows.num_rows() == 0`, set `max_sort_key_time` to `None` instead of indexing the `begin` column.
Zero rows is reachable in practice, not just theoretically — `append_call_tree` appends nothing
when the root is `None` (`span_table.rs:126-148`) and `CallTreeBuilder::finish` returns `None` on an
empty stack (`call_tree.rs:82-87`), which happens whenever every event is filtered out by the chain
range or a block carries only async events (`thread_block_processor.rs:95-101`).

Also bump `SCHEMA_VERSION` (`:38`) 2 → 3 in the same file — the lever `1429` used on this exact
view. `get_file_schema_hash` changes, so every pre-existing `thread_spans` JIT partition becomes
stale by schema hash and rebuilds automatically on its next query, carrying `max_sort_key_time`,
with no admin `retire_partitions` call required (see Trade-offs).

Note this is **not** a SQL-visible change: `SCHEMA_VERSION` feeds only the partition file-schema
hash (`get_file_schema_hash` returns `vec![SCHEMA_VERSION]`, `:325-327`). The Arrow schema users
actually query is built in `span_table.rs:50-84` and is untouched by this plan, so every existing
`thread_spans` dashboard keeps working unchanged — the rebuild is invisible apart from a one-off
first-query latency bump.

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
  nullable.

  Appending last is required twice over, and the second reason is the more durable one: **the SQL
  layer is this project's stable interface**, because users have dashboards built on it. Adding a
  trailing nullable column is purely additive — `SELECT *` consumers gain a column they can ignore,
  positional readers are unaffected, and every existing column keeps its name, type and ordinal.
  Inserting it mid-list (say, next to `max_event_time`, where it reads more naturally) would break
  both the bridge and those dashboards. Don't. No `sql_arrow_bridge.rs` change is needed: `TIMESTAMPTZ` already maps to a nullable
  `TimestampColumnReader` (`:330-337`, `:165-194`), the same path `min_event_time` uses.
- **Leave the pruning predicate alone.** `LivePartitionProvider::fetch`'s query-range branch prunes
  with `min_event_time <= $3 AND max_event_time >= $4` (`partition_cache.rs:375-376`). That must keep
  reading `max_event_time`: it is the max span *end*, so it is the only bound that conservatively
  covers a span whose `begin` precedes the query range but whose `end` reaches into it. Narrowing it
  to `max_sort_key_time` would silently prune partitions that hold matching rows. Worth a comment at
  the predicate, since the new column will look like a tempting tightening.
- Extend `Partition::validate` (`partition.rs:90-117`) with **one** structural invariant: reject a
  non-NULL `max_sort_key_time` when the event-time range is absent (an empty partition has no
  sort-key bound to record). The struct holds a single `event_time_range: Option<TimeRange>`
  (`partition.rs:14`), not two independently-nullable fields, so the clause is just
  `event_time_range.is_none() && max_sort_key_time.is_some()`; it only ever bites the `num_rows == 0`
  branch, and the "only one of min/max is NULL" case is already caught when the row is decoded
  (`partition_cache.rs:110-114`). This mirrors `validate`'s existing empty-partition clauses exactly
  in kind — presence, not value — and is guaranteed by construction, because both empty-partition
  branches of `finalize_partition_write` set `max_sort_key_time: None` alongside
  `event_time_range: None` (Part B §2).

  **Do not also assert `min_event_time <= max_sort_key_time <= max_event_time`**, even though it is
  true for every partition this plan's writer produces. Two properties of `validate` rule it out,
  and both belong in its rustdoc.

  First, **`validate` is enforced on the read path only**: its three callers are all in
  `partition_cache.rs` (`:128`, `:212`, `:458`), each propagating with `?`; nothing in
  `insert_partition` calls it. A violating partition is therefore written and committed silently and
  then hard-fails *reads* — and since `fetch_overlapping_insert_range` (`:60-91`) is not
  view-scoped, one bad row fails materialization for **every** view, not just the offending one. So
  the invariant must assert only what the writer guarantees by construction, never a hopeful
  expectation.

  An ordering clause does not clear that bar, because it depends on data the writer does not
  control. `min_event_time`/`max_event_time` are derived from `block.begin_ticks`/`end_ticks`
  (`thread_spans_view.rs:201-214`), which are bound straight from the client's payload
  (`rust/ingestion/src/web_ingestion_service.rs:192`) with no validation in ingestion, in analytics,
  or as a CHECK constraint. A block with `begin_ticks > end_ticks` yields an inverted
  `event_time_range` — today that is merely a silently odd row; under an ordering clause it becomes
  an unsatisfiable invariant and therefore a lakehouse-wide read outage triggerable by one
  malformed client. The row-level property the plan actually relies on is asserted where it is
  cheap and safe instead: `ensure_begin_non_decreasing` at write time, plus the no-DB
  `call_tree_tests.rs` bounds test (Step 8).

  Second, the invariant applies to **every** view's partitions, not just `thread_spans`'. Any future
  view populating `max_sort_key_time` must re-check the structural clause against its own writer.
  `BlocksView` illustrates why an ordering clause would have been the wrong shape here too: its
  `Concatenated` sort key is `insert_time` while its `event_time_range` is
  `[min begin_time, max insert_time]` (`blocks_view.rs:177-183`, itself carrying a
  `//todo: make more robust`), so a client clock skewed ahead of the server could put a block's
  `begin_time` above its `insert_time`.

  For the record, the ordering property *does* hold by construction for `thread_spans` — it is the
  basis of the soundness argument below, just not of a runtime assertion. Every emitted row's
  `begin` is either a real event time already filtered to `>= begin_range_ns`
  (`call_tree.rs:139-141`, assigned `:150`) or `begin_range_ns` itself (`:201`, `:109`), with
  `begin_range_ns`/`end_range_ns` derived from the chain's first/last block ticks
  (`thread_spans_view.rs:105-113`) through the same `ConvertTicks` instance and the same monotone
  formula — `delta_ticks_to_ns` and `delta_ticks_to_time` are the identical expression
  (`time.rs:120-129`) — that produces `min_event_time`/`max_event_time` (`:201-214`). Blocks are
  sorted ascending by `(begin_ticks, end_ticks)` (`jit_partitions.rs:243-248`), so each chain's
  range nests inside the partition's. Postgres's microsecond truncation is monotone and applied by
  the same encoder to all three values, so the non-strict inequalities survive it.

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
  holds there too: `FlushMonitor::Flush`
  (`unreal/MicromegasTelemetrySink/Private/FlushMonitor.cpp:45-54`) flushes the shared log/metric/
  net/image streams and its *own* thread stream directly (`:47-52`), but reaches other threads'
  streams only through `ForEachThreadStream(&MarkStreamFull)` (`:51`), whose callback does nothing
  but `MarkFull()` (`:12-15`); the actual swap always runs
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
   add the inherent `EventBlock::close_at(&mut self, end: DualTime)` alongside the retained
   self-stamping `close()` (`event/block.rs`) — `TracingBlock` and `TracingBlock::new` are unchanged,
   and no constructor call site moves; then, in each of the four `dispatch.rs` flush paths, read the
   replacement block's `begin` before handing the `Arc` to `replace_block` and pass it to
   `close_at` instead of calling `close()`. No new `DualTime::now()` is introduced, so the
   under-the-lock requirement is met without moving any code. New `rust/tracing/tests/` test following
   **emit → flush → emit → flush** (a second emit is required — the flush paths early-return on
   `is_empty()`, so flushing twice in a row yields only one block) that asserts the first closed
   block's `end` == the second's `begin`.
2. **Migration**: add `upgrade_v7_to_v8` in `rust/analytics/src/lakehouse/migration.rs` (nullable
   `max_sort_key_time TIMESTAMPTZ`, no backfill, comment stating NULL = "not recorded", modeled on
   `upgrade_v6_to_v7`'s column-add at `:439` — note that function also builds the exclusion
   constraint, which has no analogue here); bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to 8; **and wire
   the new step into the ladder** in `execute_lakehouse_migration` (`:53-106`) as an
   `if 7 == current_version { … }` block. The bump without the ladder entry is not a no-op: the
   `assert_eq!(current_version, LATEST_LAKEHOUSE_SCHEMA_VERSION)` at `:104` (and `:48`) panics on
   every service start.
3. **`Partition` + reads**: new field/accessor + the single structural `validate` clause
   (`partition.rs` — *not* an ordering clause, see Part B §4); add the
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
     as the mechanism and goes stale on both counts once the outgoing block is closed with the
     replacement's own `begin`.
   - `partition.rs` / `write_partition.rs`: field and fold-rule rustdoc (including why any-None
     poisons the partition value).
   - `doc/how_to_query/README.md`'s `#### list_partitions` Returns table (`:475-488`): add
     `max_sort_key_time`, and, while there, the three columns it's already missing (`num_rows`,
     `partition_format_version`, `sort_order`) — see Documentation.
   - `mkdocs/docs/admin/functions-reference.md`'s `list_partitions()` Returns table (`:53-67`): add
     `max_sort_key_time`, and the already-missing `partition_format_version` — see Documentation.
8. **Unit tests (no DB)**: extend `rust/analytics/tests/thread_spans_ordering_tests.rs`
   (`make_partition` helper at `:35-51`, 6 call sites): (a) two partitions whose
   `[min,max_event_time]` overlap
   but whose `max_sort_key_time` clears the next `min_event_time` → accepted; (b) same shape with
   NULL `max_sort_key_time` → still rejected (legacy fallback preserved); (c) genuine overlap in
   `max_sort_key_time` → rejected. These three need *per-call* control of the new field, so unlike
   the other four files' helpers this one needs a new `max_sort_key_time: Option<DateTime<Utc>>`
   parameter (or a sibling helper), not just a hardcoded field in its body. Update the other four
   test files with `Partition` literals
   (`per_file_scan_ordering_tests.rs`, `blocks_view_merge_ordering_tests.rs`,
   `sql_batch_view_merge_ordering_tests.rs`, `log_stats_ordering_tests.rs` — one or two helpers
   each) for the new field. Also add the running-max/`None`-poisoning fold test to
   `write_partition_tests.rs` (see Testing Strategy) — a no-DB test too, just against
   `write_rows_and_track_times` directly rather than the check.

   Also add a **row-bound test** to `rust/analytics/tests/call_tree_tests.rs`, pinning with no DB
   the two properties the new `max_sort_key_time` code actually depends on:

   (a) an event outside the chain range is **dropped, not clamped in** (`call_tree.rs:139-144`), so
   no row's `begin` can escape its partition's `[begin_range_ns, end_range_ns]`; and
   (b) preorder rows are non-decreasing on `begin`, so **the last row is the max** — the single
   assumption behind "read the last row's `begin`" in Part B §3.

   That file already drives `CallTreeBuilder` through the `ThreadBlockProcessor` trait with
   synthetic events (`:6-41`). Everything else needed is public too: `SpanRecordBuilder`
   (`span_table.rs:35`, `with_capacity` `:87`, `append_call_tree` `:126`, `finish` `:150`, already
   used no-DB by `dictionary_key_overflow_tests.rs`), `ensure_begin_non_decreasing`, and
   `ConvertTicks::from_meta_data`/`delta_ticks_to_time` (`time.rs:86`, `:120`). No new `pub` surface
   is required.

   Deliberately **not** a "cut-position" test parameterized over hour-seam vs. forced cut: block
   ticks never enter `CallTreeBuilder` — the `ThreadBlockProcessor` trait carries only
   `(block_id, event_id, scope, ts)` (`thread_block_processor.rs:11-27`) and the cut is modelled
   entirely by the `begin_range_ns`/`end_range_ns` handed to `CallTreeBuilder::new`
   (`call_tree.rs:52`). Varying the "cut position" therefore just varies two test-local constants,
   and both sides of a `P.last_begin < Q.min_event_time` assertion would be values the test chose
   itself — it would restate the claim, not test it. The claim that the cut *cause* is irrelevant
   is carried by the soundness argument (single-writer producer, "Why this is sound"), which no
   unit test at this layer can pin; what (a) and (b) pin is the part that is actually code.
9. **DB regression test** in `thread_spans_ordering_db_test.rs` — **hour-seam**, built from the
   existing `thread_spans_ordering_across_partitions` (`:208-437`) per the Repro Steps, *not* from
   the tick-rewriting pattern (see there for why fabricated ticks yield an empty, vacuous test).
   Two options, and the first is preferred:
   - **Preferred — delete the workaround in place.** Remove the 200 ms sleep and the throwaway
     "spacer" block (`:239-254`) from `thread_spans_ordering_across_partitions`, and replace that
     comment with one explaining that the shared flush stamp (Part A) and `max_sort_key_time`
     (Part B) make the seam safe without a manufactured gap. Its existing assertions (two-plus
     partitions `:366-369`, no `SortExec` `:400-403`, non-decreasing `begin` `:425`,
     `total_rows >= 2` `:431-434`) then *become* the
     regression assertions, and add one more: every partition the query scanned has a non-NULL
     stored `max_sort_key_time`. This is strictly better than a new test — the workaround's
     removal is itself the regression signal, and leaving it in place would leave a test that
     actively hides the bug this plan fixes. (The test already runs raw `sqlx` against
     `lake.db_pool` at `:271` and already queries `list_partitions()` at `:350-362` filtered to this
     `view_instance_id`, so either route reaches the new column with no extra scaffolding.)

     **Make the overlap deterministic rather than inheriting the buffer-swap width.** Removing the
     sleep and spacer leaves a real overlap, but only one buffer swap wide, and the stored bounds
     are Postgres `TIMESTAMPTZ` (microseconds) compared with a strict `>`. On a fast machine that
     overlap can truncate to *equality*, in which case the guard does not trip and the test passes
     even with the fix reverted — a regression test that silently stops biting. Fix it in the UPDATE
     the test already runs at `:271-280`: also push block 2's `begin_ticks` back by a fixed delta
     (a few milliseconds' worth of ticks) so the overlap is large and reproducible. This is safe
     precisely where the wholesale tick-fabrication pattern is not — lowering only `begin_ticks`
     *widens* the chain's `[begin_range_ns, end_range_ns]` window, so no event is filtered out by
     `call_tree.rs:139-143` and the partition still carries rows. It also cannot invert the block
     (`begin_ticks` only decreases), so it stays clear of the degenerate shape discussed under
     `validate` in Part B §4.
   - **Alternative**, if keeping the existing test untouched is preferred: add a sibling
     `#[ignore]`d `#[tokio::test]` that is a copy without the sleep/spacer. Costs a near-duplicate
     ~200-line test and leaves the misleading workaround in the original.

   Note the geometry it exercises is the *legacy* strictly-overlapping one — the blocks are
   ingested through the unfixed in-repo producer path (`ThreadStream` + `replace_block` directly,
   not the `dispatch.rs` flush that Part A changes), so the test keeps covering exactly the case
   Part B must tolerate forever.

   **Plus a persistence round-trip test**, in the style of `net_spans_retire_overlap_db_test.rs`
   (which inserts synthetic `lakehouse_partitions` rows with no ingestion, no parquet and no query
   engine). Build a `Partition` literal with `max_sort_key_time: Some(t)`,
   run it through the production `insert_partition` (`write_partition.rs:415` — make it `pub` for
   test reachability, the same lever `1429` used on `update_partition`; mirror the explanatory
   "not intended as API" rustdoc already on `thread_spans_view::update_partition` at `:255-265`),
   then read it back through `PartitionCache::fetch_overlapping_insert_range` (`partition_cache.rs:60`,
   which needs only a `&PgPool`) and assert the value survives; add a sibling row with `None` to pin
   the legacy path.

   Four concrete constraints, all of which cost an hour each to rediscover:
   - `insert_partition`'s signature is
     `(&DataLakeConnection, &Partition, RetireMatch, &[TimeRange], Arc<dyn Logger>)` — no
     transaction and no `source_row_count` parameter.
   - It derives the source row count via `hash_to_object_count(&partition.source_data_hash)`, an
     `i64::from_le_bytes`, so the test's `source_data_hash` must be **exactly 8 bytes**. The
     `vec![0]` used by the ordering tests' helpers errors here.
   - The `Some(t)` and `None` rows need **disjoint insert ranges**, or the second insert's
     `RetireMatch` step (or the `lakehouse_partitions_no_overlap` exclusion constraint) removes or
     rejects the first.
   - `num_rows > 0` literals must carry `Some(file_path)` and `Some(event_time_range)` or
     `Partition::validate` rejects them on read. No object-store *file* is needed, but the test
     harness's `connect()` still requires `MICROMEGAS_OBJECT_STORE_URI` to be set.

   Use a fresh `view_instance_id` per test so the retire predicate is a no-op and no
   object-store file is ever referenced.

   This is deliberately separate from the end-to-end test rather than folded into it, because the
   two cover different risks. This change's plumbing failure modes — a missed bind in the
   positional `VALUES($1,…,$12, 2, $13)`, a column absent from one of the four SELECTs, a
   misordered append in the strictly-positional `list_partitions` schema — are all SQL-shaped, and
   no unit test can reach them (the persistence layer is Postgres-specific: `TIMESTAMPTZ`, the
   `tstzrange`/gist exclusion constraint, the migration ladder; there is no testcontainers or
   embedded-Postgres harness in this repo). A ~50-line test that exercises exactly the INSERT and
   the SELECTs pins them directly, in seconds, instead of incidentally behind ~200 lines of
   ingestion and query machinery. The end-to-end test then has one job — proving the scan path —
   and this one has the other: proving the columns.

   A *second* DB test for the forced intra-bucket cut is deliberately **not** included. It would
   re-exercise the same write-and-check path the hour-seam test already covers, differing only in
   why the cut happened — which the soundness argument says is irrelevant. It could not even assert
   through a live query:
   `ThreadSpansView::jit_update` (`thread_spans_view.rs:371-374`) runs with the default
   `max_nb_objects` (`JitPartitionConfig { block_order: EventTime, ..Default::default() }`, so
   `20 * 1024 * 1024`) ahead of any scan, and would see small-`max_nb_objects` partitions as stale
   by source hash and rewrite them as one partition before the `Concatenated` check ever ran.
   Working around that needs a bespoke `PartitionCache` + `make_partitioned_execution_plan`
   assertion path, in an `#[ignore]`d test that only runs when someone remembers — a lot of
   machinery for coverage that a plain `cargo test` unit test delivers better. (Grouping itself is
   untouched by this plan, and mid-bucket cuts are already covered by
   `thread_spans_interrupted_run_reconverges` (`:1125`) and
   `thread_spans_cross_run_regrouping_replaces_stale_partition` (`:1378`), both `max_nb_objects: 4`.)
10. **Sanity**: run the hour-seam DB regression test from Step 9 explicitly
   (`cargo test --test thread_spans_ordering_db_test -- --ignored`, with a local stack up). It is
   `#[ignore]`d per the file's convention, so it does **not** run in `cargo test` or in
   `build/rust_ci.py` — and it is the only test in the plan that proves `update_partition` actually
   persists `max_sort_key_time` and that the scan path reads it back. Everything else in the change
   can be green while that link is broken. Confirm it fails first with the sleep/spacer removed but
   the fix reverted, so the test is known to bite. Run the new persistence round-trip test from
   Step 9 in the same pass (also `#[ignore]`d, also CI-invisible). Run
   `thread_spans_batched_generation_matches_per_segment` (`#[ignore]`d) too — grouping
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
    `rust/analytics/Cargo.toml` sets `publish = false`, so a break in either is downstream-visible —
    but only the `**Analytics:**` entry actually needs a **Minor breaking
    change** clause, matching every comparable entry already in the file (e.g. `## Unreleased` line
    8, v0.29.0 lines 13-15/18-20/24/26). The `**Tracing:**` entry needs **none**: the `TracingBlock`
    trait is untouched, `TracingBlock::new` and `EventBlock::close` keep their signatures and
    behavior, and `EventBlock::close_at` is a pure inherent addition — mention it as an
    addition and stop. The `**Analytics:**` entry
    must name `PartitionRowSet` and `Partition` (both all-public-field structs, so downstream struct
    literals break), `PartitionRowSet::new`'s new third argument, and
    `write_rows_and_track_times`'s widened return type. It should also state what is *not* broken,
    since that is what users care about: no SQL-visible change beyond one trailing nullable column
    on `list_partitions()`; `thread_spans`' queryable schema is identical, so existing dashboards
    and saved queries keep working. The `**Analytics:**`
    entry also needs an **Operational note**, following the shape of `1429`'s v0.29.0 entry
    (`CHANGELOG.md:13`, which bumped both views 1 → 2; this one bumps `thread_spans` alone):
    `ThreadSpansView::SCHEMA_VERSION` bumps 2 → 3, so every existing `thread_spans` JIT partition is
    stale after deploy and rebuilds automatically on first query — no admin action, but expect a
    one-off latency bump on the first query per stream.

## Files to Modify

| File | Change |
|---|---|
| `rust/tracing/src/time.rs` | Derive `Clone, Copy, PartialEq, Eq` on `DualTime` (`Copy` to read `begin` out of an `Arc`; `PartialEq` for the new test) |
| `rust/tracing/src/dispatch.rs` | 4 flush sites: read the replacement block's `begin`, then `close_at(begin)` instead of `close()` |
| `rust/tracing/src/event/block.rs` | Inherent `EventBlock::close_at`, alongside the retained `close()` — `TracingBlock` and `TracingBlock::new` are unchanged |
| `rust/tracing/tests/` | New boundary-stamp unit test (emit → flush → emit → flush, compare `end`/`begin` via `InMemorySink`) |
| `rust/analytics/src/lakehouse/migration.rs` | v7→v8: nullable `max_sort_key_time TIMESTAMPTZ`; bump version constant; **wire the step into the ladder** |
| `rust/analytics/src/lakehouse/partition.rs` | New field + accessor; one structural `validate` clause (no ordering assertion — see Part B §4) |
| `rust/analytics/src/lakehouse/partition_cache.rs` | Add column to 4 SELECTs / 3 struct builds (`sort_order` read pattern, not `file_path`'s) |
| `rust/analytics/src/lakehouse/list_partitions_table_function.rs` | Arrow schema + both SELECTs |
| `rust/analytics/src/lakehouse/write_partition.rs` | `PartitionRowSet` field; all-Some running-max fold in `write_rows_and_track_times`; `PartitionWriteResult`; explicit-column-list INSERT; `insert_partition` made `pub` (test reachability) |
| `rust/analytics/src/lakehouse/thread_spans_view.rs` | Set `max_sort_key_time` from last row's `begin`; bump `SCHEMA_VERSION` 2 → 3 for self-healing rebuild |
| `rust/analytics/src/lakehouse/{net_spans_view,async_events_block_processor,log_block_processor,image_block_processor}.rs` | Mechanical `max_sort_key_time: None` addition to each `PartitionRowSet` literal |
| `rust/analytics/src/lakehouse/partitioned_execution_plan.rs` | `partition_bounds` EventTime arm; rustdoc + error string |
| `rust/analytics/src/lakehouse/view.rs` | Rewrite `Concatenated` residual-caveats note (`:166-187`) |
| `rust/analytics/src/lakehouse/jit_partitions.rs` | Module-doc amendment only (`:17-23`) — **no code change** |
| `rust/analytics/tests/thread_spans_ordering_tests.rs` | New no-DB check tests; helper gains field |
| `rust/analytics/tests/{per_file_scan_ordering,blocks_view_merge_ordering,sql_batch_view_merge_ordering,log_stats_ordering}_tests.rs` | One-line helper updates for the new field |
| `rust/analytics/tests/write_partition_tests.rs` | Adapt to `write_rows_and_track_times`'s widened return |
| `rust/analytics/tests/call_tree_tests.rs` | New no-DB row-bound test: out-of-range events dropped not clamped; last preorder row is the max `begin` |
| `rust/analytics/tests/thread_spans_ordering_db_test.rs` | Drop the sleep/spacer workaround in `thread_spans_ordering_across_partitions` (`:239-254`), widen the seam deterministically via block 2's `begin_ticks`, and assert non-NULL `max_sort_key_time` — the workaround's removal *is* the regression test |
| `rust/analytics/tests/` (new file) | Persistence round-trip DB test: `Partition` → `insert_partition` → `partition_cache`, `Some(t)` and `None` rows (net_spans_retire_overlap style — no ingestion/object store/parquet) |
| `doc/how_to_query/README.md` | Add `max_sort_key_time` (and the already-missing `num_rows`, `partition_format_version`, `sort_order`) to the `list_partitions` Returns table |
| `mkdocs/docs/admin/functions-reference.md` | Add `max_sort_key_time` (and the already-missing `partition_format_version`) to the `list_partitions()` Returns table |
| `CHANGELOG.md` | Two entries under `## Unreleased`: `**Analytics:**` (breaking + operational note) and a new `**Tracing:**` subsection (additive) — see Step 12 |

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
- **Rejected write-side alternative: compute the max in the shared fold.** Instead of a
  `PartitionRowSet` field, `write_partition_from_rows` could take a
  `max_sort_key_column: Option<Arc<String>>` and `write_rows_and_track_times` could compute the
  running max off each `RecordBatch` itself. That trades 12 `PartitionRowSet` construction sites
  (only 5 of which the compiler forces) for 6 `write_partition_from_rows` call sites plus a
  column-lookup-and-downcast helper in the shared path — on a function that already carries
  `#[expect(clippy::too_many_arguments)]` with 10 parameters. Roughly a wash on touch points and
  worse on both clarity and exactness (a batch-wide max recomputed generically, versus reading the
  last row of an already-verified-monotonic batch at the one site that knows the invariant holds),
  so the producer-side field wins. Deferring the computation into `finalize_partition_write` is
  worse still: that function sees only the writer and the parquet footer (`:686-693`), so it would
  have to decode per-row-group column statistics and know which column is the sort key.
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
  identical in kind to `1429`'s. During a **mixed-version rollout** the bump also behaves exactly as
  `1429`'s did, which is worth stating rather than discovering: the `lakehouse_partitions_no_overlap`
  exclusion constraint is scoped by `file_schema_hash` (`migration.rs:502-510`) precisely so
  old- and new-hash partitions may legally coexist, but `RetireMatch::Overlap`'s SQL
  (`write_partition.rs:241-255`) carries no hash predicate — so the first new-version node to rebuild
  a stream retires the old-hash partition and schedules its parquet file for deletion, out from under
  any still-running old-version reader that had it cached. Self-limiting (the old reader's next query
  rebuilds under its own hash) and unchanged in kind by this plan, but it is why the bump wants a
  short rollout window rather than a long one. That cost is worth paying here because the plan's stated goal is
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
Int32`, `sort_order List<Utf8>`, `max_sort_key_time Timestamp(Nanosecond)`), following that table's
existing abbreviated type convention rather than the schema's literal Arrow types (which are
`Timestamp(Nanosecond, Some("+00:00"))` and `List(Field "tag", Utf8)`), so the doc covers every
column the schema carries.

`mkdocs/docs/admin/functions-reference.md`'s `list_partitions()` Returns table (`:53-67`) is the
published docs-site equivalent and has its own copy of the same column list — already missing
`partition_format_version` (it does carry `num_rows` at `:66` and `sort_order` at `:67`), and about
to go stale on `max_sort_key_time` too if left alone. Fix it in the same pass: add
`partition_format_version` and `max_sort_key_time`. Note this table uses **prose** type names
(`String`, `Timestamp`, `Integer`, `Binary`, `List(String)`) rather than Arrow ones — `num_rows` is
`Integer`, not `Int64` — so write the two new rows as `Integer` and `Timestamp` to match its other
thirteen, not in the `doc/how_to_query` table's style.

## Testing Strategy

- **Producer boundary test** (`rust/tracing/tests/`): emit → flush → emit → flush, then assert the
  first closed block's `end` equals the second's `begin` — they are literally the same `DualTime`,
  since the flush closes the outgoing block with the replacement's own `begin`. The second emit is
  mandatory, not stylistic: the flush paths early-return on `is_empty()`, so back-to-back flushes
  leave the sink with a single block. The sink only ever sees closed blocks, so the live replacement
  is unreachable from it either way.
- **No-DB check tests** (`thread_spans_ordering_tests.rs`): recorded-bound accepted / NULL
  fallback rejected / genuine overlap rejected — the check-level contract.
- **No-DB row-bound test** (`call_tree_tests.rs`, Implementation Step 8): out-of-range events are
  dropped rather than clamped in, and the last preorder row carries the max `begin` — the two code
  properties `max_sort_key_time` rests on, in plain `cargo test`. (The soundness claim proper —
  that the cut *cause* is irrelevant — is a producer argument, not something testable at this
  layer; see Step 8.)
- **DB regression test**: `thread_spans_ordering_across_partitions` with its sleep/spacer
  workaround removed and the seam widened deterministically via block 2's `begin_ticks`
  (Implementation Step 9), asserting the live `view_instance` query succeeds,
  `begin` is non-decreasing across the scan, and every scanned partition has a non-NULL
  `max_sort_key_time` — the end-to-end contract, and the only place that proves `update_partition`
  persists the new column and the scan path reads it back. That workaround exists today precisely
  because of this bug, so deleting it is the sharpest available regression signal; the deliberate
  widening is what keeps that signal from being lost to microsecond truncation of a
  buffer-swap-sized overlap. See Step 9 for
  why a second, *forced-cut* DB test is not included. Because it is `#[ignore]`d it runs neither in
  `cargo test` nor in `build/rust_ci.py`, so Step 10 calls it out as an explicit, must-run step
  rather than leaving it to whoever remembers.
- **Persistence round-trip test** (new file, Implementation Step 9): a `Partition` carrying
  `max_sort_key_time: Some(t)` survives `insert_partition` → `partition_cache`, and a `None` row
  stays `None`. This is the only test that isolates the SQL plumbing — the positional INSERT bind
  and the four SELECTs — which is where this change's realistic failure modes live and which no
  unit test can reach. Cheap enough (~50 lines, no ingestion or object store) that it is worth
  running alongside the end-to-end test rather than instead of it.
- **Running-max / `None`-poisoning fold** (`write_partition_tests.rs`): feed
  `write_rows_and_track_times` several out-of-order `PartitionRowSet`s — some `Some`, one `None` —
  over its hand-built channel (widen it past `channel(1)`, or spawn the sender, so the second send
  doesn't deadlock) and assert the result is a running `max` (not "last row set
  wins") and that the single `None` poisons the partition-level value to `None`. This is the only
  genuinely new logic in the write path and the only place it is exercised, since
  `thread_spans_view` sends exactly one row set.
- **Existing suites**: `thread_spans_batched_generation_matches_per_segment` unmodified (grouping
  untouched); `thread_spans_ordering_across_partitions` modified only by deleting its workaround
  (above); `write_partition_tests.rs` adapted (plus the new fold test above); the six
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
