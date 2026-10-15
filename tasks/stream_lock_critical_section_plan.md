# Shorten the Log and Metric Stream Critical Sections Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1669

## Overview

Every log call goes through the `Dispatch::log_stream` mutex, and every metric call goes through
`Dispatch::metrics_stream`. Each recording thread waits on these two global locks, so any work
done while holding them adds to contention. Today that work includes more than the event push:

- the sink call
- allocating the replacement block
- the page faults of a fresh block
- message formatting
- the property-arena hash

This plan moves all of that out of the lock, so the critical section is a push, plus a pointer
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
| `log_dyn_tagged_to_sink` (`:995`), `log_interop_dyn_tagged` (`:1032`) | `arena.get_or_insert(&merged)`: hashes the set, and on a miss allocates the owned copy |
| `log_interop_dyn_tagged` (`:1032`) | also `intern_string(desc.target)` (`:1028`) |
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

### 1. A ready spare block for the log and metrics streams

Only the two process-wide streams get a spare. CPU (thread) streams never do: there is one per
thread, so a spare each would hold an extra `threads_buffer_size` (10 MiB by default) per
recording thread, and with no shared lock there is no contention for it to relieve.

Add a generic `SpareBlock<Q>` in a new module `rust/tracing/src/event/spare_block.rs`. It holds
one pre-built, pre-faulted replacement block:

```rust
pub struct SpareBlock<Q> {
    slot: Mutex<Option<Arc<EventBlock<Q>>>>,
    buffer_size: usize,
    process_id: uuid::Uuid,
    stream_id: uuid::Uuid,
}
```

- `SpareBlock::new(buffer_size, process_id, stream_id)` leaves the slot empty. `Dispatch::new`
  calls `refill()` explicitly after constructing each spare.
- `refill(&self)` returns early if the slot is full. Otherwise it builds
  `EventBlock::new(buffer_size, process_id, stream_id, 0)` and calls `events.prefault()`, both
  outside any lock. Then it stores the block if the slot is still empty, and drops it otherwise.
- `take_or_new(&self)` takes the spare. If the slot is empty, it builds a block inline. This
  fallback has the same cost as today and only happens when a refill hasn't finished yet.

`Dispatch` gets `log_spare: SpareBlock<LogMsgQueue>` and `metrics_spare: SpareBlock<MetricsMsgQueue>`.
Both are created in `Dispatch::new` from the streams' ids and filled once there with `refill()`.

Lock order is always stream lock, then spare slot. `refill` takes only the slot lock, so the
order cannot be inverted.

### 2. Swap under the lock, everything else outside

Add one generic helper, called with the stream guard held:

```rust
fn swap_block<Q>(stream: &mut EventStream<EventBlock<Q>>, spare: &SpareBlock<Q>) -> Arc<EventBlock<Q>>
```

The helper does the following:

1. Take the spare.
2. Set `event_offset = old.object_offset() + old.nb_objects()`.
3. Set `begin = DualTime::now()`, then call `replace_block` and `close_at(begin)` on the old
   block.

`begin` is stamped under the lock, not when the spare is built, so `block[k].end == block[k+1].begin`
still holds and no event in the old block is later than its `end`. Both fields are already `pub`
on `EventBlock`, and the spare is reached with `Arc::get_mut`, so the swap allocates nothing.

The two callers:

- **Push paths.** Each push site replaces `if full { drop; flush }` with a call to a shared
  `on_log_pushed(guard)` / `on_metrics_pushed(guard)`. If the block is full, that function swaps
  **while still holding the guard from the push**, drops the guard, calls the sink and then
  `refill`s. The full block is replaced in the same critical section that filled it, so no other
  thread can push into it, and duplicate flushes of nearly empty blocks are no longer possible.
- **`flush_log_buffer` / `flush_metrics_buffer`** (explicit flushes and the flush monitor):
  lock, return if empty, `swap_block`, drop the guard, then sink and `refill`. The sink still
  has the block when the function returns.

Log and metrics share both helpers. Today `flush_log_buffer` and `flush_metrics_buffer` are
copies of each other. After this change, each is a few lines naming its stream, spare and sink
method. Image and thread streams don't change.

### 3. Pre-fault the queue buffer

Add `fn prefault(&mut self)` to `micromegas_transit::HeterogeneousQueue`, and generate it in
`declare_queue.rs`. It writes one byte per 4 KiB page into `buffer.spare_capacity_mut()`, using
`std::ptr::write_volatile` so the optimizer cannot remove stores it would otherwise treat as dead.
`len` stays 0. On systems with larger pages, this touches each page more than once, which is
harmless. Only `SpareBlock::refill` calls `prefault`. Thread blocks are per-thread and never
wait on a shared lock, so pre-faulting them would add cost and save nothing.

