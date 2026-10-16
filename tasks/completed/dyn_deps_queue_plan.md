# Per-Block Dynamic Dependency Queue Plan

**GitHub Issues**: https://github.com/madesroches/micromegas/issues/1654,
https://github.com/madesroches/micromegas/issues/1665

## Overview

Events that carry runtime data (dynamic property values today) store that data in a per-block
`PropertyArena` of boxed copies whose addresses act as dependency ids. At send time,
`extract` walks every event again and serializes the same data. This plan replaces the arena
with a **dynamic dependency queue**: a per-block queue of dependency records already in their
wire form, with block-scoped ids. Calls that carry dynamic data write their records when the
event is pushed. `extract` runs its usual walk for static dependencies, then appends the
queue's bytes. Events that refer only to static data do no extra work on the calling thread.

The queue deduplicates dynamic strings by content within a block, not only whole property
sets. This plan applies it to log and metric properties. That covers #1654 and folds in #1665, because
static values inside a dynamic set keep their pointer ids. The wire format and SQL surface
don't change.

## Current State

**Arena** (`rust/tracing/src/property_arena.rs`):

- `PropertyArena` (`:15`) holds `Vec<Box<PropertySet>>`, `Vec<Box<str>>`, an
  `FxHashMap<u64, Vec<u32>>` content index and an approximate `len_bytes`.
