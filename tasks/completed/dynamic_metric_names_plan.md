# Dynamic Metric Names, Leak-Free C API Properties Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1676 (follow-up to #1654,
sibling of #1677)

## Overview

A metric's name and unit come from a `StaticMetricMetadata` whose fields are `&'static str`. To
record a metric whose name or unit is only known at run time, you have to leak or intern the
strings for the life of the process. The C API does exactly that today. Analytics already
treats `name` and `unit` properties on tagged metric events as overrides of the descriptor's
columns, and since #1654 dynamic property values are written to the block's dependency queue
with block-scoped ids. So dynamic names and units can travel as dynamic property overrides.
This plan adds `imetric_dyn!`/`fmetric_dyn!` macros that do this, moves the C API onto the same
path, adds C functions that take log and metric properties, and exposes them from the Blender
add-on. Nothing the C API receives may outlive its block. C property keys are runtime strings,
so `DynProperty` learns to carry a block-scoped name. Interop log targets, which `mm_log` and
the `tracing` bridge intern for the life of the process today, become block-scoped string records
too. Event names, the wire layout, analytics and the SQL schema don't change, and static metrics
and logs run exactly the same code as today.

## Current State

**Static path.** `__metric_call!` (`rust/tracing/src/macros.rs:670-697`) builds a per-callsite
`static StaticMetricMetadata`, so `$name` and `$unit` must be constant expressions. Events carry
`desc: &'static StaticMetricMetadata` (`rust/tracing/src/metrics/metric_events.rs`), and
`MetricsMsgQueue::extract` (`rust/tracing/src/metrics/block.rs:68`) emits the descriptor's
strings and `MetricMetadataDependency`, deduplicated by address.

**Dynamic properties.** `tagged_integer_metric_dyn` / `tagged_float_metric_dyn`
(`rust/tracing/src/dispatch.rs:853-900`) merge the callsite's static set with a
`&[DynProperty]` (`merge_properties`, `rust/tracing/src/dyn_deps.rs:63`) and intern the result
with `DynDeps::intern_set`. Dynamic values become string records in the block's queue,
deduplicated by content, and the set itself is deduplicated by content. A value in a dynamic
property replaces a static property of the same name.

**Column overrides.** For `TaggedIntegerMetricEvent` and `TaggedFloatMetricEvent`,
`rust/analytics/src/measure.rs:146-158` (and the float arm after it) replace `target`, `name`
and `unit` with the value of a property of that name. The `ticks` → `seconds` conversion runs
after the override, so it sees the overridden unit. The override has been there since #260, so
every analytics service in use reads it. The properties stay in the `properties` column.

**Reserved keys.** The macros reject `target`, `name` and `unit` as property keys
(`macros.rs:771-788`, through `__prop_key!`) because of those overrides. `DynProperty`'s doc
(`rust/tracing/src/property_set.rs:35-40`) says an override can only arrive through a `..set`
spread or a runtime-built `DynProperty` slice. There is no macro for it.

