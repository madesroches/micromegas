# Dynamic High-Cardinality Log and Metric Properties Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1623

## Overview

Let Rust-instrumented code attach properties with arbitrary, non-`'static` values (e.g.
`correlation_id`, `request_id`, `session_id`) to individual log entries and metric measures,
without leaking memory. Each log/metrics block gets a property arena that owns the dynamic
values. The arena counts against the block's memory budget and is dropped with the block. On
the wire the arena's contents go out as ordinary `PropertySetDependency` / `StaticStringDependency`
records, so ingestion, analytics parsing, the `log_entries` / `measures` schemas, and
`property_get(properties, 'correlation_id')` all stay unchanged. Spans stay out of scope: they
carry no properties today and keep it that way.

## Current State

- **Interned property sets.** `PropertySet::find_or_create`
  (`rust/tracing/src/property_set.rs:42-48`) interns every set through `internment::Intern`, and
  `Property::new` takes `&'static str` for both name and value. Interned sets are never freed,
  so a per-operation value leaks one set per distinct value.
- **Events reference sets by address.** `TaggedLogString`
  (`rust/tracing/src/logs/log_events.rs:155-210`) writes `properties` as a raw pointer (`u64`).
  So do `TaggedFloatMetricEvent` / `TaggedIntegerMetricEvent`
  (`rust/tracing/src/metrics/metric_events.rs:41-61`, POD via `TransitReflect`, where
  `&'static PropertySet` is reflected as a reference member).
- **Dependencies are rebuilt for every block.** `ExtractDeps::extract` for `LogMsgQueue`
  (`rust/tracing/src/logs/block.rs:91-120`) and `MetricsMsgQueue`
  (`rust/tracing/src/metrics/block.rs:85-110`) walks the block's events with a fresh
  `recorded_deps: HashSet<u64>`. It dereferences each set pointer and emits
  `StaticStringDependency` for every property name/value plus one `PropertySetDependency`
  (id = set address). `record_properties` is duplicated verbatim in both files.
  `encode_block` (`rust/telemetry-sink/src/stream_block.rs:25-56`) calls `extract()` on the
  sink thread while the `Arc<LogBlock>` is still alive.
- **The block budget.** `EventStream::is_full` (`rust/tracing/src/event/stream.rs:87-90`)
  compares `current_block.len_bytes()` (queue bytes only, `rust/tracing/src/event/block.rs:84`)
  with `capacity - hint_max_obj_size`. `flush_log_buffer` / `flush_metrics_buffer`
  (`rust/tracing/src/dispatch.rs:804-823`, `:696-716`) swap in a fresh block and hand the old
  one to the sink.
- **Dispatch entry points.** `log_tagged` (`rust/tracing/src/dispatch.rs:750-775`) calls
  `sink.on_log(desc, properties.get_properties(), …)` *before* taking the log-stream lock (so
  local sinks can log without deadlocking), then pushes a `TaggedLogString`.
  `tagged_float_metric` / `tagged_integer_metric` (`:647-694`) push the tagged metric events.
- **Analytics parsing.** `read_dependencies` (`rust/transit/src/parser.rs:25-133`) builds a
  per-block `HashMap<u64, Value>` from that block's dependency buffer only. `parse_property_set`
  (`rust/tracing/src/parsing.rs:229-275`) resolves each `Property` name/value through it.
  `parse_tagged_log_string` (`:155-185`) and the POD metric parse resolve `properties` the same
  way. `log_entry_from_value` (`rust/analytics/src/log_entry.rs:153-195`) and `measure.rs` turn
  the object into an analytics `PropertySet`. `LogEntriesRecordBuilder` /
  `MetricsRecordBuilder` and their `PropertySetJsonbDictionaryBuilder` (pointer-dedup on
  *parse-arena* addresses, not wire ids) are built once per block
  (`rust/analytics/src/lakehouse/log_block_processor.rs:30`, `metrics_block_processor.rs:30`).
- **Macros.** `log!(lvl, properties: set, …)` (`rust/tracing/src/macros.rs:266-279`, also
  reachable through `info!`/`warn!`/…), and `imetric!` / `fmetric!` with a 4-argument
  `(name, unit, properties, value)` arm (`:163-228`).

## Design

### Wire format: unchanged

