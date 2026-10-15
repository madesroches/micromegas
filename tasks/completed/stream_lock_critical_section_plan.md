# Shorten the Log and Metric Stream Critical Sections Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1669

## Overview

Every log call goes through the `Dispatch::log_stream` mutex, and every metric call goes through
`Dispatch::metrics_stream`. Each recording thread waits on these two global locks, so any work
done while holding them adds to contention. Today that work includes more than the event push:

- the sink call
- message formatting and string interning
- the property-arena hash

This plan moves all of that out of the lock, so the critical section is a push, plus a block
swap when the block is full. The stream layout, the wire format, the `EventSink` trait and the SQL
surface don't change. A criterion run before and after the change measures the effect, using the
`properties` bench from #1664.

## Current State

All references are to `rust/tracing/src/dispatch.rs` unless noted.

**Flush paths.** `flush_log_buffer` (`:1039`) and `flush_metrics_buffer` (`:835`) do all of the
following under the stream lock:

- read `next_offset`
- allocate the replacement block, a `Vec::with_capacity` of 10 MiB for logs and 1 MiB for metrics
  by default (`transit/derive/src/declare_queue.rs:78`)
- swap it in and close the old block
- call `get_sink().on_process_*_block(old)`

`flush_image_buffer` (`:635`) already drops the lock before calling the sink. The thread-stream
flush has no lock and is out of scope.

**Push paths.** All five log paths and six metric paths (eleven push sites) end with the same code:

```rust
if stream.is_full() { drop(stream); self.flush_*_buffer(); }
```

Between that `drop` and the re-lock inside the flush, other threads keep pushing into the full
block. Each of them also sees `is_full()` and calls the flush, which then sends a nearly empty
block. They can also push past the 128-byte `hint_max_obj_size` margin, which makes the
`Vec` reallocate (copy 10 MiB) under the lock.

**Work under the lock on the push path:**

| Path | Under the lock besides the push |
|---|---|
| `log` (`:861`), non-static message | `args.to_string()` (`:878`) |
| `log_tagged_to_sink` (`:903`) | `args.to_string()` (`:918`) |
| `log_interop` (`:928`) | `intern_string(desc.target)` (a global interner) and `args.to_string()` (`:936-944`) |
| `log_dyn_tagged_to_sink` (`:970`), `log_interop_dyn_tagged` (`:1002`) | `arena.get_or_insert(&merged)`: hashes the set, and on a miss allocates the owned copy |
| `log_interop_dyn_tagged` (`:1002`) | also `intern_string(desc.target)` (`:1028`) |
| `tagged_*_metric_dyn` (`:779`, `:807`) | same `get_or_insert` |
| every push into a fresh block | the minor page faults for the buffer pages it touches first |

`PropertyArena::get_or_insert` (`rust/tracing/src/property_arena.rs:66`) computes `content_hash`
before doing the lookup. `merge_properties` (`:31`) runs outside the lock and returns a plain
`Vec<DynProperty>`.

**Sink called under the lock.** A sink that logs from inside `on_process_log_block`
re-enters `log_stream` and deadlocks. `log_dyn_tagged_to_sink` already moved `on_log` out of the
lock for this reason (comment at `:983`).

**Callers that depend on synchronous flush.** Several tests and `capi` (`rust/capi/src/lib.rs:268`)
call `flush_*_buffer()` and expect the sink to hold the block when the call returns. This plan
keeps that guarantee.