- `get_or_insert` (`:62`) hashes the merged content with chained xxh3. A hit compares content
  and allocates nothing. A miss allocates a `Box<PropertySet>`, its `Vec<Property>` and one
  `Box<str>` per value, including values that came from the interned static set (#1665). Those
  copies get new addresses, so they're never deduplicated against the interned originals.
- `merge_properties` (`:29`) combines the callsite's interned static set with the dynamic
  values into one sorted `Vec<DynProperty>`. Once merged, there's no record of which values
  were `'static`.
- The arena lives in `EventBlock::properties` (`rust/tracing/src/event/block.rs:18`), and
  `EventBlock::len_bytes` (`:89`) adds the arena's approximation to the events' bytes.
  `EventStream::get_block_mut_parts` (`rust/tracing/src/event/stream.rs:125`) hands out the
  queue and the arena together.

**Dispatch** (`rust/tracing/src/dispatch.rs`): the four dynamic paths call `get_or_insert`
under the stream lock: `tagged_float_metric_dyn` (`:775`), `tagged_integer_metric_dyn`
(`:800`), `log_dyn_tagged_to_sink` (`:968`) and `log_interop_dyn_tagged` (`:1002`). Each
stores the returned `*const PropertySet` in the event.

**Events:**

- `TaggedLogString` and `TaggedLogInteropEvent` (`rust/tracing/src/logs/log_events.rs`) have a
  manual `InProcSerialize` that writes `properties` as a `u64`.
- `TaggedFloatMetricEvent` and `TaggedIntegerMetricEvent`
  (`rust/tracing/src/metrics/metric_events.rs:43`, `:54`) derive `TransitReflect`. The derive
  marks `*const` members as references (`rust/transit/derive/src/derive_reflect.rs:17`), which
  makes the analytics parser resolve `properties` through the dependency map.

**Extraction:**

- `LogMsgQueue::extract` (`rust/tracing/src/logs/block.rs:70`) and `MetricsMsgQueue::extract`
  (`rust/tracing/src/metrics/block.rs:68`) walk every event. For each tagged event they call
  `unsafe record_properties` (`rust/tracing/src/property_set.rs:144`), which dereferences the
  set pointer (interned or arena-owned) and emits its name and value strings followed by a
  `PropertySetDependency`, deduplicated by address.
- `PropertySetDependency` (`property_set.rs:123`) holds a raw `*const PropertySet`. Its
  validity contract covers arena sets.
- `StaticStringDependency` (`rust/transit/src/static_string.rs`) derives its id from `ptr` in
  `write_value`, so the id is always an address.
- The only production caller is `encode_block` (`rust/telemetry-sink/src/stream_block.rs:35`),
  which runs on the sink side, outside the stream lock.

**Parser constraints** (`rust/transit/src/parser.rs`):

- `read_dependencies` (`:25`) builds a `HashMap<u64, Value>` in stream order and bails on a
  duplicate id. Ids are opaque `u64` keys scoped to one block.
- `parse_property_set` (`rust/tracing/src/parsing.rs:216`) resolves each property's name and
  value through `parse_pod_instance`. That function looks the reference up in the map built so
  far and bails with `dependency not found` if it's missing (`parser.rs:185`). **So a record
  must come after every record it references.**

**In-process readers** that dereference `properties`:
`rust/tracing/tests/stream_flush_tests.rs:112`,
`rust/tracing/tests/property_arena_tests.rs:67` (`set_as_pairs`), and `rust/tracing/tests/property_macro_tests.rs`, where the `props.map(|p| unsafe { &*p })` deref
at `:175` (metric sets can be arena-owned) migrates to `dyn_deps.set_properties` and the
`as usize` casts at `:180`/`:207` become `PropertySetId` equality. It also reads `block.properties.len_bytes()`
at `:184`, `:203` and `:208` (migrate to `dyn_deps.len_bytes()`). `rust/analytics/tests/log_tests.rs:363`,
`:407-420` (`test_tagged_log_interop_event`) and `rust/analytics/tests/metrics_test.rs:238` build
blocks through `merge_properties`, `get_block_mut_parts` and the arena's `get_or_insert`.

More sites also stop compiling, each with its own migration:

- `rust/object-cache/tests/foyer_backend_tests.rs:36,:57` dereference `e.properties` on
  interned sets: use `as_interned()`.
- `rust/object-cache-srv/tests/telemetry_tests.rs:71` may read an arena-owned set: use
  `set_properties` on the block's `dyn_deps`.
- `rust/telemetry-sink/tests/tracing_interop_tests.rs:36` reads interop events, which always
  take the dynamic path: use `set_properties`.
- `rust/analytics/tests/parse_corrupt_block_tests.rs:110`,
  `rust/analytics/tests/metrics_test.rs:103,:154,:164` and
  `rust/analytics/tests/log_tests.rs:250` build tagged events from
  `PropertySet::find_or_create(..)` raw pointers: wrap with `PropertySetId::interned`.

## Design

### Reference direction and output order

A dynamic record can point at static data: a dynamic set references `'static` names, and
`'static` values from the callsite set. Static data can never point at a dynamic record, since
block-scoped ids don't exist when static data is created. So the dependency stream is always
**static records first, dynamic queue appended**:

```
deps = [ static walk over events + 'static refs of dynamic sets, deduplicated ] ++ [ dynamic queue bytes ]
```

The dynamic queue contains only records with dynamic ids, and the static walk never emits a
dynamic id. So nothing is emitted twice, and the two parts don't share a dedup set. Within the
dynamic queue, a value string is always pushed before the set that references it.

### Block-scoped ids

```rust
pub const DYN_ID_TAG: u64 = 1 << 63;
fn dyn_string_id(offset: u32) -> u64 { DYN_ID_TAG | (u64::from(offset) << 1) }
fn dyn_set_id(offset: u32) -> u64 { DYN_ID_TAG | (u64::from(offset) << 1) | 1 }
fn dyn_offset(id: u64) -> u32 { ((id & !DYN_ID_TAG) >> 1) as u32 }
```

`offset` is the byte offset of the record in the dynamic queue, which gives in-process readers
O(1) decoding. Ids stay `u64` on the wire, so neither the analytics parser nor the Unreal
instrumentation changes.

Nothing in-process tells dynamic from static data by testing a pointer's high bits (see Decisions):

- **Sets** are told apart by bit 0. An interned set id is a `&'static PropertySet` address,
  which is at least 8-byte aligned on every platform (pointer tagging only touches the top
  byte), so bit 0 is clear. A dynamic set id always has bit 0 set. `property_set.rs` gets a
  `const` assert that `align_of::<PropertySet>() >= 2`.
- **Values** inside a set record are told apart by the `static_values` mask that `DynDeps`
  keeps next to each set offset (see below), never by inspecting the id. String pointers have
  no alignment to rely on.

`DYN_ID_TAG` only keeps dynamic ids out of the range of pointer ids on the wire, so the parser
never sees a duplicate. A dynamic id is `0x80` in the top byte with an address below `2^33`,
which no supported platform's allocator or loader produces. Heap pointer tagging only changes the
top byte to a different value (Android 11+ puts `0xB4` there; MTE and HWASan put a tag there too).

### `DynDeps<D>` (new `rust/tracing/src/dyn_deps.rs`, replaces `property_arena.rs`)

```rust
#[derive(Debug)]
pub struct DynDeps<D> {
    queue: D,                          // dynamic records only
    strings: FxHashMap<u64, Vec<u32>>, // content hash -> string record offsets
    sets: FxHashMap<u64, Vec<DynSet>>, // content hash -> set records
}

#[derive(Debug)]
struct DynSet {
    offset: u32,
    static_values: u64, // the MergedProperties mask the record was built with
}
```

- `D` is the stream's own deps queue type (`LogDepsQueue`, `MetricsDepsQueue`). Each record in
  a queue starts with a type-index byte that refers to that queue type's UDT list, so the
  dynamic queue must be the same type as the output for its bytes to mean the same thing there.
  That makes the append a plain `memcpy`, with no re-encoding.
  `DynDeps::new()` uses `D::new(0)` and doesn't allocate, so thread and image blocks, which
  never insert, pay nothing.
- On the first insert (`queue.capacity_bytes() == 0`), `intern_set`/`intern_str` replace the
  empty queue with `D::new(64 * 1024)`, which uses only the existing `HeterogeneousQueue` API.
  This avoids the early small reallocations under the stream lock; later doublings are amortized
  and accepted.
- `len_bytes()` returns `queue.len_bytes()`. That's exact: the budget no longer uses an
  approximation. The `'static` names and values that dynamic sets reference aren't counted, the
  same as for static dependencies today.

Pushing records from generic code needs a small trait, implemented for `LogDepsQueue` and
`MetricsDepsQueue`:

```rust
pub trait PropertyDepsQueue: HeterogeneousQueue {
    fn push_string(&mut self, dep: StaticStringDependency);
    fn push_set(&mut self, dep: PropertySetDependency);
}
```

This trait also replaces the two `fn` pointer parameters of `record_properties`.

**`intern_set(&mut self, merged: &MergedProperties) -> PropertySetId`** (requires
`D: PropertyDepsQueue`):

1. Hash the content the same way as today (chained xxh3 over names and values).
2. **Hit check.** For each candidate in `sets[hash]`, decode the set record at its offset from
   the queue bytes and compare it with `merged` by content:
   - Names are `'static`, so they're compared through `as_str`.
   - A value whose bit is set in the candidate's `static_values` is a `'static` pointer. Any
     other value is resolved to its string record in the queue through `dyn_offset`.

   On a match, return `dyn_set_id(offset)`. A hit makes one probe and allocates nothing, the
   same as today.
3. **Miss.** Build one `Vec<Property>` for the set:
   - **Names** stay `StaticStringRef::from(&'static str)`.
   - **Static values** (marked in the `static_values` mask) keep their `'static` pointer. This
     is the #1665 fix.
   - **Dynamic values** go through `intern_str`, and the property stores the returned id as its
     `StaticStringRef::ptr`, with the real `len` and `codec`. That ref is only serialized, never
     dereferenced: it's written into the queue and dropped.

   Then push the set record with id `dyn_set_id(queue.len_bytes())` and record
   `DynSet { offset, static_values: merged.static_values }` in `sets[hash]`.

**`intern_str(&mut self, value: &str) -> u64`**:

- Hash the value with xxh3 and probe `strings`. Candidates are compared against the bytes of
  the string record at their offset.
- On a miss, push a `StaticStringDependency { id: dyn_string_id(offset), codec: Utf8, len, ptr }`.
  `push` copies the bytes into the queue right away, so the caller's `&str` only needs to live
  for the duration of the call.
- Dynamic values are therefore deduplicated across sets. For example, a `correlation_id` used
  with two different property combinations is sent once.

**`set_properties(&self, id: PropertySetId) -> Vec<(&str, &str)>`**: decodes an interned or
dynamic set for in-process readers (test helpers).

The hit check, `set_properties` and `extract` all rely on one invariant: every name ref in a
set record, and every value ref whose `static_values` bit is set, was built from a
`&'static str`, so `as_str` on it is sound.

The hit check and `set_properties` decode by parsing `queue.as_bytes()` at the stored offset,
never through `read_value_at_offset` or `iter`: `StaticStringDependency::read_value` and
`PropertySetDependency::read_value` are `unimplemented!()`. Each record is a type byte and a
`u32` size (dynamic types), then:

- string record: id `u64`, codec `u8`, len `u32`, bytes;
- set record: id `u64`, count `u32`, then raw `Property` structs written with `write_any`.
  They're padded, so read them unaligned with `read_any`.

### `MergedProperties` (#1665)

`merge_properties` moves into `dyn_deps.rs` and returns:

```rust
pub struct MergedProperties<'a> {
    pub properties: Vec<DynProperty<'a>>,
    static_values: u64, // bit i set: properties[i].value comes from the &'static set
}
```

- `merge_properties` merges into `(DynProperty, bool)` pairs, sorts them with the same comparator
  as today (`sort_dyn_properties`), and builds the mask from the sorted flags, so bit i refers to
  the post-sort index.
- Sinks still receive `&merged.properties`, so `EventSink` doesn't change.
- Only the non-spread form folds callsite literal pairs into the interned static set (by
  `callsite_set`). With a `..set` spread, the macros pass every literal as a `DynProperty`
  through `__dyn_slice!`, so the literals stay dynamic: they get no `static_values` bit and are
  still copied into the queue, but deduplicated by content through `intern_str`.
- A set with more than 64 properties treats the extra ones as dynamic. That's still correct;
  those values just aren't deduplicated against their originals.
- The mask is what justifies building `StaticStringRef` values from the merged `&str` (with the
  existing `from_str_unchecked`).

### Event field: `PropertySetId`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PropertySetId(pub(crate) u64);
impl PropertySetId {
    pub fn interned(set: &'static PropertySet) -> Self;
    pub fn as_interned(self) -> Option<&'static PropertySet>; // None when bit 0 is set
}
```

- The tuple field is `pub(crate)`: `dyn_deps.rs` (`intern_set`) and the log events' `read_value`
  (`logs/log_events.rs`) construct ids from raw `u64`s directly. There is no public raw
  constructor, which is what keeps the safe `as_interned` sound.
- The four tagged events replace `properties: *const PropertySet` with
  `properties: PropertySetId`.
- `#[repr(transparent)]` guarantees the same layout as a `u64`. The metric events are serialized
  by copying the raw struct (`impl InProcSerialize for TaggedFloatMetricEvent {}`), so their
  bytes depend on it.
- `as_interned` is safe, because an id with bit 0 clear can only come from a `&'static PropertySet`.
  It tests bit 0 only, so it also holds for top-byte-tagged pointers.
  This removes the `unsafe` from `record_properties` and the arena clause of the validity
  contract on `PropertySetDependency`.
- Log events already serialize the field by hand as a `u64`, so their wire layout is unchanged.
- The metric events derive `TransitReflect`, which only marks pointer and reference types as
  references. Add a field attribute `#[transit(reference)]` to the derive
  (`rust/transit/derive/src/derive_reflect.rs`, registered through `attributes(transit)` on the
  derive in `rust/transit/derive/src/lib.rs`) and put it on both metric `properties` fields.
  The attribute forces the same `is_reference` flag that the derive otherwise computes from the
  syntactic type; that flag feeds both the emitted `Member` and the secondary-UDT loop, which must
  skip the field (otherwise the derive emits a nonexistent `PropertySetId::reflect()`).
  Their UDT keeps the same member name, offset, size and `is_reference: true`.
- The member's `type_name` string changes, from the pointer type's text to `PropertySetId`. The
  parser reads reference members before it looks at `type_name` (`parser.rs:174`), so nothing
  reads that string.

### Dependency record changes

- **`StaticStringDependency`** (transit) gains an explicit `id: u64` field, and `write_value`
  writes it instead of `ptr as u64`. Existing constructors (for example
  `StaticStringRef::into_dependency`) set `id = ptr`. The wire format is unchanged.
- **`PropertySetDependency`** becomes `{ id: u64, properties: Cow<'static, [Property]> }`.
  - At extraction, interned sets borrow their `&'static` slice.
  - Dynamic sets own the `Vec` built on a miss, which is dropped right after `push`. That's one
    allocation per miss, against 2 + N today.
  - The raw pointer and its safety contract go away.

### Block, stream and extraction

- `EventBlock::properties: PropertyArena` becomes `dyn_deps: DynDeps<Q::DepsQueue>`, with an
  `ExtractDeps` bound on `Q`.
- `ExtractDeps::DepsQueue` gets a `HeterogeneousQueue` bound. `TracingBlock::len_bytes` uses
  `dyn_deps.len_bytes()`.
- `get_block_mut_parts` returns `(&mut Q, &mut DynDeps<Q::DepsQueue>)`.
- `ExtractDeps::extract(&self)` becomes `extract(&self, dyn_deps: &DynDeps<Self::DepsQueue>)`.
  For log and metric queues:
  1. Walk the events as today, including the `last_desc`/`last_set` shortcuts. For a tagged
     event, call `record_properties` only when `as_interned()` is `Some`; skip dynamic set ids.
  2. For each `DynSet` in `dyn_deps.sets` (order doesn't matter: all of these go in the static
     part), decode the set record and push each name ref, and each value ref whose
     `static_values` bit is set, as a
     `StaticStringDependency` if `recorded.insert(id)`. This runs on the sink thread, outside
     the stream lock, once per distinct dynamic set.
  3. Append the dynamic queue with `deps.extend_from(dyn_deps.queue())`. This is a new
     `HeterogeneousQueue` method generated by `declare_queue_struct` that takes `&Self`, so
     only a queue of the same type, with the same type indices, can be appended. It does
     `buffer.extend_from_slice(other.as_bytes())` (a `memcpy`) and adds `other`'s object count.
- Thread and image queues ignore the argument.
- `encode_block` calls `block.events.extract(&block.dyn_deps)`.

## Implementation Steps

1. **Transit.**
   - Add the `id` field to `StaticStringDependency` and update every constructor. The
     `StaticStringRef::into_dependency` update (`id: self.ptr`) lives in
     `rust/tracing/src/static_string_ref.rs`, in the tracing crate.
   - Add `extend_from` to `HeterogeneousQueue` and generate it in
     `rust/transit/derive/src/declare_queue.rs`.
   - Add the `#[transit(reference)]` field attribute in
     `rust/transit/derive/src/derive_reflect.rs`, overriding `is_reference` for both the `Member`
     emission and the secondary-UDT skip.
   - Register the attribute in `rust/transit/derive/src/lib.rs`:
     `#[proc_macro_derive(TransitReflect, attributes(transit))]`. Without it,
     `#[transit(reference)]` fails to compile.
2. **Property types** (`rust/tracing/src/property_set.rs`):
   - Add `PropertySetId` (`#[repr(transparent)]`).
   - Add the `const` assert that `align_of::<PropertySet>() >= 2`.
   - Reshape `PropertySetDependency` to `{ id, properties: Cow }`.
   - Add the `PropertyDepsQueue` trait (see Design).
   - Make `record_properties` safe: it takes `&'static PropertySet` and a
     `D: PropertyDepsQueue`.
3. **`dyn_deps.rs`**: add `DYN_ID_TAG`, the id helpers, `DynDeps`, `DynSet`, `MergedProperties` and
   `merge_properties`. Delete `property_arena.rs` and update `rust/tracing/src/lib.rs` (same
   native-only `cfg`).
4. **Queues** (`logs/block.rs`, `metrics/block.rs`):
   - Implement `PropertyDepsQueue`.
   - Change `extract` to the new signature and three-step order (step 2 decodes the set records
     through `DynDeps`).
   - Update `spans/block.rs` and `images/block.rs` to the new signature.
5. **Events**: switch the four tagged events to `PropertySetId` (`logs/log_events.rs`,
   `metrics/metric_events.rs`).
6. **Block and stream**: change the `dyn_deps` field, its bounds and `len_bytes` in
   `event/block.rs`, and `get_block_mut_parts` in `event/stream.rs`. Struct bounds don't
   propagate, so add `Q: ExtractDeps` to `impl<Q> EventBlock<Q>` (`close`/`close_at`) and
   `impl<Q> EventStream<EventBlock<Q>>`.
7. **Dispatch**: add `Q: ExtractDeps` to `swap_block<Q>`, switch the four dynamic paths in `dispatch.rs` to `merge_properties` →
   `intern_set`, pass `&merged.properties` to sinks in the two log paths (`log_dyn_tagged_to_sink`, and `log_interop_dyn_tagged`, which builds its `filtered` list from `merged.properties`; the metric paths call no sink), and wrap the static paths' sets with
   `PropertySetId::interned`.
8. **Sink**: update `encode_block` in `rust/telemetry-sink/src/stream_block.rs`.
9. **Callers**: update the bench (`rust/tracing/benches/properties.rs`), the tracing tests and
   the analytics, object-cache, object-cache-srv and telemetry-sink tests listed above, using
   the migration named for each site. Rename `property_arena_tests.rs` to `dyn_deps_tests.rs`.
10. **Measure**: run all three tracing benches (`cargo bench -p micromegas-tracing --bench
    properties`, `--bench with_dispatch`, `--bench with_no_dispatch`) on the baseline (main,
    before any change) and on the finished branch. Acceptance: no regression beyond noise on any
    bench case. Add a `CHANGELOG.md` entry with the before/after numbers and a **Minor breaking
    change** clause.

## Files to Modify

- `rust/transit/src/static_string.rs`, `rust/transit/src/heterogeneous_queue.rs`
- `rust/transit/derive/src/declare_queue.rs`, `rust/transit/derive/src/derive_reflect.rs`,
  `rust/transit/derive/src/lib.rs`
- `rust/tracing/src/dyn_deps.rs` (new), `rust/tracing/src/property_arena.rs` (deleted),
  `rust/tracing/src/lib.rs`
- `rust/tracing/src/property_set.rs`, `rust/tracing/src/static_string_ref.rs`
- `rust/tracing/src/event/block.rs`, `rust/tracing/src/event/stream.rs`
- `rust/tracing/src/logs/block.rs`, `rust/tracing/src/logs/log_events.rs`
- `rust/tracing/src/metrics/block.rs`, `rust/tracing/src/metrics/metric_events.rs`
- `rust/tracing/src/spans/block.rs`, `rust/tracing/src/images/block.rs`
- `rust/tracing/src/dispatch.rs`, `rust/tracing/src/macros.rs` (two doc comments)
- `rust/telemetry-sink/src/stream_block.rs`
- `rust/tracing/benches/properties.rs`
- `rust/tracing/tests/property_arena_tests.rs` → `dyn_deps_tests.rs`,
  `rust/tracing/tests/stream_flush_tests.rs`, `rust/tracing/tests/property_macro_tests.rs`
- `rust/analytics/tests/log_tests.rs`, `rust/analytics/tests/metrics_test.rs`,
  `rust/analytics/tests/parse_corrupt_block_tests.rs`
- `rust/object-cache/tests/foyer_backend_tests.rs`,
  `rust/object-cache-srv/tests/telemetry_tests.rs`,
  `rust/telemetry-sink/tests/tracing_interop_tests.rs`
- `CHANGELOG.md`

## Decisions

- The dynamic queue is appended after the static dependencies, never prepended. It holds only
  records with dynamic ids.
- The dynamic queue is the same type as the stream's static deps queue, so the type-index
  bytes match and the append is a `memcpy`.
- Events that carry only static data get no new work on the calling thread.
- Property names stay `'static` and are deduplicated by pointer, under the existing cardinality
  contract.
- Log message bodies stay inline `DynString`s. They're too high-cardinality for dedup to pay
  off.
- #1665 is folded into this plan through `MergedProperties::static_values`.
- The user requires no performance degradation on any tracing bench.
- Declined: pre-building the whole dependency list at push time, since every static event would pay a dedup probe under the stream lock.
- Declined: a two-level set key `(name_ptr, value_id)`, since it costs one string probe per dynamic value on every hit, and hits are the hot path.
- Declined: pointer ids from a stable bump allocator, since it needs memory addressable for the whole block while tagged offsets need none.
- Declined for now: a chunked dynamic queue, since the `Vec` growth copy is bounded by the block budget; chunking stays internal to `DynDeps` if benches show tail latency.
- Declined: a hand-written `Reflect` for the two metric events, since it duplicates layout code that the derive attribute covers.
- Declined: recording the `'static` refs of dynamic sets in a list on each miss, since it adds
  work under the stream lock and memory outside the block budget; `extract` derives them from
  the set records instead.
- The design must work unchanged on every platform, including arm64 with heap pointer tagging
  (Android); no in-process check relies on a pointer's high bits.

## Documentation

No mkdocs page describes the arena or the dependency stream. Documentation updates:

- The `CHANGELOG.md` entry.
- The "Dynamic properties" section of the crate docs in `rust/tracing/src/lib.rs`, which says
  that with a spread every pair is copied. That no longer holds for the spread set's values.
- The doc comments in `rust/tracing/src/macros.rs` (`:164-166` metric macros, `:325-327` log
  macros) that say a spread copies all pairs into the block.
- Arena wording in comments: `rust/tracing/src/property_set.rs` (`merge_properties` and "the
  arena set"), `event/stream.rs`, `event/block.rs`, and the SAFETY comments about "this block's
  arena" in `logs/block.rs` and `metrics/block.rs`.

## Testing Strategy

All of these are no-DB unit tests.

**`rust/tracing/tests/dyn_deps_tests.rs`**, which replaces the arena tests:

- Interning the same set twice returns the same id and doesn't grow `len_bytes`.
- Different values give different ids.
- A set given as static plus dynamic values and the same set given as all-dynamic values get
  the same id.
- A dynamic value used in two different sets produces one string record.
- Static values from the callsite set produce no string record in the queue, and `extract`
  emits them once with their original pointer (#1665).
- `extract` emits each name and static value referenced only by dynamic sets exactly once,
  before the appended queue bytes.
- `len_bytes()` equals the queue's byte length.
- Every dynamic id has `DYN_ID_TAG` set; set ids have bit 0 set and string ids have it clear.
- `as_interned` returns `Some` for `PropertySetId::interned` and `None` for every dynamic set id.
- `set_properties` decodes both interned and dynamic sets.

**Round trip through the real parser** (`rust/analytics/tests/log_tests.rs` and
`metrics_test.rs`, which already encode blocks and parse them):

- Extend the existing `test_dynamic_tagged_log_entries` (`log_tests.rs`) and
  `test_dynamic_tagged_measures` (`metrics_test.rs`), which already encode and parse merged
  sets, with interned-set events sharing names, rather than adding a separate test. A block that mixes interned sets, dynamic sets sharing names with them, and static values
  inside dynamic sets parses without `duplicate dependency id` or `dependency not found`, with
  the right properties on every event. This pins the ordering constraint that is the main
  correctness risk.
- Tagged metric events still resolve `properties`.

**Wire layout:** `TaggedFloatMetricEvent::reflect()` and `TaggedIntegerMetricEvent::reflect()`
keep the `properties` member at the same offset and size with `is_reference: true`. The
assertions go in `rust/tracing/tests/dyn_deps_tests.rs`.

**Existing coverage:** `stream_flush_tests.rs` and `property_macro_tests.rs` keep their
assertions through `set_properties` and `PropertySetId` equality.

**Bench:** the `properties`
bench reports `extract` time and dependency bytes per event. The #1665 example (about 226 bytes
per event against 73) should get close to the dynamic-value-only figure.