### 4. Hash merged properties before the lock

`merge_properties` returns a `MergedProperties<'a>` that holds the sorted `Vec<DynProperty<'a>>`
and its `content_hash`, computed at merge time. It implements `Deref<Target = [DynProperty<'a>]>`,
so sink calls (`on_log(desc, &merged, ...)`) and the interop filter compile unchanged.
`PropertyArena::get_or_insert` takes `&MergedProperties` and uses the stored hash. With that
signature, every caller has to pass a hashed set, so the compiler lists every call site.
Arena hits now do only an index lookup and a compare under the lock. A miss still allocates
under the lock (see Trade-offs).

### 5. Format and intern before the lock

In `log`, `log_tagged_to_sink` and `log_interop`, build the `DynString` message and look up the
interned target before taking the lock, as `log_dyn_tagged_to_sink` already does. In
`log_interop_dyn_tagged`, intern `desc.target` before the lock too.

### Behavior notes

- **Block order at the sink.** The sink is now called after the lock is released, so two
  racing flushes of the same stream can reach the sink in reverse order. For example, a
  push-path flush can race the flush monitor. Swaps are still serialized, so `event_offset` and
  `begin`/`end` stay contiguous and correct. The image stream already behaves this way.
- **Memory.** Each process keeps one extra resident block for logs and one for metrics, not per thread: 10 MiB for logs plus 1 MiB
  for metrics at the defaults. Today the next block's pages fault in gradually.
- **Fault cost.** The page faults move to the thread that triggered the flush, or to the flush
  monitor, and happen outside the lock. The total CPU cost stays the same. Waiting threads no
  longer pay it.

## Implementation Steps

### Phase 0: Baseline benchmark, before any change

1. In `rust/tracing/benches/properties.rs`, add `("log/formatted", log_case("formatted"))` to the
   `bench_contention_group` cases. Moving formatting out of the lock is one of the changes, and
   the contention group doesn't cover it yet. Commit this bench-only change on its own.
2. From `rust/`, with the bench change built and no tracing changes applied, save a baseline:
   `cargo bench -p micromegas-tracing --bench properties -- --save-baseline lock-before`.
   Run it on a quiet machine. Run it a second time and check that the two runs agree. If they
   don't, re-run before trusting the baseline.

### Phase 1: Transit

3. `rust/transit/src/heterogeneous_queue.rs`: add `fn prefault(&mut self);`.
4. `rust/transit/derive/src/declare_queue.rs`: generate `prefault`.

### Phase 2: Arena hash

5. `rust/tracing/src/property_arena.rs`: add `MergedProperties`, make `merge_properties` return it,
   and change `get_or_insert` to take `&MergedProperties` and use its hash.
6. Update the callers in `dispatch.rs`, and `rust/tracing/tests/property_arena_tests.rs`.

### Phase 3: Spare block and swap

7. Create `rust/tracing/src/event/spare_block.rs` with `SpareBlock`, and declare and
   re-export it in `event/mod.rs` under `#[cfg(not(target_arch = "wasm32"))]`, like `block` and
   `stream`.
8. `dispatch.rs`: add `log_spare` and `metrics_spare`, the `swap_block` helper, and
   `on_log_pushed` / `on_metrics_pushed`. Rewrite `flush_log_buffer` and `flush_metrics_buffer`
   on top of them, and switch all eleven push sites over: log paths `log`, `log_tagged_to_sink`, `log_interop`,
   `log_dyn_tagged_to_sink`, `log_interop_dyn_tagged`; metric paths `int_metric`, `float_metric`,
   `tagged_float_metric`, `tagged_integer_metric`, `tagged_float_metric_dyn`,
   `tagged_integer_metric_dyn`.
9. `dispatch.rs`: move message formatting and `intern_string` above the lock in `log`,
   `log_tagged_to_sink` and `log_interop`, and `intern_string` in `log_interop_dyn_tagged`.

### Phase 4: Measure after

10. Run `cargo bench -p micromegas-tracing --bench properties -- --baseline lock-before` on the
    same machine, under the same conditions as the baseline.
11. Record a before/after table in the Results section of this plan:
    - the `contention` group at 1/4/8 threads for every case
    - the single-thread `log` and `metric` groups, to check that the uncontended path didn't
      regress (the `MergedProperties` hash moves rather than adds work)

    Put the headline numbers in the CHANGELOG entry. If 4- and 8-thread contention is still
    significant, note that sharding the streams is the next step, as the issue says.

### Phase 5: Finish