The design rests on dependency ids only having to be unique *within one block*. The instrumented
process already emits every dependency a block references into that same block. So an arena set
can use its heap address as its `PropertySetDependency` id, and each owned value string can use
its heap address as its `StaticStringDependency` id. While the block is alive these addresses
can't collide with each other or with `'static` data. The receiving side sees exactly the
records it parses today. Results:

- no new transit types, no new custom readers, no `SCHEMA_VERSION` bump;
- analytics servers already deployed can read blocks from instrumented processes that use the
  new API;
- the Unreal sink could adopt the same scheme later without server changes.

What does change is the meaning of an id: *the same id in two different blocks no longer implies
the same content*, because a freed arena address can be reused by the next block. Any
receiving-side code that carries a wire id across blocks has to change (see
[Cross-block handle stability](#cross-block-handle-stability)).

### `PropertyArena` (new, `rust/tracing/src/property_arena.rs`)

```rust
/// Owns the dynamic property sets referenced by one block's events.
/// Addresses are stable until the arena is dropped (each set/value is boxed).
#[derive(Debug, Default)]
pub struct PropertyArena {
    sets: Vec<Box<PropertySet>>,        // id = address of the boxed set
    values: Vec<Box<str>>,              // owned dynamic values
    index: HashMap<u64, Vec<u32>>,      // content hash -> indices into `sets`
    len_bytes: usize,
}

impl PropertyArena {
    /// Returns the arena-owned set equal to `properties`, inserting a copy (with owned
    /// values) on first sight. A hit allocates nothing.
    pub fn get_or_insert(&mut self, properties: &[Property]) -> *const PropertySet;
    pub fn len_bytes(&self) -> usize;
}
```

- Lookup is by content: hash the names and value bytes, then compare candidates by content.
  Within a block, every log of one operation that shares a `correlation_id` then ships the value
  and the set once. Analytics' pointer-dedup JSONB builder also keeps working, because the parse
  arena has one object per wire set.
- On insert, names keep their `'static` `StaticStringRef`. Each value is copied into a
  `Box<str>`, and its `StaticStringRef` is built from the boxed bytes through a new
  `pub(crate) unsafe fn StaticStringRef::from_str_unchecked(&str)`. The caller guarantees the
  string outlives every use of the ref.
- `len_bytes` adds each new value's length, plus `size_of::<PropertySet>()` and
  `n * size_of::<Property>()` per new set. This approximates the dependency bytes the set adds to
  the encoded block, which is what the budget is meant to bound. Hashmap overhead is ignored.

### The arena lives on the block

`EventBlock<Q>` (`rust/tracing/src/event/block.rs`) gains `pub properties: PropertyArena`, and
`TracingBlock::len_bytes` returns `events.len_bytes() + properties.len_bytes()`. `is_full` then
triggers a flush once events plus arena reach the existing budget, so per-stream memory stays
around `logs_buffer_size` (or `metrics_buffer_size`) plus one oversized entry, whatever the
cardinality. The next block starts with an empty arena, and a value still in use is simply
re-emitted there.

Thread and image blocks carry an always-empty arena: no allocation until first insert, and no
API inserts into it. That is cheaper than splitting `EventBlock` into two types just to keep the
field off spans.

**Safety invariant** (documented on the field): events in `events` may hold addresses into
`properties`. Both live in the same `EventBlock`, which is only dropped as a whole. `extract()`
runs while the block is alive, and nothing removes or mutates arena entries once they are
inserted.

### Static property sets stay first-class

Interned `&'static PropertySet`s remain the right tool for bounded-cardinality properties, and
their API and cost don't change:

- `log!(lvl, properties: set, …)`, `log_tagged`, the 4-argument `imetric!` / `fmetric!` arms and
  `tagged_{float,integer}_metric` keep their `&'static PropertySet` signatures;
- they never touch the arena: no merge, no hashing, no copy. The event stores the interned
  set's address exactly as today, and the set is emitted once per block through
  `recorded_deps`.

The arena is used only when a call passes `dyn_properties`. When a call combines a static set
with dynamic properties, the merged arena set reuses the static entries' `StaticStringRef`s
(their strings are still emitted once per block as `'static` dependencies). Only the dynamic
values are copied.

### Event field types