**C API** (`rust/capi/src/lib.rs:27-52`): `intern_metric` keeps a global
`Mutex<HashMap<(String, String), &'static StaticMetricMetadata>>` and `Box::leak`s the name, unit
and descriptor of every new pair. `mkdocs/docs/native/index.md:101` documents it as a
cardinality contract. `mm_metric_i`/`mm_metric_f` (`:208-260`) take no properties, and every C
metric gets `target: "capi"`. `mm_init` (`:115-136`) already reads process properties as
parallel `property_keys`/`property_values` arrays with a `property_count`, skipping a pair whose
key or value is null or whose key is empty or not UTF-8; a non-UTF-8 value is kept as `""`. The header `rust/capi/include/micromegas.h` is maintained by hand (it has
`MM_LEVEL_*` defines and its own doc comments).
**Interop logs** (`rust/tracing/src/dispatch.rs:1013-1130`): `log_interop` and
`log_interop_dyn_tagged` take a `&LogMetadata<'_>` whose strings can be runtime (`mm_log` builds
one per call from C strings; the `tracing` bridge in `telemetry-sink/src/tracing_interop.rs`
passes `tracing` metadata). Both call `intern_string(desc.target)` and store the result as
`target: StaticStringRef` in `LogStringInteropEvent` or `TaggedLogInteropEvent`
(`rust/tracing/src/logs/log_events.rs`), so every distinct target is kept for the life of the
process. `log_interop`'s static-message path (`dispatch.rs:1022`) writes a
`LogStaticStrInteropEvent` instead, whose `target` is a `StringId` (`#[derive(TransitReflect)]`,
so its member type name is part of the wire format). `LogMsgQueue::extract` (`logs/block.rs:95-118`) emits each target
by dereferencing that pointer. `log_interop_dyn_tagged` filters `target`/`level` properties out
of what the sink's `on_log` sees (`dispatch.rs:1103-1105`; `intern_set` stores the unfiltered set),
so an interop log keeps its own target and level. The `log` crate bridge
(`telemetry-sink/src/log_interop.rs:32-39`) also calls `log_interop` with a runtime target and
benefits with no edit. `mm_log` takes no properties. Tests in `analytics/`
push `LogStaticStrInteropEvent`s with `'static` targets directly (`parse_block_tests.rs`,
`parse_alloc_test.rs`, `parse_corrupt_block_tests.rs`, `log_tests.rs`, several `*_db_test.rs`,
`benches/parse_block.rs`); only `analytics/tests/log_tests.rs` also pushes `LogStringInteropEvent`
and `TaggedLogInteropEvent`.

**Blender add-on**: `blender/micromegas_blender/binding.py` binds `mm_metric_i`/`mm_metric_f`
through ctypes (`:69-83`, wrappers `metric_i`/`metric_f` at `:136-152`) and declares every
symbol's `argtypes` when it loads the library. `MicromegasLib.init` (`:91`) already turns a
`dict` into the parallel key/value arrays for `mm_init`. `__init__.py:292` exposes a public
`log()` for other add-ons that does nothing without an active session and swallows errors. It
takes no properties. There
is no public metric function. Tests run outside Blender against the `RecordingLib` double
(`tests/conftest.py:120`). `build/build_blender_plugin.py` bundles the C API library into the
add-on.
**Property names are `'static`.** `DynProperty::name` is `&'static str`
(`rust/tracing/src/property_set.rs:41`), with public fields; it's built in `__dyn_prop!`
(`macros.rs:722`), `PropertySet::dyn_properties` (`property_set.rs:115`),
`telemetry-sink/src/tracing_interop.rs` and tests. `intern_set` (`dyn_deps.rs:301`) writes every
name as a `StaticStringRef` to its `'static` memory, `same_content` and `set_properties` read
names back through `static_ref_str`, and `record_static_refs` emits every name as a static
string before the queue. Only values have a `static_values` mask that lets them be either
`'static` or a string record in the queue. A runtime key would have to be interned with
`intern_string`, which keeps it for the life of the process.

## Design

### Block-scoped property names

Give names the same treatment values got in #1654: a name is either `'static` (referenced by
address, emitted by `extract`) or a string record in the block's queue.

```rust
#[derive(Debug, Clone, Copy)]
pub struct DynProperty<'a> {
    name: &'a str,
    value: &'a str,
    static_name: bool,
}

impl<'a> DynProperty<'a> {
    pub const fn new(name: &'static str, value: &'a str) -> Self;      // static_name: true
    pub const fn with_dyn_name(name: &'a str, value: &'a str) -> Self; // static_name: false
    pub fn name(&self) -> &'a str;
    pub fn value(&self) -> &'a str;
}
```

The fields are private because `static_name: true` on a non-`'static` name would make
`static_ref_str` read freed memory. `new` keeps the current contract and is what the macros, the
static set's `dyn_properties()` and interop use; `with_dyn_name` is for names from outside Rust.

In `dyn_deps.rs`:
- `MergedProperties` and `DynSet` gain a `static_names: u64` mask next to `static_values`, with
  the same rule: only the first 64 entries can be marked, and an unmarked entry is copied into
  the queue, which is always safe. `merge_properties` sets bit `i` from `static_name`.
- `intern_set` writes a static name as today and a dynamic one as
  `StaticStringRef { ptr: self.intern_str(name), .. }`, the way it writes dynamic values. The
  string record precedes the set record, which the parser's ordering rule requires.
- `same_content` and `set_properties` decode a name through the mask, as they do values.
- `record_static_refs` emits only masked names.