12. Add the CHANGELOG entry. Run `cargo fmt`, then
    `cargo clippy --workspace -- -D warnings`, then `cargo test -p micromegas-tracing -p micromegas-transit`.

## Files to Modify

- `rust/tracing/benches/properties.rs`: add `log/formatted` to the contention group
- `rust/transit/src/heterogeneous_queue.rs`: `prefault` trait method
- `rust/transit/derive/src/declare_queue.rs`: generated `prefault`
- `rust/tracing/src/property_arena.rs`: `MergedProperties`, `get_or_insert` signature
- `rust/tracing/src/event/spare_block.rs` (new), `rust/tracing/src/event/mod.rs`
- `rust/tracing/src/dispatch.rs`: spares, swap helper, push and flush paths, formatting moved out of the lock
- `rust/tracing/tests/property_arena_tests.rs`: new signature
- `rust/tracing/tests/stream_flush_tests.rs` (new)
- `rust/telemetry-sink/src/http_event_sink.rs`: restate the `try_push` comments (`:94-99`, `:202`, `:234`)
- `CHANGELOG.md`

## Trade-offs

- **A recycled-buffer pool instead of a pre-faulted spare.** A pool would keep the `Arc`s of sent
  blocks and reclaim each one once the sink drops its reference. That avoids the page faults
  entirely, not just moves them out of the lock. However, it needs a retired list, a reset path
  for every block field and the arena, and a cap on retained memory. It also only pays off when
  the sink has finished with the previous block. The spare already gets the faults out of the
  lock, and the faults are what hurt the waiting threads. If the benchmark shows the
  fault CPU cost still matters, the pool can be added inside `SpareBlock::refill`, and the
  dispatch side wouldn't change.
- **Swap in the push path, not drop-then-flush.** This costs one `DualTime::now()` under the
  lock per full block. In exchange, the window where threads push past the full margin is gone,
  and so are the redundant tiny-block flushes. It is only possible because the spare makes the
  swap allocation-free.
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
- CPU (thread) streams get no spare block: the memory would scale with the number of recording
  threads (user call).
- Sharding the streams is out of scope. Whether to do it depends on the after numbers.

## Documentation

- `CHANGELOG.md`: a Tracing entry with the before/after numbers. Add a **Minor breaking change**
  clause covering `HeterogeneousQueue::prefault`, the `merge_properties` return type and the
  `PropertyArena::get_or_insert` signature.
- `rust/telemetry-sink/src/http_event_sink.rs`: the comments that say `try_push` must not emit
  telemetry because dispatch calls `on_process_*_block` under the stream mutex are no longer true.
  Restate the reason as recursion into the sink, or drop them.
- No mkdocs change. The capi threading-model note (`mkdocs/docs/native/index.md:171`, "per-stream
  `Mutex`") is still accurate.

## Testing Strategy

No-DB unit tests in a new `rust/tracing/tests/stream_flush_tests.rs`. They use
`init_in_memory_tracing()`, whose 1 KiB buffers force frequent flushes, and all are marked `#[serial]`.
The sink re-entrancy test skips `init_in_memory_tracing()` and uses its own `DispatchGuard` and
sink (see below):

- **Sink can log during block processing.** `init_in_memory_tracing()` can't be
  used, since it always installs `InMemorySink`; the test is still `#[serial]`. Build it on the `property_arena_tests.rs` pattern:
  a `CapturingSink` and a `DispatchGuard` that calls `init_event_dispatch` with 1024-byte buffers
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
  is above the full threshold (no tiny duplicate flushes), and that every block's
  `capacity_bytes()` is still 1024. This is the concurrency check that
  would otherwise fail silently.
- **Fallback when the spare is missing.** Construct a `SpareBlock` with `SpareBlock::new` (reachable through
  `pub mod event`), which leaves the slot empty, and call `take_or_new` on it. Assert that the
  returned block has the right `stream_id` and an empty queue.
- **`prefault` keeps the queue empty.** In transit's tests: after `prefault()`, `len_bytes() == 0`,
  `nb_objects() == 0`, `capacity_bytes()` is unchanged, and pushes and iteration still work.
- **Arena.** Update `property_arena_tests.rs`: equal merged sets still dedup, different sets
  don't, and `MergedProperties` hashes identical content identically however the static and
  dynamic properties were split.

The performance effect is measured by the before/after criterion run in Phases 0 and 4, not by
tests.

## Results

_Fill in during Phase 4: before/after criterion numbers for the `contention` group (1/4/8
threads) and the single-thread `log`/`metric` groups._

## Open Questions

- Is one extra resident log block and one extra metrics block (11 MiB at the defaults) acceptable for every embedder,
  including capi hosts? If not, the spare could be sized lazily or kept only for logs.