`TaggedLogString.properties` and the two tagged metric events' `properties` change from
`&'static PropertySet` to `*const PropertySet`. This is internal plumbing so the same event type
can carry either an interned set or an arena set; the static paths above just store
`set as *const PropertySet`. With arena-backed sets, keeping the field `&'static` would be a lie.
The reflection derive already treats `syn::Type::Ptr` as a reference member
(`rust/transit/derive/src/derive_reflect.rs:17`), so the UDT stays a reference to the
`PropertySetDependency` id. Only the reflected member `type_name` string changes, and parsing
ignores it for reference members. The static-set paths cast `&'static PropertySet` to the
pointer. `record_properties` takes `*const PropertySet` and dereferences it under the block
invariant above. This also does away with the `transmute::<u64, &PropertySet>` in
`TaggedLogString::read_value`, leaving one small piece of
`tasks/backlog/pointer-provenance.md`.

DRY: move the duplicated `record_properties` into `property_set.rs`, generic over a deps queue
that can `push` `StaticStringDependency` and `PropertySetDependency`. Both `extract` impls call
it.

### Dispatch API (`rust/tracing/src/dispatch.rs`)

```rust
/// A property whose value is not 'static. Names stay 'static: name cardinality must remain bounded.
pub struct DynProperty<'a> { pub name: &'static str, pub value: &'a str }

pub fn log_dyn_tagged(desc: &'static LogMetadata, properties: &'static PropertySet,
                      dyn_properties: &[DynProperty<'_>], args: fmt::Arguments<'_>);
pub fn tagged_float_metric_dyn(desc: &'static StaticMetricMetadata, properties: &'static PropertySet,
                               dyn_properties: &[DynProperty<'_>], value: f64);
pub fn tagged_integer_metric_dyn(desc: &'static StaticMetricMetadata, properties: &'static PropertySet,
                                 dyn_properties: &[DynProperty<'_>], value: u64);
```

Shared step `merge_properties(static_set, dyn_properties) -> Vec<Property>`: start
from the static set's properties, apply each dynamic property (a dynamic value replaces a static
property of the same name, compared case-insensitively to match `property_get`), then sort the
same way `find_or_create` does. The dynamic entries' refs point at the caller's strings and are
only valid for the duration of the call.

`log_dyn_tagged` follows `log_tagged`: level check, `sink.on_log(desc, &merged, …)` *before*
locking (so local sinks keep their lock-free re-entrancy), then take the log-stream lock, call
`get_or_insert(&merged)` on the current block's arena, push a `TaggedLogString` with the
returned pointer, and do the usual `is_full` flush. The arena is reached through a new
`EventStream::get_block_mut_parts() -> (&mut Queue, &mut PropertyArena)`, which uses the same
`Arc::get_mut` as `get_events_mut`. The metric variants do the same under the metrics lock,
without `on_log`. `dispatch_wasm.rs` gets matching no-op/forwarding stubs.

`DynProperty` and the empty static `PropertySet` reference used by the dyn-only `log!` arm must
exist on both native and wasm32 (also defined in `dispatch_wasm.rs` and the wasm `property_set`
stub), so the prelude re-export and the macros compile on wasm32.

### Macros (`rust/tracing/src/macros.rs`)

- `log!(lvl, properties: set, dyn_properties: dyn, …)` and
  `log!(lvl, dyn_properties: dyn, …)` (static set = a `lazy_static` empty interned set), both
  calling `log_dyn_tagged`. They flow through `info!`/`warn!`/`error!`/`debug!`/`trace!`/`fatal!`
  through the existing generic arm.
- `imetric!` / `fmetric!` gain a 5-argument arm `(name, unit, properties, dyn_properties, value)`.

Usage:

```rust
let cid = request.correlation_id(); // String
info!(dyn_properties: &[DynProperty { name: "correlation_id", value: &cid }], "fetched {n} rows");
imetric!("rows_fetched", "count", &*STATIC_PROPS, &[DynProperty { name: "correlation_id", value: &cid }], n);
```

```sql
SELECT time, process_id, msg FROM log_entries
WHERE property_get(properties, 'correlation_id') = '…' ORDER BY time
```

### `tracing` crate interop (`rust/telemetry-sink/src/tracing_interop.rs`)

