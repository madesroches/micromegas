# Properties on Thread and Async Spans, Recorded at Span End Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1691

## Overview

Let a native Rust span carry properties, as logs, metrics and `otel_spans` already do. The
properties are recorded on the span's **end** event, so a span can report its results (row
counts, bytes, cache hit/miss, error status) as well as context the caller attaches. Today the
only way to put context on a span is a runtime name (`span_scope_named_dyn!`,
`span_async_named_dyn!`). That puts high-cardinality values in the name and breaks name-based
aggregation. Properties keep names low-cardinality. Begin events and plain end events don't
change. New tagged end events reuse the per-block dynamic dependency queue (`DynDeps`) and
`merge_properties`. `thread_spans`, `async_events` and `process_spans` each gain a `properties`
column, appended last.

## Current State

**Events** (`rust/tracing/src/spans/span_events.rs`): there are six end events, plain and named
for thread and async spans: `EndThreadSpanEvent` (`:45`), `EndThreadNamedSpanEvent` (`:61`),
`EndThreadDynNamedSpanEvent` (`:130`), `EndAsyncSpanEvent` (`:84`), `EndAsyncNamedSpanEvent`
(`:106`) and `EndAsyncDynNamedSpanEvent` (`:151`). None of them carries properties. All of them
derive `TransitReflect`.

**Block** (`rust/tracing/src/spans/block.rs`): `ThreadEventQueue` (`:18`) holds the twelve
begin/end types. `ThreadDepsQueue` (`:35`) holds `SpanRecord`, `SpanLocationRecord`,
`Utf8StaticStringDependency` and `StaticStringDependency`. It implements `StringDepsQueue` but not
`PropertyDepsQueue`, because thread blocks only hold dynamic strings (span names) in their
`DynDeps`. `ThreadEventQueue::extract` (`:129`) never calls `DynDeps::record_static_refs`.
`ThreadStream::push_dyn_named` (`:226`) is the only way into a thread block's dynamic queue.

**Dispatch** (`rust/tracing/src/dispatch.rs`): `on_end_scope` (`:350`),
`on_end_named_scope[_dyn]` (`:367`, `:455`) and `on_end_async_[named_]scope[_dyn]` (`:389`,
`:424`, `:489`) push into the thread-local `LOCAL_THREAD_STREAM` through `on_thread_event`
(`:549`) or `on_thread_dyn_named_event` (`:565`). They take no lock. The metric dyn paths
(`tagged_float_metric_dyn`, `:852`) show the tagged pattern: an interned set goes in as
`PropertySetId::interned` with no hashing, and otherwise `merge_properties` is followed by
`dyn_deps.intern_set`.

**Guards and futures:**

- `ThreadSpanGuard` and `ThreadNamedSpanGuard<N: SpanName>` (`rust/tracing/src/guards.rs:98`,
  `:120`) end the span in `Drop`.
- `InstrumentedFuture` and `InstrumentedNamedFuture<F, N>`
  (`rust/tracing/src/spans/instrumented_future.rs:147`, `:221`) end it when `poll` returns
  `Ready`.
- `SpanName` (`rust/tracing/src/spans/span_name.rs:15`) is sealed, with `StaticSpanName` and
  `DynSpanName<N>` behind it.
- The span macros (`rust/tracing/src/macros.rs:31-232`) bind the guard to a hygienic
  `guard_named`, so callers can't name it.

**Property macros:** `__props!` (`macros.rs:674`) parses `key = value` lists into a per-callsite
literal set, `static` values and dynamic values, with `log` and `metric` kinds. `__prop_key!`
(`:1021`) rejects the keys that each kind reserves.

**Analytics:**

- `parse_thread_block_payload` (`rust/analytics/src/thread_block_processor.rs:50`) dispatches on
  `type_name`. It warns on unknown types and ignores async events.
  `ThreadBlockProcessor::on_end_thread_scope` (`:20`) has no properties parameter.