The content hash and the case-insensitive override in `merge_properties` already work on the
text, so they don't change. Analytics resolves names through the dependency map like any other
reference. The extra work on the existing dynamic paths is one mask bit per property; static
events don't touch any of this.

### Block-scoped interop log targets

The two interop events that hold a `StaticStringRef` target (`LogStringInteropEvent` and
`TaggedLogInteropEvent`) keep their names and their `target` member's layout and reflection, but
the member's value becomes the id of a string record in the block's queue, written with
`dyn_deps.intern_str(desc.target)` under the stream lock in place of `intern_string`.

- A new `BlockStringRef { id: u64, len: u32, codec: StringCodec }` in `static_string_ref.rs`
  has the layout of `StaticStringRef`, and its `Reflect` returns `StaticStringRef::reflect()`.
  So the wire bytes and the UDT the parser sees are unchanged. It has no `as_str()`, so those two
  events' `target: BlockStringRef` can't be dereferenced, and the compiler lists every
  site that used to dereference it.
- `DynDeps::intern_str_ref(&str) -> BlockStringRef` wraps `intern_str`. `DynDeps::str(id)`
  decodes the text for in-process readers.
- `log_interop` and `log_interop_dyn_tagged` call `get_block_mut_parts()` (`log_interop`
  currently uses `get_events_mut()`) and intern the target. The formatted-message handling doesn't
  change.
- `log_interop` also emits `LogStringInteropEvent` when `args.as_str()` yields a static message,
  copying the message into its `DynString` and using the block-scoped target, as the
  formatted-message path does. `LogStaticStrInteropEvent` (`target: StringId`) is no longer
  written, but stays a readable event type: its reflection, its `extract` arm and its analytics
  parsing are unchanged, so old data and the analytics tests and bench that push it with
  `'static` targets keep working.
- `extract`: the `LogStringInteropEvent` and `TaggedLogInteropEvent` arms stop emitting the
  target, since it is in the appended queue. The `LogStaticStrInteropEvent` arm is unchanged.
- `analytics/tests/log_tests.rs`, which pushes `LogStringInteropEvent` and
  `TaggedLogInteropEvent`, builds the target through `dyn_deps.intern_str_ref`.

Cost: an `intern_str` probe (content hash plus a compare) replaces `intern_string`, which does a
global interner lookup on every interop log. On the wire, a target is still written once per
block. The bench step measures it.

### Macros

`imetric_dyn!` and `fmetric_dyn!` follow the grammar of `imetric!`/`fmetric!`, including
`target:` and the full property list. `name` and `unit` can be any `AsRef<str>`: a literal,
`&String`, `format!(..)` or `Arc<str>`.

```rust
imetric_dyn!(format!("queue.{queue}.depth"), "count", depth);
fmetric_dyn!(target: "my_crate::io", name, unit, "http.method" = "GET", elapsed);
```

They run the shared `__props!` muncher with a context marked dynamic, so reserved keys, `..set`
placement and value kinds behave as in the static macros. For that context, `__metric_call!`
takes only the `tagged` route, even with no user properties, and:

- builds a callsite `static StaticMetricMetadata` with `name: ""`, `unit: ""` and the usual
  `target`, `file!()`, `line!()` and `lod`;
- puts `DynProperty::new("name", AsRef::<str>::as_ref(&$name))` and the same for
  `unit` at the end of the macro's dynamic slice (`__dyn_slice!` gains a suffix form);
- calls the existing `tagged_integer_metric_dyn` / `tagged_float_metric_dyn`.

`merge_properties` dedups case-insensitively and keeps the last entry, so the macro's `name` and
`unit` win over any case-variant key (`"Name" = v`) and over a `..set` spread containing `name` or
`unit`. Temporaries such as `format!(..)` live
until the end of the dispatch statement, as dynamic property values already do.

The doc comment states the cardinality caveat from the issue. Dedup within a block bounds the
wire cost, but every distinct name is a distinct series for queries and materialized views. It
also states that `name` and `unit` appear in the `properties` column.

Export both macros from the prelude.

### C API