Today `FieldFormatVisitor` (`tracing_interop.rs:11-31`) folds every event field into the message
as `name=value`, and `on_event` calls `log_interop`, which pushes a
`LogStaticStrInteropEvent` / `LogStringInteropEvent` (`rust/tracing/src/dispatch.rs:777-802`).
Those events have no properties.

- **Fields become dynamic properties.** `tracing`'s `Field::name()` is `&'static str` (it comes
  from the callsite), so field names satisfy the static-name rule as they are. The visitor also
  collects each field other than `message` and those whose name starts with `log.` (the
  `log.target` / `log.module_path` / `log.file` / `log.line` fields added by `tracing-log` for
  bridged `log` records) as `(name, value)`:
  `record_str` keeps the `&str` as an owned copy, and the other `record_*` methods format with
  `Debug`, as they do for the message today. The message text stays exactly as it is
  (`name=value` included), so existing queries that match on `msg` keep working.
- **Wire event.** A Rust `TaggedLogInteropEvent` (`rust/tracing/src/logs/log_events.rs`) mirrors
  the existing Unreal event of the same name (`unreal/MicromegasTracing/Public/MicromegasTracing/LogEvents.h:26-75`):
  `time: i64, level: u8, target: StaticStringRef, properties: *const PropertySet, msg: DynString`.
  It is dynamically sized, its reflected name is `"TaggedLogInteropEvent"`, and its secondary UDT
  is `StaticStringRef`. Analytics already parses it (`parse_tagged_log_interop_event`,
  `rust/tracing/src/parsing.rs:115-153`; `log_entry.rs:124-151`), so this is still no
  receiving-side change. Add it to `LogMsgQueue`. In `extract`, emit its target dependency (as
  `LogStringInteropEvent` does) and call `record_properties`.
- **Dispatch.** `log_interop_dyn_tagged(desc: &LogMetadata, dyn_properties: &[DynProperty<'_>],
  args)` follows `log_interop`. It merges against the empty static set, calls
  `on_log(desc, &merged, …)`, then takes the lock and pushes a `TaggedLogInteropEvent` using
  `arena.get_or_insert(&merged)` and the interned target.
- **No fields, no cost.** `on_event` calls `log_interop_dyn_tagged` only when at least one
  property was collected. Field-less events keep the current `log_interop` path, which doesn't
  touch the arena.
- Span fields aren't propagated, since scope inheritance is out of scope. Only the event's own
  fields count.

### Cross-block handle stability

Code on the receiving side could assume that a wire dependency id ("handle") means the same
thing in every block of a process. Arena addresses break that assumption. An audit of the
receiving code found nothing that relies on it:

- **Per-block.** These are built fresh for every block: the dependency map
  (`read_dependencies`, `rust/transit/src/parser.rs:31`), the bump arena (`parse_block`,
  `rust/analytics/src/payload.rs:75-86`), `PropertySetJsonbDictionaryBuilder` (keyed on
  *parse-arena* object addresses, and only used by the per-block `LogEntriesRecordBuilder` /
  `MetricsRecordBuilder`), and the producers' `recorded_deps` / Unreal `Ids` sets.
- **Content-keyed.** Code that does merge blocks keys on content, never on wire ids:
  the call tree (`compute_scope_hash`), `NetSpanTreeBuilder`, and the perfetto intern maps.
- **Not exposed.** No lakehouse table stores a dependency id. The `parse_block()` UDTF does
  expose `id` members of POD dependency records (e.g. `desc.id`), but those stay `'static`
  metadata. Property-set ids are dropped by the custom reader (`parser.rs:101-107`), and string
  dependencies become plain strings, so no arena address ever reaches SQL output.

So the analytics side needs **no code change**. The new contract (ids are scoped to one block)
is pinned by the cross-block test below. On the producer side, the audit found two
requirements, and the block-owned arena meets both:

- Arena entries must live until the block is *encoded*, not merely closed. The arena is owned by
  the `Arc`'d block that the sink encodes.
- No address may be reused within one block, otherwise `recorded_deps` would emit only the first
  content behind it. The arena never frees an entry before the whole block is dropped.

## Implementation Steps

1. **Arena.** Add `rust/tracing/src/property_arena.rs` (`PropertyArena`), export it from
   `lib.rs`, and add `StaticStringRef::from_str_unchecked`.
2. **Block.** Add the `properties` field to `EventBlock`, initialize it in `TracingBlock::new`,
   fold it into `len_bytes`, and document the invariant. Add `EventStream::get_block_mut_parts`.
3. **Events.** Switch `TaggedLogString`, `TaggedFloatMetricEvent` and `TaggedIntegerMetricEvent`
   `properties` to `*const PropertySet`, and update the static-set call sites in `dispatch.rs`
   plus the tests that build or read these events
   (`rust/analytics/tests/log_tests.rs`, `parse_corrupt_block_tests.rs`,
   `rust/object-cache/tests/foyer_backend_tests.rs`, `rust/object-cache-srv/tests/telemetry_tests.rs`).
   The object-cache tests dereference the pointer while the sink still holds the block.
4. **Deps.** Move `record_properties` into `property_set.rs` (generic, `*const PropertySet`) and
   call it from both `extract` impls.
5. **Dispatch.** Add `DynProperty`, `merge_properties`, `log_dyn_tagged`,
   `tagged_{float,integer}_metric_dyn` (plus wasm stubs), and re-export `DynProperty` from the
   prelude. `DynProperty` and the empty-set reference are available on wasm32 too.
6. **Macros.** Add the `dyn_properties:` log arms and the 5-argument metric arms, with rustdoc
   examples.
7. **`tracing` interop.** Add the Rust `TaggedLogInteropEvent` (plus its `LogMsgQueue` entry
   and `extract` arm), the new match arm in `rust/tracing/tests/utils.rs` (it matches
   `LogMsgQueueAny` exhaustively), and `log_interop_dyn_tagged` (plus a wasm stub). Then extend
   `FieldFormatVisitor` to collect the fields and route `on_event` to the new dispatch when
   there are any.
8. **Docs and changelog** (below). No analytics code changes (see
   [Cross-block handle stability](#cross-block-handle-stability)).

## Files to Modify

- `rust/tracing/src/property_arena.rs` (new)
- `rust/tracing/src/lib.rs`, `rust/tracing/src/static_string_ref.rs`,
  `rust/tracing/src/property_set.rs`
- `rust/tracing/src/event/block.rs`, `rust/tracing/src/event/stream.rs`
- `rust/tracing/src/logs/log_events.rs`, `rust/tracing/src/logs/block.rs`
- `rust/tracing/src/metrics/metric_events.rs`, `rust/tracing/src/metrics/block.rs`
- `rust/tracing/src/dispatch.rs`, `rust/tracing/src/dispatch_wasm.rs`,
  `rust/tracing/src/macros.rs`
- `rust/telemetry-sink/src/tracing_interop.rs`
- Tests constructing tagged events: `rust/analytics/tests/log_tests.rs`,
  `rust/analytics/tests/parse_corrupt_block_tests.rs`,
  `rust/object-cache/tests/foyer_backend_tests.rs`, `rust/object-cache-srv/tests/telemetry_tests.rs`
- `rust/tracing/tests/utils.rs` (new `LogMsgQueueAny` match arm)
- `CHANGELOG.md`, `rust/tracing/README.md`

## Trade-offs

- **Arena with dependency-format output (chosen) vs. inlining dynamic properties in each event**
  (a new `TaggedLogStringV2` carrying `(name, DynString)` pairs). Inlining needs no arena and no
  pointer reasoning. But it needs a new event type, new custom readers, and analytics plus
  Unreal-parser changes, it breaks reading by servers that are already deployed, it repeats the
  value in every event of an operation, and it defeats the per-block JSONB pointer-dedup (one
  parse object per row). The arena keeps the wire format and dedups within the block, which is
  the approach suggested in the issue thread.
- **Content-keyed dedup vs. none.** Without dedup, every dynamic log would add a set and its
  values to the block. Dedup costs one hash of the merged property list per call, which is small
  next to the `args.to_string()` that `log_tagged` already pays.
- **Field on `EventBlock` vs. separate log/metrics block types.** Separate types would keep the
  arena off thread/image blocks, but they'd duplicate the `TracingBlock` impl and the
  `encode_block` plumbing for an empty `Vec`/`HashMap` per block. Span dispatch never touches
  the field.
- **`'static` names.** Names stay `&'static str`. Dynamic names would turn the JSONB key space
  into a cardinality problem on the query side, and the issue only asks for dynamic values.

## Decisions

- Logs and metrics only. Spans get no properties (user call).
- Static interned property sets stay supported as a zero-overhead option with unchanged
  signatures; the arena is only used for calls that pass `dyn_properties` (user call).
- Scope-inherited properties (from the issue's "or to a scope") are out of scope: properties
  are attached per call only (user call).
- `tracing` interop ships in the same PR, with event fields mapped to dynamic properties (user
  call). The message keeps its `name=value` text so `msg`-matching queries don't change.
- Handles in analytics: an audit found no code that carries a wire id across blocks, so there
  is no analytics change. The `desc.id` shown by `parse_block()` stays `'static` and is out of
  scope.

## Documentation

- `CHANGELOG.md` (Unreleased): new dynamic-properties API, plus a **Minor breaking change**
  clause: the `properties` field of `TaggedLogString`, `TaggedFloatMetricEvent` and
  `TaggedIntegerMetricEvent` is now `*const PropertySet`, `EventBlock` has a new
  `properties` field, and `LogMsgQueueAny` has a new `TaggedLogInteropEvent` variant.
- `rust/tracing/README.md` (the crate's docs.rs landing page): a short "Dynamic properties"
  section with the log and metric examples above and the cardinality note (names static, values
  free).
- Rustdoc on the new macros arms, `DynProperty` and `PropertyArena`.
- The `log_entries` / `measures` schema docs (`mkdocs/docs/query-guide/schema-reference.md`)
  need no change, since the columns are unchanged.

## Testing Strategy

All of these are no-DB unit tests under `rust/tracing/tests/` and `rust/analytics/tests/`:

- **Arena** (`tracing/tests/property_arena_tests.rs`): the same content returns the same
  pointer, and `len_bytes` doesn't grow on a hit; different values return different sets; the
  stored values are copies (mutate or drop the source `String` after insert, then read the set);
  `len_bytes` grows by the expected amount; `merge_properties` lets dynamic override static and
  matches names case-insensitively.
- **Static path untouched**: after `log_tagged` / `tagged_*_metric` calls with an interned set,
  the block's arena is still empty (`len_bytes() == 0`), and the encoded event references the
  interned set's address.
- **Budget**: push dynamic-tagged logs with unique large values into a `LogStream` of small
  capacity and assert `is_full()` trips on events-plus-arena bytes before the queue alone would.
  Do the same for a `MetricsStream`.
- **Dispatch**: a test-local `EventSink` records the `on_log` properties (the in-memory sink
  ignores them); install it with `init_event_dispatch` under `#[serial]`, like
  `InMemoryTracingGuard`. Assert `log_dyn_tagged` delivers the merged properties to `on_log`,
  and that after a flush the stream's new current block has an empty arena while the flushed
  block keeps its sets.
- **Round trip** (extending `rust/analytics/tests/log_tests.rs` and the metrics equivalent):
  build a block with dynamic-tagged logs and metrics, encode it with `encode_block`, parse it
  through `parse_block` / `log_entry_from_value` / the measure path, and assert
  `properties` contains `correlation_id` with the right value next to the static properties.
  Also assert that two events with the same dynamic set in one block share one
  `PropertySetDependency`.
- **Cross-block** (`rust/analytics/tests/`): two blocks of the same stream whose property-set
  and value ids are *identical* but whose contents differ (built by hand-encoding the dependency
  buffers, so address reuse is forced rather than left to chance) parse to their own values.
  This pins the new contract that ids are scoped to one block, so a future cross-block cache
  keyed on wire ids fails here instead of silently mislabeling rows.
- **`tracing` interop**:
  - Visitor (`rust/telemetry-sink/tests/tracing_interop_tests.rs`, new): run a `tracing` event
    under `tracing::subscriber::with_default` with a test layer that drives
    `FieldFormatVisitor`. Assert that `correlation_id = %id` and `n = 3` become properties, that
    `message` and a `log.*` field (e.g. `log.module_path`) do not, and that the message text is unchanged.
  - Round trip: a block holding a Rust `TaggedLogInteropEvent` parses through
    `log_entry_from_value` with its properties and target. This pins the Rust and Unreal layouts
    to the one analytics parser.
- Existing `parse_corrupt_block_tests.rs` and the property-set tests keep passing against the
  changed field types.