- `CallTreeBuilder` (`rust/analytics/src/call_tree.rs:40`) builds a tree over a whole chain of
  blocks (`thread_spans_view.rs:188`). So it can't keep a `PropertySet<'a>` that borrows one
  block's parse arena.
- `SpanRecordBuilder` / `get_spans_schema` (`rust/analytics/src/span_table.rs`) feed
  `thread_spans`, and `process_spans` (`lakehouse/process_spans_table_function.rs:59`) reuses
  the same schema. Its async half is a SQL join over `async_events` (`:385`) with an explicit
  column list.
- `parse_async_block_payload` (`rust/analytics/src/async_block_processing.rs:59`) feeds
  `AsyncEventCollector` (`lakehouse/async_events_block_processor.rs:34`). A new collector is
  built for every block, so `PropertySetJsonbDictionaryBuilder`'s one-arena invariant holds
  there. `async_events_table_schema` is in `rust/analytics/src/async_events_table.rs`.
- `SCHEMA_VERSION` is 3 in both `thread_spans_view.rs:38` and `async_events_view.rs:36`.
- `log_entry.rs:261` shows how to read a `properties` reference member:
  `obj.get::<&Object>("properties")` returns an `&Object`, which `.into()` converts to an
  analytics `PropertySet` (`From<&Object>`, `properties/property_set.rs:50`).

## Design

### Events (`span_events.rs`)

There are four new tagged end events. Each existing end event keeps its layout and cost.

```rust
#[derive(Debug, TransitReflect)]
pub struct TaggedEndThreadSpanEvent {
    pub thread_span_desc: &'static SpanMetadata,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub time: i64,
}

#[derive(Debug, TransitReflect)]
pub struct TaggedEndThreadNamedSpanEvent {
    pub thread_span_location: &'static SpanLocation,
    pub name: DynStringId,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub time: i64,
}

// TaggedEndAsyncSpanEvent / TaggedEndAsyncNamedSpanEvent: the same, plus
// span_id, parent_span_id, depth, as in EndAsync[Dyn]NamedSpanEvent.
```

The named variants always carry the name as a block-scoped string record (`DynStringId`), whether
it was `'static` or built at runtime. Analytics matches begin and end by the content hash of the
scope description, not by the event type or string id, so a static-named begin closes correctly
with this end.

### Block (`spans/block.rs`)

- Add the four types to `ThreadEventQueue`, and add `PropertySetDependency` to
  `ThreadDepsQueue` with a `PropertyDepsQueue` impl, as `LogDepsQueue` does.
- `extract` handles the new arms like their plain counterparts. It also calls
  `record_properties` for interned sets, caching the last set as the log extractor does
  (`logs/block.rs:80`). Before `extend_from`, it calls `dyn_deps.record_static_refs(...)`. Today
  this does nothing, because thread blocks have no dynamic sets.
- Add `ThreadStream::push_tagged<T>(set, dyn_properties, make: impl FnOnce(PropertySetId) -> T)`
  and `push_tagged_named<T>(name, set, dyn_properties, make: impl FnOnce(DynStringId,
  PropertySetId) -> T)`. With no dynamic properties, the set goes in as
  `PropertySetId::interned`. Otherwise it is `merge_properties` then `intern_set`. These mirror
  `push_dyn_named`, so the `get_block_mut_parts` borrow stays in one place.

### Dispatch (`dispatch.rs`, `dispatch_wasm.rs`)

```rust
pub fn on_end_scope_tagged(scope: &'static SpanMetadata,
                           properties: &'static PropertySet, dyn_properties: &[DynProperty<'_>]);
pub fn on_end_named_scope_tagged(loc: &'static SpanLocation, name: &str,
                                 properties: &'static PropertySet, dyn_properties: &[DynProperty<'_>]);
pub fn on_end_async_scope_tagged(span_id: u64, parent_span_id: u64, scope: &'static SpanMetadata,
                                 depth: u32, properties: &'static PropertySet,
                                 dyn_properties: &[DynProperty<'_>]);
pub fn on_end_async_named_scope_tagged(span_id: u64, parent_span_id: u64, loc: &'static SpanLocation,
                                       name: &str, depth: u32, properties: &'static PropertySet,
                                       dyn_properties: &[DynProperty<'_>]);
```

- If both the set and `dyn_properties` are empty, the unnamed tagged functions fall back to
  `on_end_scope` / `on_end_async_scope`, and the two named ones fall back to
  `on_end_named_scope_dyn` / `on_end_async_named_scope_dyn`, because they take a runtime
  `&str`. Guards and futures check for emptiness first and end through `SpanName::end_*`, so
  static-named spans keep their exact bytes.