**Existing functions.** `mm_metric_i`/`mm_metric_f` keep their signatures and become thin calls
to the new functions with no properties. Delete `intern_metric`, `metric_interner` and
`MetricKey`. The `Box::leak` of names, units and descriptors goes away, along with the global
mutex. Existing callers, including the Blender binding, don't change.

**New functions**, additive to the ABI. Their property arrays follow `MmConfig`'s convention:

```c
void mm_log_tagged(MmHandle *handle, int level, const char *target, const char *msg,
                   const char *const *property_keys, const char *const *property_values,
                   unsigned int property_count);
void mm_metric_i_tagged(MmHandle *handle, const char *name, const char *unit,
                        const char *const *property_keys, const char *const *property_values,
                        unsigned int property_count, uint64_t value);
void mm_metric_f_tagged(MmHandle *handle, const char *name, const char *unit,
                        const char *const *property_keys, const char *const *property_values,
                        unsigned int property_count, double value);
```

All three use one `#[doc(hidden)] pub` function, `read_properties(keys, values, count) ->
Vec<DynProperty>`, which tests can reach. The metric functions build their list through a
second `#[doc(hidden)] pub fn metric_properties` (keys, values, count, name, unit ->
`Vec<DynProperty>`) that
calls `read_properties` and appends `name`/`unit`, since a test can't get a non-null `MmHandle`
(only `mm_init` creates one). `read_properties` reads each pair exactly as `mm_init` does: it skips the pair
when the key or value is null or the key is empty or not UTF-8, and a non-UTF-8 value becomes
`""`. Null arrays or a zero count
mean no properties. Keys use `DynProperty::with_dyn_name`.

`mm_log_tagged` keeps `mm_log`'s argument handling (null handle or message is a no-op, null
target means `"capi"`, level mapping). It calls `log_interop_dyn_tagged`, or `log_interop` when
there are no properties. `mm_log` becomes `mm_log_tagged` with no properties. Interop logs
filter `target` and `level` keys out of what the sink sees, so the arguments always decide those.

The metric functions:
- return early on a null handle or a null, empty or non-UTF-8 name, like today, and treat a
  null unit as `""`;
- append `DynProperty::new("name", ..)` and `DynProperty::new("unit", ..)` to the pairs.
  `merge_properties` keeps the last entry of a name, so the arguments win over keys of the same
  name, and a `target` key overrides the default `"capi"` target;
- call `tagged_integer_metric_dyn` / `tagged_float_metric_dyn` with one static descriptor per
  type (`lod: Verbosity::Min`, `target: "capi"`, `file: "micromegas-capi"`, `line: 0`, `name: ""`, `unit: ""`; the other values
  are what `intern_metric` uses today) and `PropertySet::empty()`.

Names, units, targets, keys and values are all freed with their block; the C API keeps nothing for the
life of the process. Cardinality still matters for queries: every distinct name is a series and
every distinct property set is a dictionary entry.

Add the three declarations to `include/micromegas.h` by hand, in its existing style. Don't
regenerate it with cbindgen, which would drop the hand-written content.

### Blender add-on

- `binding.py`: declare `mm_log_tagged`, `mm_metric_i_tagged` and `mm_metric_f_tagged`.
  `log`, `metric_i` and `metric_f` gain `properties: Optional[dict] = None` and call the
  `_tagged` function when it's non-empty. They build the arrays with one helper shared with
  `init`. The public `__init__.py` functions apply `str()` to
  values, like `log()`'s message; the shared helper doesn't, so `init`'s behaviour is unchanged.
- `__init__.py`: the public `log()` gains `properties=None`, which keeps existing callers
  working. New public `metric_i(name, unit, value, properties=None)` and `metric_f(...)` sit next
  to it with the same contract: no-op without a session, exceptions swallowed, so other add-ons
  can call them unconditionally.
- `tests/test_init.py`'s `CountingLib.log` and `tests/test_public_log_api.py`'s `RaisingLib.log`
  gain `properties=None` too.
- `tests/conftest.py`: `RecordingLib.log`/`metric_i`/`metric_f` accept `properties=None`. They
  keep recording the existing `(level, target, msg)` and `(name, unit, value)` 3-tuples and
  record properties in separate parallel `log_props`/`metric_props` lists.
- The add-on bundles its own library build, so the new symbols are always present. An add-on
  loaded against an older library would fail when it declares the `argtypes`; that combination
  doesn't ship.

