# Dynamic Span Names Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1677 (follow-up to #1654,
related: #1676)

## Overview

Named spans only accept a `&'static str` name. To use a name built at runtime, you pass it
through `intern_string`, and the process keeps it forever. With high-cardinality names, that
memory has no bound. This plan gives thread blocks the per-block dynamic dependency queue
(`DynDeps`) that logs and metrics already have. A dynamic name is written to the block's queue
as a string record with a block-scoped id, and that record is freed along with the block.
Dynamic names get their own four event types, so the `&'static str` events and their paths don't
change. The SQL surface doesn't change.

## Current State

**Events** (`rust/tracing/src/spans/span_events.rs:51-115`): the four named events
(`BeginThreadNamedSpanEvent`, `EndThreadNamedSpanEvent`, `BeginAsyncNamedSpanEvent`,
`EndAsyncNamedSpanEvent`) store `name: StringId`. All of them derive `TransitReflect` and are
serialized by copying the raw struct.

**`StringId`** (`rust/tracing/src/string_id.rs`): `{ pub ptr: *const u8, pub len: u32 }`, with
a hand-written `Reflect` (the derive doesn't support reference UDTs) exposing one reference
member `id`. The parser resolves any `is_reference` member by looking its 8-byte key up in the
dependency map (`transit/src/parser.rs:174`); it doesn't care which UDT carries the key.

**Dispatch** (`rust/tracing/src/dispatch.rs:356-445`): `on_begin_named_scope`,
`on_end_named_scope`, `on_begin_async_named_scope` and `on_end_async_named_scope` take
`name: &'static str` and call `on_thread_event` (`:482`). That function pushes to the
thread-local `LOCAL_THREAD_STREAM` without taking a lock and flushes when `is_full()`. There
are wasm no-op stubs in `dispatch_wasm.rs:213-255`.

**Callers:**
- `ThreadNamedSpanGuard` (`guards.rs:122-143`) stores `name: &'static str`.
- `InstrumentedNamedFuture` (`spans/instrumented_future.rs:210-291`) stores
  `name: &'static str`. Its begin and end can run on different threads, which means different
  thread streams.
- The macros `span_scope_named!`, `span_async_named!` and `instrument_named!`
  (`macros.rs:58-148`). The `span_scope_named!` doc says "The span name still needs to be
  statically allocated".
- `rust/tracing/examples/named_async_spans.rs` (Example 5) uses `intern_string` for a dynamic
  name in a loop.

**Thread block** (`rust/tracing/src/spans/block.rs`):
- `ThreadDepsQueue` holds `<SpanRecord, SpanLocationRecord, Utf8StaticStringDependency>`.
- `ThreadEventQueue::extract` ignores its `_dyn_deps` argument.
- `record_named_scope_event_dependencies` pushes `Utf8StaticStringDependency::from(name)`,
  which dereferences the pointer.
- `EventBlock<ThreadEventQueue>` already carries a `dyn_deps: DynDeps<ThreadDepsQueue>` that
  stays empty. `TracingBlock::len_bytes` already counts it, so `is_full` accounts for dynamic
  records without any change.

**`DynDeps`** (`rust/tracing/src/dyn_deps.rs`):
- `intern_str` (`:275`) deduplicates by xxh3 content hash and pushes a `StaticStringDependency`
  with id `DYN_ID_TAG | offset << 1`.
- It sits in `impl<D: PropertyDepsQueue>`. `PropertyDepsQueue` (`property_set.rs:176`)
  requires both `push_string` and `push_set`, and the thread queue has no
  `PropertySetDependency`.

**Analytics**: `thread_block_processor.rs:79-101` and `async_block_processing.rs:104-139` match
events by type name and read `name` as a resolved `&str`; unknown types are skipped
(`thread_block_processor` logs `unknown event type`). `BorrowedScopeDesc` hashes by content. The
parser handles `StaticStringDependency` records by UDT name (`transit/src/parser.rs:79`), so it
already knows how to read them in any stream.

**Test sink**: `tracing/tests/utils.rs:105-116` matches `ThreadEventQueueAny` exhaustively.

## Design

### Dynamic named events

A reference UDT for a block-scoped id, next to `StringId` in `string_id.rs`, with a hand-written
`Reflect` like `StringId`'s (name `DynStringId`, size 8, reference member `id`):

```rust
#[derive(Debug)]
pub struct DynStringId {
    pub id: u64,
}
```

Four new events in `span_events.rs`, each a copy of its static counterpart with
`name: DynStringId`: `BeginThreadDynNamedSpanEvent`, `EndThreadDynNamedSpanEvent`,
`BeginAsyncDynNamedSpanEvent` and `EndAsyncDynNamedSpanEvent`. Append them last to
`ThreadEventQueue`'s type list. The existing events, `StringId`, and the log interop path are
untouched, so a dynamic id can never reach code that dereferences `StringId::ptr`.

### Queue trait split

Move `push_string` out of `PropertyDepsQueue` into a supertrait in `dyn_deps.rs`:

```rust
pub trait StringDepsQueue: HeterogeneousQueue {
    fn push_string(&mut self, dep: StaticStringDependency);
}
pub trait PropertyDepsQueue: StringDepsQueue {
    fn push_set(&mut self, dep: PropertySetDependency);
}
```

`intern_str` moves to `impl<D: StringDepsQueue> DynDeps<D>`. `intern_set` and
`record_static_refs` stay on `PropertyDepsQueue`. The log and metric queues split their impls
and their behavior doesn't change.

### Thread deps queue

- Append `StaticStringDependency` last to `ThreadDepsQueue`, giving
  `<SpanRecord, SpanLocationRecord, Utf8StaticStringDependency, StaticStringDependency>`. This
  is the record type that `intern_str` writes and that `string_at` decodes.
- Implement `StringDepsQueue` for `ThreadDepsQueue`.

### Extraction

`ThreadEventQueue::extract(&self, dyn_deps)`:

1. Walk the events as today. Split the location half of `record_named_scope_event_dependencies`
   into `record_span_location_dependencies`; the four dynamic arms call only that, since their
   name lives in `dyn_deps`.
2. `deps.extend_from(dyn_deps.queue())`. String records reference nothing, so unlike the
   property case there are no `'static` refs to emit first.

### Dispatch

A helper on the thread stream, in `spans/block.rs`, shared by the four dynamic paths and usable
by tests:

```rust
impl ThreadStream {
    pub fn push_dyn_named<T>(&mut self, name: &str, make: impl FnOnce(DynStringId) -> T)
    where T: InProcSerialize + ThreadEventQueueTypeIndex
    {
        let (events, dyn_deps) = self.get_block_mut_parts();
        let id = dyn_deps.intern_str(name);
        events.push(make(DynStringId { id }));
    }
}
```

In `dispatch.rs`, an `on_thread_dyn_named_event(name, make)` mirrors `on_thread_event`: it
does nothing when the thread has no stream, otherwise it calls `push_dyn_named` and flushes
when `is_full()`. The new public functions take `&str`:

- `on_begin_named_scope_dyn(loc: &'static SpanLocation, name: &str)`
- `on_end_named_scope_dyn(loc, name: &str)`
- `on_begin_async_named_scope_dyn(loc, name: &str, parent_span_id, depth) -> u64`
- `on_end_async_named_scope_dyn(span_id, parent_span_id, loc, name: &str, depth)`

The begin functions read `now()` inside `make`, after the intern. The end functions read it
before calling the helper. This keeps the interning cost outside the measured span. Add matching
no-op stubs to `dispatch_wasm.rs`.

**Block boundaries.** Ids are scoped to a block, and an async span can begin and end on
different thread streams. So the end path always interns the name again in the block that is
current at that point; it never reuses the id from begin. A new block starts with an empty
`DynDeps`, so the record is written again there. When begin and end fall in the same block, the
end path pays one hash probe and a content compare, and allocates nothing.

### Analytics

Add each dynamic event name as an alternative in the matching static arm, e.g.
`"BeginThreadNamedSpanEvent" | "BeginThreadDynNamedSpanEvent"`, in `thread_block_processor.rs`
(including the ignored async arm) and `async_block_processing.rs`. The members have the same
names, so the arm bodies don't change.

### Guards, futures and macros

So that the guard and future code (in particular the 40-line `poll`) isn't duplicated, add a
sealed name trait in `spans/` with two implementations:

```rust
pub trait SpanName: sealed::Sealed {
    fn begin_thread(&self, loc: &'static SpanLocation);
    fn end_thread(&self, loc: &'static SpanLocation);
    fn begin_async(&self, loc: &'static SpanLocation, parent: u64, depth: u32) -> u64;
    fn end_async(&self, span_id: u64, parent: u64, loc: &'static SpanLocation, depth: u32);
}
impl SpanName for &'static str { /* static dispatch fns */ }
pub struct DynSpanName<N>(pub N);
impl<N: AsRef<str>> SpanName for DynSpanName<N> { /* *_dyn dispatch fns */ }
```

- `ThreadNamedSpanGuard<N: SpanName = &'static str>` and
  `InstrumentedNamedFuture<F, N: SpanName = &'static str>` store `name: N`. The default type
  parameter keeps existing type mentions compiling. The static path monomorphizes to the same
  calls it makes today.
- `InstrumentFuture` gains a hidden `__instrument_named_dyn_internal(self, loc, name: N)`,
  which returns `InstrumentedNamedFuture<Self, DynSpanName<N>>`.
- New macros with the same arms as their static counterparts:
  - `span_scope_named_dyn!(name)`. `name` is any `AsRef<str>`, such as `&s`, `format!(..)` or
    `Arc<str>`. The guard owns it, so a temporary like `format!(..)` lives for the whole scope.
  - `span_async_named_dyn!(name, async_block)` and `instrument_named_dyn!(future, name)`. The
    future owns the name. A `String` or `Arc<str>` keeps the future `'static`.
- Export the macros from the prelude (`lib.rs:238`).

## Implementation Steps

1. **Events**: `DynStringId` (`string_id.rs`) and the four `*DynNamedSpanEvent` types
   (`span_events.rs`).
2. **Trait split**: add `StringDepsQueue` to `dyn_deps.rs`, make `PropertyDepsQueue` extend it
   (`property_set.rs`), move `intern_str` to the `StringDepsQueue` impl block, and split the
   impls in `logs/block.rs` and `metrics/block.rs`.
3. **Thread block** (`spans/block.rs`): append the four events to `ThreadEventQueue` and
   `StaticStringDependency` to `ThreadDepsQueue`, implement `StringDepsQueue`, add the location-only
   arms to the walk, append the dynamic queue in `extract`, and add `ThreadStream::push_dyn_named`.
   Add the four arms to `tests/utils.rs`.
4. **Dispatch**: add `on_thread_dyn_named_event` and the four `*_dyn` functions to
   `dispatch.rs`, plus wasm stubs in `dispatch_wasm.rs`.
5. **Analytics**: the match alternatives in `thread_block_processor.rs` and
   `async_block_processing.rs`.
6. **Callers**: add `SpanName`/`DynSpanName` (new `spans/span_name.rs`, re-exported from
   `spans/mod.rs`), make `ThreadNamedSpanGuard` (`guards.rs`) and `InstrumentedNamedFuture`
   (`instrumented_future.rs`) generic, and add the hidden `InstrumentFuture` method.
7. **Macros** (`macros.rs`): add `span_scope_named_dyn!`, `span_async_named_dyn!` and
   `instrument_named_dyn!`. Change the `span_scope_named!` doc to point at the dynamic variant
   instead of "needs to be statically allocated". Add them to the prelude.
8. **Example**: change Example 5 of `examples/named_async_spans.rs` to `span_async_named_dyn!`
   with a `format!` name, and drop the `intern_string` import.
9. **Bench + changelog**: add `dispatch/span_scope_named`, `dispatch/span_scope_named_dyn`
   (same name every call) and `dispatch/span_scope_named_dyn_high_card` (a name from a rotating
   set of 10k precomputed strings) to `benches/with_dispatch.rs`. Add `dispatch/span_scope_named`
   in its own commit before the functional change, so it can be run against the pre-change code
   as the baseline (or cherry-picked onto main). Run all three tracing benches on that baseline
   and on the branch. Add a `CHANGELOG.md` entry with the numbers and a **Minor breaking
   change** clause: `PropertyDepsQueue::push_string` moves to `StringDepsQueue`,
   `ThreadEventQueueAny` gains four variants, and the guard and future types gain a type
   parameter.

## Files to Modify

- `rust/tracing/src/string_id.rs`, `rust/tracing/src/dyn_deps.rs`,
  `rust/tracing/src/property_set.rs`, `rust/tracing/src/spans/span_events.rs`
- `rust/tracing/src/spans/block.rs`, `rust/tracing/src/spans/mod.rs`,
  `rust/tracing/src/spans/span_name.rs` (new), `rust/tracing/src/spans/instrumented_future.rs`
- `rust/tracing/src/logs/block.rs`, `rust/tracing/src/metrics/block.rs` (trait split only)
- `rust/tracing/src/dispatch.rs`, `rust/tracing/src/dispatch_wasm.rs`,
  `rust/tracing/src/guards.rs`, `rust/tracing/src/macros.rs`, `rust/tracing/src/lib.rs`
- `rust/tracing/examples/named_async_spans.rs`, `rust/tracing/benches/with_dispatch.rs`
- `rust/tracing/tests/utils.rs`, `rust/tracing/tests/dynamic_span_names_tests.rs` (new)
- `rust/analytics/src/thread_block_processor.rs`, `rust/analytics/src/async_block_processing.rs`,
  `rust/analytics/tests/span_tests.rs`
- `mkdocs/docs/when-to-use/index.md`
- `CHANGELOG.md`

## Documentation

- Doc comments on the three new macros and the four `*_dyn` dispatch functions.
- The `span_scope_named!` doc comment (`macros.rs:42-56`).
- `examples/named_async_spans.rs`, which shows the dynamic macro in place of `intern_string`.
- The `CHANGELOG.md` entry.
- `mkdocs/docs/when-to-use/index.md` ("Code vs. data" paragraph): say a Rust span name can be a
  statically allocated string or built at runtime with the `_dyn` macros. Also update the FAQ answer
  to "How do I find which asset, script or query made my code slow", which says a Rust span name is
  "a statically allocated string", to say the same.

## Testing Strategy

All of these are no-DB unit tests.

**`rust/tracing/tests/dynamic_span_names_tests.rs`** (new), using `init_in_memory_tracing` and
`init_thread_stream`:
- Two `span_scope_named_dyn!` scopes with the same runtime name in one block produce four
  `*ThreadDynNamedSpanEvent`s with the same id and one string record (`dyn_deps.len_bytes()` equals one
  record's size).
- Distinct names produce distinct ids.
- A static `span_scope_named!` emits the static event types and leaves `dyn_deps` empty.
- Begin, then `flush_thread_buffer()`, then end: the second block's `dyn_deps` holds a record
  for the name, and its end event's id points at it.
- `instrument_named_dyn!` with an owned `String`, polled to completion: the begin and end async
  events are the `*AsyncDynNamedSpanEvent` types and carry ids that resolve to records in their block.

**Round trip through the real parser** (`rust/analytics/tests/span_tests.rs`, which already
encodes a `ThreadBlock` and calls `parse_block`): add a test that builds a block with
`ThreadStream::push_dyn_named` for all four dynamic event types. It mixes in static named events,
including a static name with the same text as a dynamic one, and a `span_scope!` event.
Encode, parse through `parse_thread_block_payload` and the async block parser, and assert:
- each event's resolved `name`,
- no `duplicate dependency id`,
- no `dependency not found`.

**Bench**: the step 8 runs. Acceptance:
- No regression beyond noise on any case of the `with_dispatch` (including
  `dispatch/span_scope_named`), `properties` and `with_no_dispatch` benches.
- Measured, reported numbers for the two `_dyn` cases.

## Decisions

- The end path always interns again in the current block. It never carries the begin id.
- Dynamic names use separate `*DynNamedSpanEvent` types rather than a flag in `StringId`. Older
  analytics skip them as unknown events; accepted, since the next release is already
  forward-incompatible.
- Declined: an `is_dyn` flag in `StringId`'s padding. It kept the event types but made every
  `StringId` consumer branch at runtime, and a dynamic id reaching a `ptr` dereference was only
  prevented by convention.
- The SQL surface doesn't change.
- Declined for now: caching the begin id with a block generation; can be added behind
  `push_dyn_named` if the bench shows the end probe matters.