- Otherwise the function reads `now()` first, as the existing `_dyn` end paths do. It then goes
  through a new `on_thread_tagged_event` helper, which has the same shape as
  `on_thread_dyn_named_event`. No lock is taken, because the thread stream is thread-local.
- `dispatch_wasm.rs` gets no-op stubs.

### Properties held by the guard

```rust
let mut span = span_scope!(name = "fetch_rows", region = "eu", request = %req_id);
let rows = fetch()?;                       // an early return still records region and request
span.set_property("rows", rows.len());
span.set_property("cache", if hit { "hit" } else { "miss" });
// the end event at drop carries region, request, rows and cache
```

**`SpanProperties`** (new, `rust/tracing/src/spans/span_properties.rs`):

```rust
/// Properties a span carries until its end event. Values are copied in, so they only need to
/// outlive the call that sets them.
pub struct SpanProperties {
    static_set: &'static PropertySet,             // literal and `static` values from the macro
    entries: SmallVec<[(&'static str, u32, u32); 8]>, // name, offset and len into `values`
    values: SmallVec<[u8; 128]>,
}

impl SpanProperties {
    pub fn new(static_set: &'static PropertySet) -> Self;
    pub fn set_property(&mut self, name: &'static str, value: impl fmt::Display);
}
```

- `set_property` formats the value straight into `values` with `fmt::Write`, so short values
  (`&str`, numbers, anything `Display`) cost no heap allocation. Longer ones spill once.
- Setting a name that is already present (case-insensitive, as in `merge_properties`) replaces
  the value. The old bytes are compacted out, so a property set inside a loop doesn't grow the
  buffer.
- At drop, the guard builds a `DynProperty` slice of the entries and passes it, with
  `static_set`, to the matching `on_end_*_tagged`. The merge happens only in `push_tagged`. A
  value set at run time beats a literal of the same name, the rule `log!` already uses. With no properties at all, the guard emits the
  plain end.
- Names are `&'static str`, because name cardinality must stay bounded.
- `smallvec` and `rustc-hash` are native-only dependencies, and `merge_properties` lives in the
  native-only `dyn_deps`, while `guards` and `instrumented_future` compile on wasm32. Following
  the stub convention in `lib.rs`, wasm32 gets a cfg'd `SpanProperties` stub holding only the
  `static_set`, with a no-op `set_property`. The guard's tagged drop path is cfg'd to the plain
  end there, so the wasm clippy build (`-D warnings`) stays clean.

**Guards.** `SpanName` gains `fn as_str(&self) -> &str`, so that both name kinds reach the single
`_named_scope_tagged` dispatch.

- `ThreadSpanGuard` and `ThreadNamedSpanGuard` gain a properties type parameter `P`. It is bounded
  by a sealed trait with two impls:
  - `()`, the default, which ends the span plainly. Every existing guard keeps its size and its
    drop path.
  - `SpanProperties`, which ends the span through the tagged path.
- `set_property` exists only when `P = SpanProperties`, takes `&mut self`, and forwards to the storage.
- Both guard types become `#[must_use]`.

**Macros.** `span_scope!`, `span_scope_named!` and `span_scope_named_dyn!` each gain a `name =`
arm, placed before the existing arms. The arm is an expression that returns the guard:
`span_scope!(name = <name>, k = v, ...)`.

- The name follows the macro's existing rule: a literal for `span_scope!`, a `&'static str` for
  `span_scope_named!`, and an owned or borrowed runtime string for `span_scope_named_dyn!`.
- The property list is the one `log!` takes: literals, `static` values, `%`/`?`/bare dynamic
  values and a trailing `..set`. It is parsed by a new `span` kind in `__props!`, with an end arm
  that accepts an empty rest and a `span` arm for a trailing `.. $set:expr $(,)?` with nothing
  after it (the existing `..set` arms need a comma and more tokens).
  - Literals and `static` values become the guard's `static_set` through the callsite set and
    static set cache. A `..set` combined with literals goes through `__static_set!` with the
    spread set as the base, producing one `&'static` `static_set`; `log!`'s non-`static` spread
    path, which sends the literals as dynamic entries, does not apply because the guard holds a
    single `static_set`. A `..set` alone is used as the `static_set` directly.
  - Each dynamic entry becomes a `set_property` call on the new guard. `%v` goes in as
    `Display` with no `format!`. `?v` goes in through a small `Debug`-as-`Display` adapter. A
    bare value goes through `PropertyValue`.