## Implementation Steps

1. **Bench baseline**: in its own commit, add to `rust/tracing/benches/with_dispatch.rs`
   `dispatch/metric_dyn_prop` (an `imetric!` with one dynamic property, the cost floor for the new
   macros) and `dispatch/log_interop` (`log_interop` with a runtime-built `LogMetadata`, as
   `mm_log` does). Run the tracing benches on that commit.
2. **Block-scoped names** (`rust/tracing/src/property_set.rs`, `rust/tracing/src/dyn_deps.rs`):
   make `DynProperty`'s fields private, add `new`, `with_dyn_name` and the accessors, and add the
   `static_names` mask to `MergedProperties`, `DynSet`, `intern_set`, `same_content`,
   `set_properties` and `record_static_refs`. Migrate the construction sites (`__dyn_prop!`,
   `dyn_properties()`, `tracing_interop.rs`, `analytics/tests/{log,metrics}_test.rs`,
   `tracing/tests/dyn_deps_tests.rs`) to `DynProperty::new`, and the direct field reads to the
   accessors: `dispatch.rs:1104`, `dyn_deps.rs:69,81-83,303,314,318-319,324`,
   `tracing/tests/property_macro_tests.rs:73`, `telemetry-sink/tests/tracing_interop_tests.rs:112`
   and `tracing/tests/dyn_deps_tests.rs:425,739-741`. `merge_properties` computes `static_values`
   by pointer comparison after its sort (`dyn_deps.rs:73-87`), so it sets the `static_names` bits
   after the sort too; it needs a `pub(crate)` static-name accessor (`is_static_name`) unless the
   API above already has one. Give the wasm32 stub `DynProperty`
   in `rust/tracing/src/lib.rs` the same constructors and accessors.