**Benchmark.** `rust/tracing/benches/properties.rs` (#1664) has single-thread `log` and `metric`
groups and a `contention` group that runs 1, 4 and 8 threads over `log/static`,
`log/dyn_high_card`, `metric/plain` and `metric/dyn_high_card`. Its `BenchSink` drops blocks,
and the bench runs with production buffer sizes (10 MiB logs, 1 MiB metrics).

## Design

### 1. Swap under the lock, everything else outside

Add one generic helper, called with the stream guard held:

```rust
fn swap_block<Q>(
    stream: &mut EventStream<EventBlock<Q>>,
    buffer_size: usize,
    process_id: uuid::Uuid,
) -> Arc<EventBlock<Q>>
```

The helper does what the flush functions do under the lock today:

1. Build the replacement with `EventBlock::new(buffer_size, process_id, stream_id, next_offset)`,
   where `next_offset = old.object_offset() + old.nb_objects()`.
2. Call `replace_block`, then `close_at(new.begin)` on the old block.

`begin` is stamped under the lock, so `block[k].end == block[k+1].begin` still holds and no event
in the old block is later than its `end`.

The two callers:

- **Push paths.** Each push site replaces `if full { drop; flush }` with a call to a shared
  `on_log_pushed(guard)` / `on_metrics_pushed(guard)`. If the block is full, that function swaps
  **while still holding the guard from the push**, drops the guard, then calls the sink. The full
  block is replaced in the same critical section that filled it, so no other thread can push into
  it, nothing pushes past the margin for events within the 128-byte size hint, and duplicate flushes of nearly empty blocks are no longer
  possible.
- **`flush_log_buffer` / `flush_metrics_buffer`** (explicit flushes and the flush monitor):
  lock, return if empty, `swap_block`, drop the guard, then call the sink. The sink still has the
  block when the function returns.

Log and metrics share `swap_block`; `on_log_pushed` / `on_metrics_pushed` each name their stream's sink method. Today `flush_log_buffer` and `flush_metrics_buffer` are
copies of each other. After this change, each is a few lines naming its stream, buffer size and
sink method. Image and thread streams don't change.

### 2. Hash merged properties before the lock

`merge_properties` returns a `MergedProperties<'a>` that holds the sorted `Vec<DynProperty<'a>>`
and its `content_hash`, computed at merge time. It implements `Deref<Target = [DynProperty<'a>]>`,
so sink calls (`on_log(desc, &merged, ...)`) and the interop filter compile unchanged.
`PropertyArena::get_or_insert` takes `&MergedProperties` and uses the stored hash. With that
signature, every caller has to pass a hashed set, so the compiler lists every call site.
Arena hits now do only an index lookup and a compare under the lock. A miss still allocates
under the lock (see Trade-offs).

### 3. Format and intern before the lock

In `log`, `log_tagged_to_sink` and `log_interop`, build the `DynString` message and look up the
interned target before taking the lock, as `log_dyn_tagged_to_sink` already does. In
`log_interop_dyn_tagged`, intern `desc.target` before the lock too.

### Behavior notes

- **Block order at the sink.** The sink is now called after the lock is released, so two
  racing flushes of the same stream can reach the sink in reverse order. For example, a
  push-path flush can race the flush monitor. Swaps are still serialized, so `event_offset` and
  `begin`/`end` stay contiguous and correct. The image stream already behaves this way.

## Implementation Steps

### Phase 0: Baseline benchmark, before any change

1. In `rust/tracing/benches/properties.rs`, add `("log/formatted", log_case("formatted"))` to the
   `bench_contention_group` cases. Moving formatting out of the lock is one of the changes, and
   the contention group doesn't cover it yet. Commit this bench-only change on its own.
2. From `rust/`, with the bench change built and no tracing changes applied, save a baseline:
   `cargo bench -p micromegas-tracing --bench properties -- --save-baseline lock-before`.
   Run it on a quiet machine. Run it a second time and check that the two runs agree. If they
   don't, re-run before trusting the baseline.

### Phase 1: Arena hash

3. `rust/tracing/src/property_arena.rs`: add `MergedProperties`, make `merge_properties` return it,
   and change `get_or_insert` to take `&MergedProperties` and use its hash.
4. Update the callers in `dispatch.rs`, and `rust/tracing/tests/property_arena_tests.rs`.

### Phase 2: Swap and formatting

5. `dispatch.rs`: add the `swap_block` helper and `on_log_pushed` / `on_metrics_pushed`. Rewrite
   `flush_log_buffer` and `flush_metrics_buffer` on top of them, and switch all eleven push sites
   over: log paths `log`, `log_tagged_to_sink`, `log_interop`, `log_dyn_tagged_to_sink`,
   `log_interop_dyn_tagged`; metric paths `int_metric`, `float_metric`, `tagged_float_metric`,
   `tagged_integer_metric`, `tagged_float_metric_dyn`, `tagged_integer_metric_dyn`.
6. `dispatch.rs`: move message formatting and `intern_string` above the lock in `log`,
   `log_tagged_to_sink` and `log_interop`, and `intern_string` in `log_interop_dyn_tagged`.

### Phase 3: Measure after

7. Run `cargo bench -p micromegas-tracing --bench properties -- --baseline lock-before` on the
   same machine, under the same conditions as the baseline.
8. Record a before/after table in the Results section of this plan:
   - the `contention` group at 1/4/8 threads for every case
   - the single-thread `log` and `metric` groups, to check that the uncontended path didn't
     regress (the `MergedProperties` hash moves rather than adds work)

   Put the headline numbers in the CHANGELOG entry. If 4- and 8-thread contention is still
   significant, note that sharding the streams is the next step, as the issue says.

### Phase 4: Finish

9. Add the CHANGELOG entry. Run `cargo fmt`, then
   `cargo clippy --workspace -- -D warnings`, then `cargo test -p micromegas-tracing`.

## Files to Modify

- `rust/tracing/benches/properties.rs`: add `log/formatted` to the contention group
- `rust/tracing/src/property_arena.rs`: `MergedProperties`, `get_or_insert` signature
- `rust/tracing/src/dispatch.rs`: swap helper, push and flush paths, formatting moved out of the lock
- `rust/tracing/tests/property_arena_tests.rs`: new signature
- `rust/tracing/tests/stream_flush_tests.rs` (new)
- `rust/telemetry-sink/src/http_event_sink.rs`: restate the `try_push` comments (`:94-99`, `:202`, `:234`)
- `CHANGELOG.md`

## Trade-offs

- **Swap in the push path, not drop-then-flush.** The window where threads push past the full margin is gone,
  and so are the redundant tiny-block flushes.
- **Arena misses still allocate under the lock.** Building the owned copy before the lock would
  add allocations to every hit, which are the low-cardinality case that is free today. Only
  high-cardinality properties miss, and they also pay for formatting their value, which is
  already outside the lock.
- **Hash in `MergedProperties`, not a separate `hash` argument.** With a type, nobody can pass a
  hash that doesn't match the slice, and the compiler finds every call site. This changes the
  Rust API, which is acceptable here.
- **Sink calls outside the lock can arrive out of order.** Keeping them ordered would need a
  second lock held across the sink call, which brings back the deadlock risk for a guarantee
  that no consumer relies on. Blocks carry their own offsets and timestamps.

## Decisions

- Benchmark before and after is a required step: a baseline is saved before any tracing change,
  and the results are recorded in this plan and the CHANGELOG (user request).
- Image and thread streams are out of scope. The image stream already releases the lock before
  calling the sink. Thread streams have no shared lock.
- No pre-built spare block: allocating the replacement under the lock is a cheap untouched
  allocation; revisit with a spare if the contention benchmark shows page faults under the lock
  matter (user call).
- Sharding the streams is out of scope. Whether to do it depends on the after numbers.

## Documentation

- `CHANGELOG.md`: a Tracing entry with the before/after numbers. Add a **Minor breaking change**
  clause covering the `merge_properties` return type and the
  `PropertyArena::get_or_insert` signature.
- `rust/telemetry-sink/src/http_event_sink.rs`: the comments that say `try_push` must not emit
  telemetry because dispatch calls `on_process_*_block` under the stream mutex are no longer true.
  Restate the reason as recursion into the sink, or drop them.
- No mkdocs change. The capi threading-model note (`mkdocs/docs/native/index.md:171`, "per-stream
  `Mutex`") is still accurate.

## Testing Strategy

No-DB unit tests in a new `rust/tracing/tests/stream_flush_tests.rs`. They use
`init_in_memory_tracing()`, whose 1 KiB buffers force frequent flushes, and all are marked `#[serial]`.
Each test calls `set_max_level(LevelFilter::Trace)` first, like `test_macros.rs`: the max level
starts at Off, so otherwise every `info!` is a no-op.
The sink re-entrancy test skips `init_in_memory_tracing()` and uses its own `DispatchGuard` and
sink (see below):

- **Sink can log during block processing.** `init_in_memory_tracing()` can't be
  used, since it always installs `InMemorySink`; the test is still `#[serial]`. The new test file defines its own `CapturingSink` and `DispatchGuard`, modeled on
  `property_arena_tests.rs` (whose guard uses 64 KiB buffers): the guard calls `init_event_dispatch` with 1024-byte buffers
  and runs `shutdown_dispatch` + `force_uninit` on drop. The sink calls `info!` once (one-shot
  flag) from inside `on_process_log_block`. Run the body in a spawned thread and wait with
  `recv_timeout`, so a regression fails instead of hanging. Fill and flush the log stream, call
  `flush_log_buffer()` a second time, and assert that the nested event is observed in a later
  block. This deadlocks today, so it shows that the sink is called outside the lock.
- **Contiguous blocks after many flushes, single thread.** Log and record metrics until several
  blocks have flushed, then flush explicitly. For each stream, assert that
  `offset[k+1] == offset[k] + nb_objects[k]`, `end[k] == begin[k+1]`, and that
  `block.events.capacity_bytes() == 1024` (the configured size, so any reallocation is caught) for
  every log and metrics block. Keep messages under 128 bytes.
- **Concurrent recording, no lost or duplicated events.** Use 8 threads, each logging N events
  with a dynamic `id`. Assert that the block event counts sum to 8·N, that offsets are
  contiguous once the blocks are sorted by offset, and that every block except possibly the last
  is above the full threshold, i.e. its `len_bytes()` (events + arena) exceeds `capacity - 128` (no tiny duplicate flushes), and that every block's
  `capacity_bytes()` is still 1024. Keep messages under 128 bytes. This is the concurrency check that
  would otherwise fail silently.
- **Arena.** Update `property_arena_tests.rs`: equal merged sets still dedup, different sets
  don't, and `MergedProperties` hashes identical content identically however the static and
  dynamic properties were split.

The performance effect is measured by the before/after criterion run in Phases 0 and 3, not by
tests.

## Results

Median time per call (per round of threads for `contention`), `cargo bench -p micromegas-tracing
--bench properties`, release profile, 24-core WSL machine, not otherwise idle. The baseline
(`--save-baseline lock-before`) was re-taken on the unchanged code immediately before the
"after" run, because the first baseline had drifted by tens of percent by the time the change
was ready (see Noise below).

### Contention group

| Case | Before | After | Change |
|---|---|---|---|
| contention/log/static/1 | 15.257 ns | 15.886 ns | +4.1% |
| contention/log/static/4 | 342.18 ns | 333.62 ns | -2.5% |
| contention/log/static/8 | 875.11 ns | 876.54 ns | +0.2% |
| contention/log/formatted/1 | 55.539 ns | 57.564 ns | +3.6% |
| contention/log/formatted/4 | 687.59 ns | 536.64 ns | -22.0% |
| contention/log/formatted/8 | 2.0124 us | 1.6991 us | -15.6% |
| contention/log/dyn_high_card/1 | 325.82 ns | 361.09 ns | +10.8% |
| contention/log/dyn_high_card/4 | 3.5723 us | 4.4229 us | +23.8% |
| contention/log/dyn_high_card/8 | 8.3473 us | 10.034 us | +20.2% |
| contention/metric/plain/1 | 8.8725 ns | 9.7954 ns | +10.4% |
| contention/metric/plain/4 | 201.72 ns | 189.63 ns | -6.0% |
| contention/metric/plain/8 | 507.54 ns | 513.90 ns | +1.3% |
| contention/metric/dyn_high_card/1 | 245.93 ns | 307.26 ns | +24.9% |
| contention/metric/dyn_high_card/4 | 2.6441 us | 3.2430 us | +22.7% |
| contention/metric/dyn_high_card/8 | 5.9818 us | 8.0262 us | +34.2% |

### Single-thread groups

| Case | Before | After | Change |
|---|---|---|---|
| log/static | 15.430 ns | 15.826 ns | +2.6% |
| log/formatted | 57.187 ns | 60.603 ns | +6.0% |
| log/literals | 44.986 ns | 45.418 ns | +1.0% |
| log/set | 43.618 ns | 45.770 ns | +4.9% |
| log/dyn_low_card | 88.012 ns | 94.530 ns | +7.4% |
| log/dyn_high_card | 335.33 ns | 366.58 ns | +9.3% |
| log/dyn_mixed | 656.08 ns | 527.79 ns | -19.6% |
| metric/plain | 9.7111 ns | 9.8739 ns | +1.7% |
| metric/literals | 11.405 ns | 11.567 ns | +1.4% |
| metric/set | 10.213 ns | 10.987 ns | +7.6% |
| metric/literals_set | 145.67 ns | 130.93 ns | -10.1% |
| metric/dyn_low_card | 67.754 ns | 70.842 ns | +4.6% |
| metric/dyn_high_card | 241.58 ns | 244.23 ns | +1.1% |

### Reading the numbers

- `log/formatted` under contention is the clearest win: moving formatting out of the lock gives
  -22% at 4 threads and -16% at 8 threads. Single-thread cost is unchanged within noise.
- `log/static` and `metric/plain` under contention are unchanged within noise: their critical
  section was already a plain push.
- Single-thread `log` and `metric` are within about +-8% apart from two cases (`log/dyn_mixed`
  -20%, `metric/literals_set` -10%) that vary by that much between identical runs; no
  uncontended regression is visible.
- `dyn_high_card` (every event inserts a new set, so the arena miss allocates under the lock)
  reads +20-35% slower in this run, including at one thread where there is no contention. This
  is noise from the allocator-bound case, not the change: two identical baseline runs differed
  by up to 15% on these cases, and re-running the *unchanged* code against the first baseline
  read +18% (1 thread), +31-37% (4) and +43-46% (8). Measured back to back, the changed code was
  slightly faster than the unchanged code on `contention/log/dyn_high_card` (about 333 ns / 4.0
  us / 9.5-9.8 us against 340-352 ns / 4.3-4.4 us / 11.0-11.4 us at 1/4/8 threads).
- Contention is still significant at 4 and 8 threads for every case (8 threads cost 30-60x the
  single-thread call). Sharding the streams is the next step.