- `__prop_key!(span name)` / `(span "name")` is a compile error, because `name` is the span's
  name. `span_scope!(name = "x", name = "y")` therefore fails to compile.
- Writing `span_scope!(name = "x", ...);` as a bare statement drops the guard at once. That
  triggers the `#[must_use]` warning. CI clippy runs without `--all-targets`, so the warning is
  denied in library code only; the compile-fail doctest's `#![deny(unused_must_use)]` covers the
  rest.
- The existing arms don't change. They stay statements with a hidden guard and no properties.
- Dynamic values are evaluated without a check. Spans have no level gate, and span names built at
  runtime are evaluated the same way.

**Async spans.** `InstrumentFuture` gains
`instrument_with(self, desc, on_end: impl FnOnce(&Self::Output, &mut SpanProperties))`. The four
async macros (`instrument_named!`, `instrument_named_dyn!`, `span_async_named!`,
`span_async_named_dyn!`) each gain an optional trailing closure argument:

```rust
let rows = instrument_named!(fetch(id), "fetch", |out, span| {
    if let Ok(rows) = out {
        span.set_property("rows", rows.len());
    }
})
.await;
```

- `InstrumentedFuture` and `InstrumentedNamedFuture` gain an end-hook type parameter. Its default
  ends the span plainly, so `instrument` and the existing macro arms keep their types and their
  cost.
- On `Ready`, the future builds an empty `SpanProperties` on the stack and passes it to the hook
  with the output. It then emits the tagged end, or the plain end when the hook set nothing.
- The hook is a separate type parameter, so a non-capturing closure keeps the future `Send`.

### Analytics

**Parsing:**

- `ThreadBlockProcessor::on_end_thread_scope` and `AsyncBlockProcessor::on_end_async_scope` gain
  a `properties: PropertySet<'_>` parameter. Plain ends pass `PropertySet::empty()`.
- `parse_thread_block_payload` maps `TaggedEndThreadSpanEvent` to the `EndThreadSpanEvent` arm
  and `TaggedEndThreadNamedSpanEvent` to the named arm. It reads `properties` with
  `obj.get::<&Object>("properties")`. The two tagged async types join the ignore list there.
- `parse_async_block_payload` does the same for the async types.

**Content-keyed JSONB dictionary** (new, `rust/analytics/src/properties/jsonb_dictionary.rs`).
`thread_spans` builds one tree over several blocks, so the pointer-keyed
`PropertySetJsonbDictionaryBuilder` can't be used there (see its invariant). Instead:

```rust
/// JSONB property sets deduplicated by content. Index 0 is the empty set.
pub struct JsonbDictionary { values: Vec<Vec<u8>>, index: HashMap<Vec<u8>, i32> }
impl JsonbDictionary {
    pub fn new() -> Self;                         // seeds index 0 with the empty set's JSONB
    pub fn intern(&mut self, jsonb: &[u8]) -> i32;
    pub fn values(&self) -> &[Vec<u8>];
    pub fn finish(self, keys: Vec<i32>) -> Result<DictionaryArray<Int32Type>>;
}
```

- `CallTreeNode` gains `properties: i32`, which defaults to 0. `CallTree` gains
  `property_sets: JsonbDictionary`.
- When a span ends, `CallTreeBuilder::on_end_thread_scope` serializes a non-empty set once with
  `serialize_property_set_to_jsonb`, then interns it. That covers all three close paths: a
  matching top, a span adopted from the root, and a span that began before the range.