3. **Interop targets** (`rust/tracing/src/static_string_ref.rs`, `dyn_deps.rs`,
   `logs/log_events.rs`, `logs/block.rs`, `dispatch.rs`): `BlockStringRef`, `intern_str_ref`,
   `str`, the `target` type of `LogStringInteropEvent` and `TaggedLogInteropEvent`, the two
   dispatch functions (`log_interop`'s static-message path emits `LogStringInteropEvent`) and
   their `extract` arms. Migrate `analytics/tests/log_tests.rs` (the only analytics file that
   builds those two events; the tests and bench that push `LogStaticStrInteropEvent` are
   unchanged), and `extract_interop` in
   `rust/tracing/tests/dyn_deps_tests.rs:242`, which builds a `TaggedLogInteropEvent` and asserts
   `extract`'s records.
4. **Macros** (`rust/tracing/src/macros.rs`): `imetric_dyn!`, `fmetric_dyn!`, the dynamic
   context in `__metric_call!`, and the suffix form of `__dyn_slice!`. Add the prelude export in
   `rust/tracing/src/lib.rs`.
5. **C API** (`rust/capi/src/lib.rs`): add `read_properties`, `metric_properties`, `mm_log_tagged`,
   `mm_metric_i_tagged` and `mm_metric_f_tagged`, route the existing functions through them, and
   remove the interner. Add the declarations to `rust/capi/include/micromegas.h` by hand.
6. **Blender add-on** (`blender/micromegas_blender/binding.py`, `__init__.py`,
   `tests/conftest.py`): the binding, the public functions and the test double. Rebuild the
   bundled library with `python3 build/build_blender_plugin.py` to try it in Blender.
7. **Tests** (see Testing Strategy).
8. **Benches, docs and changelog**: add `dispatch/metric_dyn` (same name on every call) and
   `dispatch/metric_dyn_high_card` (names from a rotating set of 10k precomputed strings). Run
   all three tracing benches, update the docs listed below, and add the `CHANGELOG.md` entry
   with the numbers and a **Minor breaking change** clause: `DynProperty`'s fields are private,
   built with `DynProperty::new` / `with_dyn_name`; `LogStringInteropEvent`'s and
   `TaggedLogInteropEvent`'s `target` is a `BlockStringRef`.

## Files to Modify

- `rust/tracing/src/macros.rs`, `rust/tracing/src/lib.rs`, `rust/tracing/src/property_set.rs`,
  `rust/tracing/src/dyn_deps.rs`, `rust/tracing/src/static_string_ref.rs`,
  `rust/tracing/src/dispatch.rs`, `rust/tracing/src/logs/log_events.rs`,
  `rust/tracing/src/logs/block.rs` (`lib.rs` includes the wasm32 `DynProperty` stub)
- `rust/telemetry-sink/src/tracing_interop.rs`
- `rust/tracing/benches/with_dispatch.rs`
- `rust/tracing/tests/property_macro_tests.rs`, `rust/tracing/tests/dyn_deps_tests.rs`,
  `rust/tracing/tests/interop_log_tests.rs` (new), `rust/telemetry-sink/tests/tracing_interop_tests.rs`
- `rust/capi/src/lib.rs`, `rust/capi/include/micromegas.h`, `rust/capi/README.md`,
  `rust/capi/cbindgen.toml`,
  `rust/capi/tests/smoke_test.rs`, `rust/capi/tests/properties_tests.rs` (new)
- `rust/analytics/tests/metrics_test.rs`, `rust/analytics/tests/log_tests.rs`
- `blender/micromegas_blender/binding.py`, `blender/micromegas_blender/__init__.py`,
  `blender/micromegas_blender/tests/conftest.py`,
  `blender/micromegas_blender/tests/test_public_log_api.py`,
  `blender/micromegas_blender/tests/test_init.py`,
  `blender/micromegas_blender/tests/test_public_metric_api.py` (new),
  `blender/micromegas_blender/tests/test_binding.py` (new)
- `mkdocs/docs/blender/index.md`, `blender/README.md`
- `mkdocs/docs/native/index.md`, `mkdocs/docs/when-to-use/index.md`
- `CHANGELOG.md`

## Trade-offs

- **Property overrides (chosen) vs. new event types** (`Dyn*MetricEvent` referencing a
  dynamic `MetricMetadataDependency` record, with the issue's `DynDeps::intern_desc`). New event
  types would avoid the per-call `merge_properties` `Vec` and keep `name`/`unit` out of the
  `properties` column. They would also cost four event types, a descriptor index in `DynDeps`,
  changes to `extract`, and new analytics arms that older services skip as unknown events.
  Overrides reuse code every analytics service already runs and add no tracing internals. New
  event types remain the fallback if the bench shows the merge cost matters.
- **Declined: replacing `desc` with a `PropertySetId`-style id** on the existing events (the
  issue's proposal). It changes the static event type and every in-process reader of `desc`, for
  a benefit the new-event-type option also gives.
- **Block-scoped C keys (chosen) vs. a key cache owned by `MmHandle`** and freed at
  `mm_shutdown`. The cache needs no tracing change, but it still grows with every distinct key
  for the life of the handle, which is a leak in practice.
- **`BlockStringRef` vs. reusing `StaticStringRef`** for interop targets. Reusing the type
  needs no new type, but a dynamic id stored in a dereferenceable type is only kept safe by
  convention. A distinct type with the same layout makes the compiler find every reader.
- **Static interop messages as `LogStringInteropEvent` (chosen) vs. a block-scoped variant of
  `LogStaticStrInteropEvent`.** A static interop message is now copied into the block instead of
  referenced by pointer, in exchange for no new reflection type and no change to
  `LogStaticStrInteropEvent`.

## Decisions

- Scope (user call): the `_dyn` metric macros, a leak-free C API that accepts log and metric
  properties, the Blender add-on exposing it, and the doc updates.
- The C API adds `*_tagged` functions instead of changing `mm_log`/`mm_metric_i`/`mm_metric_f`, so
  existing binaries and bindings keep working.
- No leaks (user call): nothing received by the C API, keys included, outlives its block, so
  `DynProperty` gains block-scoped names. Rust callers keep `'static` keys through
  `DynProperty::new`.
- `LogStaticStrInteropEvent` is no longer emitted; static-message interop logs use `LogStringInteropEvent` so its `StringId` target needs no block-scoped variant.
- The pre-existing interop target leak is fixed here, since `mm_log` and `mm_log_tagged` go
  through it.
- The per-call `Vec` of `merge_properties` is acceptable for dynamic metrics (user call).
- Overrides were judged good enough; static metrics must not pay for dynamic ones (user call).
- The Blender add-on exposes the new C API to other add-ons (user call).
- No LOD argument: Rust metric dispatch doesn't filter by LOD. The callsite's static descriptor
  carries `lod` as it does for static metrics.
- Wire format, event names and SQL schema don't change.
- Accepted (user call): `name` and `unit` stay in the `properties` column of dynamic metrics,
  C API metrics included.

## Documentation

- Doc comments on `imetric_dyn!` and `fmetric_dyn!`, with the cardinality caveat.
- The `imetric!` doc (`macros.rs:235`): point at `imetric_dyn!` for runtime names.
- The crate docs in `rust/tracing/src/lib.rs` (metrics section, around `:47`): mention the
  `_dyn` metric macros.
- Stale doc comments: `rust/tracing/src/lib.rs:54` (keys "are always 'static"),
  `rust/tracing/src/lib.rs:83-88` (override only through a `..set` spread or runtime `DynProperty`
  slice), the `__prop_key!` doc at `rust/tracing/src/macros.rs:749-751`, and the wasm32 stub
  `DynProperty` doc at `rust/tracing/src/lib.rs:191-192` ("Names stay 'static").
- The `DynProperty` doc (`property_set.rs:35`): names can be `'static` (`new`) or
  block-scoped (`with_dyn_name`), and overrides also come from the `_dyn` metric macros.
- `rust/capi/include/micromegas.h:3-4` and `rust/capi/README.md:54-55` say the header is
  maintained by hand in sync with `src/lib.rs`, not generated; the stale "Auto-generated by
  cbindgen" header string in `rust/capi/cbindgen.toml:2` is fixed to match.
- `rust/capi/src/lib.rs` module doc, `rust/capi/README.md` (`:8`, `:51`) and
  `mkdocs/docs/native/index.md`. In the native page: the architecture list (`:14`) loses the
  string interner; the function sections document `mm_log_tagged`, `mm_metric_i_tagged` and
  `mm_metric_f_tagged`, with examples that pass properties. They say that only the exact lowercase `target` key
  overrides a metric's target (analytics matches case-sensitively), that logs filter
  `target`/`level` keys out of what the sink sees, and what happens to null pairs. The "Cardinality contract" warning (`:101`) now says names, units, targets, keys and
  values are freed with their block, and distinct names and values still cost series
  and dictionary entries in queries.
- `rust/capi/include/micromegas.h:66-75` ("each unique pair is permanently interned"), the
  `mm_metric_i`/`mm_metric_f` doc comments (`rust/capi/src/lib.rs:203-206`, `:233-235`) and the
  comment at `:26-27`.
- `blender/README.md:256-271` (Cardinality section, which has no interning text): add the same
  line planned for `mkdocs/docs/blender/index.md`, that property keys and values should come from
  bounded sets.
- `mkdocs/docs/when-to-use/index.md:151`: reword the cardinality contract so only static callsite
  names, static targets and static property sets stay interned, and runtime names, targets and
  keys are block-scoped.
- `rust/capi/tests/smoke_test.rs:46,80`: the "interner deduplication" and "the interner must not
  grow" comments.
- `mkdocs/docs/blender/index.md`: a short "Public API for other add-ons" section documenting
  `log()`, `metric_i()` and `metric_f()`, with properties on a log and on a metric. Also a line in
  "Cardinality" (`:210`): property keys and values, like metric names, should come from bounded
  sets.
- The `CHANGELOG.md` entry.

## Testing Strategy

All of these are no-DB unit tests.

**Block-scoped names** (`rust/tracing/tests/dyn_deps_tests.rs`):
- A set with a `with_dyn_name` property writes a string record for the name; the same set
  interned twice returns the same id and doesn't grow `len_bytes`.
- A dynamic name equal to a dynamic value, or to another set's dynamic name, shares one string
  record.
- The same text as a static name and as a dynamic name gives equal content: both sets resolve
  to the same id and `set_properties` returns the same pairs.
- `set_properties` decodes dynamic names; `extract` emits only static names before the queue.

**Interop targets** (`rust/tracing/tests/interop_log_tests.rs`, new):
- `log_interop` with a target built at run time leaves a string record for it in the block, and
  two logs with the same target share it.
- After `flush_log_buffer()`, the next block writes the target again.
- `dyn_deps.str(evt.target.id)` returns the target text.
- A `log_interop` call whose `args.as_str()` is a static message produces a
  `LogStringInteropEvent` (not a `LogStaticStrInteropEvent`) whose target resolves from the block
  and whose `msg` equals the static text.
- `log_interop_dyn_tagged` with a `with_dyn_name` key: the set resolves, the sink's `on_log`
  sees no `target`/`level` keys, and the event keeps its own target and level. This case uses a
  custom recording `EventSink` (modeled on `rust/telemetry-sink/tests/tracing_interop_tests.rs`)
  because `InMemorySink::on_log` ignores its properties.

**Macros** (`rust/tracing/tests/property_macro_tests.rs`, through its `metrics()` helper and
`dyn_deps.set_properties`):
- `imetric_dyn!` with a `format!` name, a `&String` and an `Arc<str>` emits a tagged event whose
  set holds the `name` and `unit` values.
- Two calls with the same runtime name in one block share one set id.
- `fmetric_dyn!` with `target:`, a literal property and a dynamic property: the descriptor holds
  the target, and the set holds all four pairs.
- A `..set` spread containing `name` is overridden by the macro's name.
- `imetric!`/`fmetric!` still emit untagged events and leave `dyn_deps` empty.
- `compile_fail` doc tests, as ```` ```compile_fail ```` blocks in the `imetric_dyn!`/`fmetric_dyn!`
  doc comments in `rust/tracing/src/macros.rs` (as existing there): `imetric_dyn!` with a `name =`
  key, and with a misplaced `..set`.

**Round trip through the real parser** (`rust/analytics/tests/metrics_test.rs`, extending
`test_dynamic_tagged_measures`): a block with static metrics and dynamic-name metrics. It
includes a dynamic name equal to a static metric's name, two names sharing a unit, and a dynamic
`"ticks"` unit. After parsing, each measure has the overridden name and unit, including the
`ticks` → `seconds` conversion. It also includes a set with a dynamic property name, which
must resolve with no `dependency not found` or `duplicate dependency id`. This pins the analytics
contract and the ordering rule that the design depends on. `log_tests.rs` gets the same check for
logs: a block mixing static logs, interop logs with block-scoped targets (including a target
equal to a static log's target) and a tagged interop log with a dynamic key. After parsing, each
entry has the right target, properties and level.

**C API**:
- `rust/capi/tests/properties_tests.rs` (new) tests `read_properties` and
  `metric_properties`:
  - null, empty and non-UTF-8 keys, and null values, are dropped, and a non-UTF-8 value is
    kept as `""`;
  - null arrays with a nonzero count yield no pairs;
  - keys are dynamic names;
  - in `metric_properties`, `name`/`unit` come last, so `merge_properties` lets them win over keys
    of the same name, and a `target` key is kept.
- `rust/capi/tests/smoke_test.rs`: `init_and_exercise_api` (the one test allowed to call
  `mm_init`) also calls `mm_log_tagged`, `mm_metric_i_tagged` and `mm_metric_f_tagged` with and
  without properties,
  so the exported symbols are exercised.

**Blender add-on** (pytest, outside Blender, run like the existing
`blender/micromegas_blender/tests`):
- `test_public_metric_api.py` (new), modeled on `test_public_log_api.py`: `metric_i`/`metric_f`
  forward name, unit, value and properties to the active session; no-op without one; swallow an
  exception from the lib.
- `test_public_log_api.py`: `log()` forwards `properties`, and existing calls without them
  still work.
- `test_binding.py` (new): the shared array helper is unit-tested directly (keys, values and
  count). Symbol selection is checked by building `MicromegasLib` through `object.__new__` with a
  stub `_lib` (its `__init__` calls `ctypes.CDLL` directly): a non-empty `properties` calls the
  `_tagged` symbol with matching keys, values and count, and `None` or `{}` calls the untagged
  symbol, for logs and metrics.

**Bench**: the step 1 and step 8 runs. Acceptance:
- No change beyond noise on any case of `with_dispatch`, `properties` and `with_no_dispatch`,
  and no regression on `dispatch/log_interop`.
- Reported numbers for `dispatch/metric_dyn` and `dispatch/metric_dyn_high_card` against
  `dispatch/metric_dyn_prop`.