- A span that never ends, or that is closed at the edge of the range, keeps 0.
- Cost scales with tagged ends only. A plain span pays for one `i32`.
- `SpanRecordBuilder` owns a `JsonbDictionary` and a `Vec<i32>` of keys.
  - `append_call_tree` remaps the tree's dictionary into the builder's once per tree (one
    `intern` for each distinct set), then pushes one remapped key per node.
  - `append(SpanRow, properties: &[u8])` takes the properties as a parameter; `SpanRow` stays
    owned and gains no field. Both `append` and `append_call_tree` go through a private helper
    that appends the 11 existing columns. `append` then interns the bytes and pushes the key;
    `append_call_tree` pushes its remapped key directly. Tests use `append`.
  - `finish` builds the column.
- `async_events` keeps `PropertySetJsonbDictionaryBuilder`, since its collector handles one block
  at a time. `AsyncEventRecord` gains `properties: PropertySet<'a>`, and begin rows get the
  empty set.

**SQL surface** (additive; each column is appended last):

| Table | New last column | Type |
|---|---|---|
| `thread_spans` (`get_spans_schema`) | `properties` | `Dictionary(Int32, Binary)`, non-null, JSONB |
| `async_events` | `properties` | same; begin rows hold the empty set |
| `process_spans` | `properties` (through `get_spans_schema`) | same; the async join selects `e.properties` last |

The column is non-null and an empty set means "no properties", as in `log_entries`. Bump
`SCHEMA_VERSION` to 4 in `thread_spans_view.rs` and `async_events_view.rs`.

## Implementation Steps

**Phase 1: thread spans**

1. `span_events.rs`: add `TaggedEndThreadSpanEvent` and `TaggedEndThreadNamedSpanEvent`, each with
   `impl InProcSerialize`.
2. `spans/block.rs`: add the queue entries, `PropertySetDependency` in `ThreadDepsQueue` with
   its `PropertyDepsQueue` impl, the new `extract` arms with `record_properties` and
   `record_static_refs`, and `push_tagged` / `push_tagged_named`. Switch `recorded_deps` and the
   three `record_*_dependencies` helper signatures from `std::collections::HashSet` to
   `rustc_hash::FxHashSet`, as `logs/block.rs` and `metrics/block.rs` do.
3. `dispatch.rs`: add `on_thread_tagged_event`, `on_end_scope_tagged` and
   `on_end_named_scope_tagged`. `dispatch_wasm.rs`: add stubs.
4. `spans/span_properties.rs` (new, plus its `mod` / `pub use` in `spans/mod.rs`):
   `SpanProperties`, with the wasm32 stub (checked by the wasm clippy build). `span_name.rs`: add `SpanName::as_str`. `guards.rs`: add the properties type
   parameter with its sealed trait, `set_property`, the tagged drop path and `#[must_use]`.
5. `macros.rs`: add the `name =` arms on the three scope macros, the `span` kind in `__props!`
   and `__prop_key!` (with `name` reserved), and a `__span_guard_call!` that builds the guard.
6. `rust/tracing/tests/utils.rs`: add the new `ThreadEventQueueAny` match arms.
7. Analytics: add `JsonbDictionary` (`properties/jsonb_dictionary.rs`, plus its `mod`
   declaration), the trait parameter on `ThreadBlockProcessor`, the parser arms, `CallTreeNode`
   / `CallTree` / `CallTreeBuilder`, `SpanRow` / `SpanRecordBuilder` / `get_spans_schema`, and
   the `SCHEMA_VERSION` bump in `thread_spans_view.rs`. Update the test impls of
   `ThreadBlockProcessor` (`tests/span_tests.rs`), the direct `on_end_thread_scope` calls in
   `tests/call_tree_tests.rs`, and the `SpanRow` users
   (`tests/dictionary_key_overflow_tests.rs`).
8. `process_spans_table_function.rs`: append `arrow_cast(jsonb_parse('{}'), 'Dictionary(Int32, Binary)')
   as properties` (the empty set) to the async join's select list, because `async_events` has no
   `properties` column until Phase 2 and the batch is built strictly against `get_spans_schema`.
   The thread half passes the view's batches through unchanged.
9. Bench: add `dispatch/span_props_literal` and `dispatch/span_props_dyn` cases to
   `rust/tracing/benches/with_dispatch.rs`, next to `dispatch/span_scope`.

**Phase 2: async spans** (can ship as its own PR)

10. Add `TaggedEndAsyncSpanEvent` and `TaggedEndAsyncNamedSpanEvent` (each with
    `impl InProcSerialize`) to the events, the queue
    and `extract`; add the two new `ThreadEventQueueAny` arms in `rust/tracing/tests/utils.rs`. Add `on_end_async_scope_tagged` and `on_end_async_named_scope_tagged`, with
    wasm stubs.
11. `instrumented_future.rs`: add the end-hook type parameter,
    `InstrumentFuture::instrument_with`, and the closure arms on the four async macros.
12. Analytics: add the `AsyncBlockProcessor` parameter, the parser arms (and add the two tagged
    async types to `parse_thread_block_payload`'s ignore list), `AsyncEventRecord` /
    `AsyncEventRecordBuilder` / `async_events_table_schema`, and the `SCHEMA_VERSION` bump in
    `async_events_view.rs`. In `process_spans_table_function.rs`, replace the empty-set literal with
    `e.properties`. Update the test impls (`tests/span_tests.rs`,
    `tests/async_events_tests.rs`, `tests/dictionary_key_overflow_tests.rs`). Bump the
    `num_columns() == 12` asserts in `tests/async_events_tests.rs` to 13.

**Phase 3: docs and changelog** (see Documentation)

## Files to Modify

- `rust/tracing/src/spans/span_events.rs`, `spans/block.rs`, `spans/span_name.rs`,
  `spans/instrumented_future.rs`
- `rust/tracing/src/dispatch.rs`, `dispatch_wasm.rs`, `guards.rs`, `macros.rs`, `lib.rs`
- `rust/tracing/tests/utils.rs`, new `rust/tracing/tests/span_properties_tests.rs`
- `rust/tracing/benches/with_dispatch.rs`
- `rust/analytics/src/thread_block_processor.rs`, `async_block_processing.rs`, `call_tree.rs`,
  `span_table.rs`, `async_events_table.rs`, new `properties/jsonb_dictionary.rs` (+ `mod`)
- `rust/analytics/src/lakehouse/thread_spans_view.rs`, `async_events_view.rs`,
  `async_events_block_processor.rs`, `process_spans_table_function.rs`
- `rust/analytics/tests/span_tests.rs`, `call_tree_tests.rs`, `async_events_tests.rs`,
  `dictionary_key_overflow_tests.rs`
- `CHANGELOG.md`, `mkdocs/docs/query-guide/schema-reference.md`,
  `mkdocs/docs/query-guide/functions-reference.md`,
  `mkdocs/docs/when-to-use/instrumentation-overhead.md`
- `claude-plugin/skills/micromegas-query/SKILL.md`, `rust/tracing/README.md`,
  `doc/how_to_query/README.md`

## Trade-offs

- **Tagged end events vs. a separate "span properties" event pushed before the plain end.** A
  separate event would need no new end types, but analytics would have to pair it with the next
  end event across block boundaries. A crash between the two events would also leave a dangling
  record. A tagged end is one atomic event.
- **Four tagged types (named ends always use a block string) vs. six (a static-name variant
  too).** The static-name variant would save one string hash per tagged end, which is small next
  to the property merge. Two fewer types means less code in the producer, `extract` and both
  parsers.
- **A `name =` arm that returns the guard vs. turning the existing `span_scope*!` arms into
  expressions.** If `span_scope!("x")` returned the guard, every existing `span_scope!("x");`
  statement would drop it at once and silently record a zero-length span. A new arm keeps
  existing call sites correct, and `#[must_use]` catches the same mistake in the new form.
- **A setter on a live guard vs. a consuming `span_end!(guard, ..)`.** The setter needs storage
  on the guard until drop. The inline buffer keeps that free of heap allocation for short values,
  and the default `P = ()` keeps it off plain guards. In return, every exit path records what
  was set so far, including `?` and early returns. With a consuming end, those paths lose every
  property. Dynamic set ids are block-scoped and a block can flush mid-span, so the values can't
  be written to the block as they are set. They have to wait in the guard.
- **A content-keyed dictionary for `thread_spans` vs. Arrow's `BinaryDictionaryBuilder`.**
  Arrow's builder would hash the JSONB bytes of every row, including the empty set on every plain
  span in the largest table. The index scheme hashes once per distinct set per tree.

## Decisions

- A span with properties is created with `let mut span = span_scope!(name = .., k = v, ..)`, which
  returns the guard. `name` is a reserved property key. A `let` form inside the macro
  (`span_scope!(let span = ..)`) was rejected (user call).
- Results are added with `span.set_property(name, value)` on the live guard. There is no
  `span_end!` (user call). The name follows OTel's `set_attribute` and Rust's `set_` convention
  (user call). Properties given at creation and set later are combined in the guard,
  with the last value of a name winning, and are written once on the end event. The issue's "no
  merge" goal holds on the wire and in analytics.
- The property syntax is the codebase's `key = value` (`log!`, `imetric!`), not the issue's
  `"rows" => n` sketch, so that every property-taking macro parses the same way.
- C API, Unreal bindings, properties on begin events, and `#[span_fn]` integration are out of
  scope for v1, following the issue.
- Perfetto `args` mapping is not part of this plan. `perfetto_trace_execution_plan.rs` selects
  explicit columns, so it is unaffected.

## Compatibility

- **Deploy analytics before instrumented code that records span properties.** An older analytics server
  treats a tagged thread end as an unknown type and skips it. The next end then pops the wrong
  span, and the call tree fails with `top scope mismatch`. On the async side, an older server
  shows the span as never ended. Put this in the CHANGELOG entry.
- Spans without properties emit exactly the bytes they emit today.
- **Minor breaking change (Rust API only):**
  - `ThreadBlockProcessor::on_end_thread_scope` and `AsyncBlockProcessor::on_end_async_scope`
    take `properties`.
  - `CallTreeNode`, `CallTree` and `AsyncEventRecord` gain fields; `SpanRecordBuilder::append`
    gains a `properties` parameter.
  - `ThreadEventQueueAny` and `ThreadDepsQueueAny` gain variants.
  - `SpanName` gains `as_str`. It is sealed, so only in-crate impls are affected.
  - `ThreadSpanGuard` and `ThreadNamedSpanGuard` gain a defaulted type parameter and become
    `#[must_use]`.
  - `InstrumentedFuture` and `InstrumentedNamedFuture` gain a defaulted type parameter.

## Documentation

- `CHANGELOG.md` (Unreleased): the feature, the deploy-order note, the new last column on
  `thread_spans`, `async_events` and `process_spans`, and the **Minor breaking change** clause
  above.
- `rust/tracing/src/lib.rs` crate docs: a "Span properties" section next to the span section
  (`:100`). It shows `let mut span = span_scope!(name = ..)` with `set_property` and the async closure form,
  and calls out the gotchas:
  - `let _ = span_scope!(name = ..)` drops the guard at once, like any `let _`. Bind a name.
  - A span that never ends, or that is cut at the edge of a query range, has no properties.
- Rustdoc on the `name =` arms, `SpanProperties::set_property`, the guards and
  `instrument_with`, with a passing doctest for each form. The compile-fail doctests are listed
  under Testing Strategy.
- `mkdocs/docs/query-guide/schema-reference.md`: add a `properties` row to the `thread_spans` and
  `async_events` tables, plus an example
  (`SELECT name, property_get(properties, 'rows') ... FROM view_instance('thread_spans', ..)`).
- `mkdocs/docs/query-guide/schema-reference.md` (`:662`): the `properties` description ("log
  properties, metric properties") also covers spans.
- `claude-plugin/skills/micromegas-query/SKILL.md` (`:329-364`): add a `properties` row to the
  `thread_spans` and `async_events` column tables.
- `rust/tracing/README.md` (`:25`): properties attach to logs, metrics and spans.
- `doc/how_to_query/README.md` (`:323+`, `:339+`): add `properties` to the `async_events` and
  `thread_spans` tables of this legacy schema copy.
- `mkdocs/docs/query-guide/functions-reference.md`: `process_spans` returns `properties`.
- `mkdocs/docs/when-to-use/instrumentation-overhead.md` (`:5`, `:165`): replace "Micromegas
  spans carry no attributes" with a note that spans can carry end-time properties, which this
  comparison doesn't measure.

## Testing Strategy

All of these are no-DB unit tests.

- **Producer** (`rust/tracing/tests/span_properties_tests.rs`, `#[serial]`, under
  `InMemoryTracingGuard`):
  - `span_scope!(name = "x", k = "lit")` with literals only emits a `TaggedEndThreadSpanEvent`
    whose `properties` resolves through `as_interned`, and the block's `dyn_deps` stays empty.
  - With a `%` value and a `set_property` call, the set resolves through
    `dyn_deps.set_properties` to the merged pairs. A runtime value overrides a literal of the
    same name, and a `key = value` pair overrides a `..set` entry.
  - `span_scope!(name = "x")` with nothing set emits the plain `EndThreadSpanEvent`, and the
    existing `span_scope!("x")` form is unchanged.
  - A function that returns early through `?` after a `set_property` still emits that property.
  - The begin event type is unchanged.
  - The `span_scope_named!` and `span_scope_named_dyn!` forms emit
    `TaggedEndThreadNamedSpanEvent`, with the name in `dyn_deps.str`.
  - Two spans with the same dynamic set in one block share one set record.
  - `SpanProperties`: setting a name twice keeps the last value and doesn't grow the buffer, a
    case-insensitive repeat replaces, and a value longer than the inline capacity round-trips.
  - Compile-fail doctests: `name` repeated as a property key and a
    bare-statement `span_scope!(name = "x");` under `#![deny(unused_must_use)]`.
  - Phase 2: a future built with `instrument_named!(.., |out, span| ..)` passes its output to the
    closure and emits a tagged async end with the matching `span_id`. A closure that sets nothing
    yields the plain end, and a future without a closure is unchanged.
- **Encode/parse round trip** (`rust/analytics/tests/span_tests.rs`):
  - Build a `ThreadStream` block with begin plus tagged ends (an interned set, a dynamic set with
    a static name inside, and a named span), then encode it with `StreamBlock::encode_bin`.
  - Run `parse_thread_block_payload` and assert that the processor receives the expected pairs,
    and empty sets for plain ends.
  - This pins `extract`'s output order (static refs before the dynamic queue), which the parser
    requires.
  - Do the same for async in Phase 2.
- **Call tree / `thread_spans` rows** (`rust/analytics/tests/call_tree_tests.rs`):
  - Feed `CallTreeBuilder` directly. A span ended with properties gets the right JSONB in the
    `SpanRecordBuilder` column, and plain spans get the empty set.
  - A span that begins in one block and ends with properties in the next keeps them.
  - A span whose begin is before the range (the root-adoption path) keeps its end properties.
  - A span left open at the end of the range gets the empty set.
  - Two trees appended to one builder that share a set produce one dictionary value.
  - The batch's last field is `properties`.
- **`JsonbDictionary`**: index 0 is the empty set, equal bytes intern to the same index, and
  `finish` round-trips the keys.
- **`async_events` rows** (`rust/analytics/tests/async_events_tests.rs`): begin rows hold the
  empty set, end rows hold the event's set, and the schema's last field is `properties`.
- Existing span, call tree, `thread_spans` ordering and dictionary overflow tests keep passing
  with the new parameter and fields.

## Manual Verification

These need running services. One measures partition size, which can't be asserted. The other
exercises `process_spans`' SQL against materialized views, and a mismatch there fails loudly on
first use.

1. Start the services: `python3 local_test_env/ai_scripts/start_services.py --monolith`. Run an
   instrumented example that records many spans and gives a fraction of them properties.
2. **Partition size.** On the same source data, run
   `micromegas-query "SELECT view_set_name, sum(file_size) FROM list_partitions() WHERE view_set_name IN ('thread_spans','async_events') GROUP BY 1"`
   once on the pre-change build, after querying the stream's `thread_spans` and the process's
   `async_events` so that the JIT partitions exist. Then run it again on the new build, which
   rebuilds them because of the `SCHEMA_VERSION` bump. Expected: for plain spans the growth is
   negligible, since the column is one repeated dictionary key. Record the numbers in the PR.
3. **`process_spans`.** Run
   `micromegas-query "SELECT thread_name, name, property_get(properties, 'rows') FROM process_spans('<process_id>', 'both') WHERE properties_length(properties) > 0"`.
   Expected: thread rows and async rows (`thread_name = 'async'`) both show their end
   properties.

## Open Questions

None.
