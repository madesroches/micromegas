# Tagged Logs With Static Messages Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1666

## Overview

A tagged log whose message has no arguments, such as `info!(a = "1", "static")` or
`info!(..set, "static")`, allocates twice per call. It collects the interned set into a `Vec`
to hand the sink a slice, and it formats the message into a `DynString` even though the
format string already travels in `LogMetadata`. This plan adds a tagged log event with no
`msg`. Analytics reads the message from `desc.fmt_str`, the same way it does for
`LogStaticStrEvent`. The plan also caches each interned set's `DynProperty` view on the set, so
the sink gets a slice without a per-call `Vec`. The SQL surface doesn't change.

The process picks the static event only when the message it would print is byte-for-byte
`desc.fmt_str`. That also fixes an existing bug in `LogStaticStrEvent`: analytics can store
text that differs from what the process printed (see Current State).

## Current State

**Dispatch** (`rust/tracing/src/dispatch.rs`):

- `log` (`:846`) pushes `LogStaticStrEvent` when `args.as_str()` is `Some`, and
  `LogStringEvent` with `args.to_string()` otherwise.
- `log_tagged_to_sink` (`:887`) collects `properties.dyn_properties()` into a
  `Vec<DynProperty<'static>>` (`:895`) for `sink.on_log`, then always pushes `TaggedLogString`
  with `msg: DynString(args.to_string())` (`:897`).
- `log_dyn_tagged_to_sink` (`:951`) falls back to `log_tagged_to_sink` when `dyn_properties` is
  empty. Otherwise it merges the properties, calls `on_log` with the merged slice, and pushes
  `TaggedLogString` with `msg: DynString(args.to_string())` (`:965`) and the set id that
  `dyn_deps.intern_set` returns.
- Every `log!` macro arm that has properties goes through `log_dyn_tagged` or
  `log_dyn_tagged_enabled` (`rust/tracing/src/macros.rs:544`, `:559`). For literals and `..set`,
  the dynamic slice is empty. `dispatch::log_tagged` is only called from
  `rust/tracing/tests/dyn_deps_tests.rs`.

**`as_str()` is not the format string.** `LogStaticStrEvent` assumes that when
`args.as_str()` is `Some`, the message equals `desc.fmt_str`. With the current toolchain
(rustc 1.96), that's false in two cases:

```
format_args!("a {{b}}").as_str()      == Some("a {b}")     // fmt_str "a {{b}}"
format_args!("value {}", 3).as_str()  == Some("value 3")   // fmt_str "value {}": literal args are inlined
```

So `info!("value {}", 3)` is stored as `value {}`, and `info!("a {{b}}")` as `a {{b}}`, while
the process prints `value 3` and `a {b}`. The issue noted only the escape case.

**Property sets** (`rust/tracing/src/property_set.rs`):

- `PropertySet { properties: Vec<Property> }` (`:82`) derives `Eq` and `Hash` and is interned
  with `internment::Intern` in `find_or_create` (`:97`) after `sort_properties`.
- `dyn_properties(&'static self)` (`:115`) returns an iterator. Its callers are
  `log_tagged_to_sink` and `merge_properties` (`rust/tracing/src/dyn_deps.rs:67`), which
  collects it and then mutates the result.
- `Property` holds two `StaticStringRef`s. A `DynProperty<'static>` made from one points at
  the same `'static` bytes, and `merge_properties`' `static_values` mask relies on that pointer
  identity (`dyn_deps.rs:76`).

**Events** (`rust/tracing/src/logs/log_events.rs`):

- `LogStaticStrEvent { desc, time }` derives `TransitReflect`, so no custom reader is needed.
- `TaggedLogString { desc, properties, time, msg }` has a manual `InProcSerialize` and a custom
  reader (`rust/tracing/src/parsing.rs:319`).
- `TaggedFloatMetricEvent` shows the derive form for a set reference:
  `#[transit(reference)] pub properties: PropertySetId`
  (`rust/tracing/src/metrics/metric_events.rs:43`).

**Extraction** (`rust/tracing/src/logs/block.rs:71`): `LogMsgQueue::extract` records the desc and
interned-set dependencies of each `TaggedLogString`. Dynamic sets come from `dyn_deps`.

**Analytics** (`rust/analytics/src/log_entry.rs`):

- The `LogStaticStrEvent` arm (`:43`) takes `msg` from `desc.fmt_str`.
- The `TaggedLogString` arm (`:153`) reads `msg`, `desc`, `properties`, and applies `target` /
  `level` overrides from the properties.
- An unknown type name hits `_ => { warn!(...); Ok(None) }` (`:197`). The entry is dropped and
  the block still parses.

**Benchmark baseline**: see Benchmarks.

## Design

### Static message test

One helper in `dispatch.rs`:

```rust
/// The message is `fmt_str` verbatim, so analytics can read it from the metadata.
#[inline(always)]
fn is_static_msg(desc: &LogMetadata, args: &fmt::Arguments<'_>) -> bool {
    args.as_str() == Some(desc.fmt_str)
}
```

`&str` equality compares lengths first, so messages with a format argument or an escape usually
exit on the length check. A message that passes the test costs a `memcmp` over its own short
bytes. A message that fails takes the formatting path, so it's stored exactly as printed.

The same helper replaces the `args.as_str()` match in `log`, which fixes `LogStaticStrEvent`
for new data. Blocks already stored keep their current text.

### New event

```rust
#[derive(Debug, TransitReflect)]
pub struct TaggedLogStaticStrEvent {
    pub desc: &'static LogMetadata<'static>,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub time: i64,
}

impl InProcSerialize for TaggedLogStaticStrEvent {}
```

The event is a fixed-size POD: 24 bytes plus its type tag, versus 35 for `TaggedLogString` with a
6-byte message. It's also the shape the deferred-formatting event in #1653 needs, so that work
can reuse it as is.

- Add it to `LogMsgQueue` (`logs/block.rs:13`) and export it from `logs`.
- `extract`: add an arm identical to the `TaggedLogString` one (desc, then interned set).
  Merge the two arms with an or-pattern if they bind the same fields.
- No custom reader: the derive emits the layout, and the generic parser resolves `desc` and
  `properties` through the dependency map, the same as the tagged metric events.

### Dispatch

- `log_tagged_to_sink`: call `sink.on_log(desc, properties.dyn_properties(), time, args)`
  (cached slice, see below). If `is_static_msg`, push `TaggedLogStaticStrEvent`; otherwise
  format and push `TaggedLogString`. Format outside the lock, as today.
- `log_dyn_tagged_to_sink` (non-empty dynamic slice): same choice, with the set id that
  `dyn_deps.intern_set` returns. The merged `Vec` stays: the dynamic path needs it anyway.

### Cached sink view on `PropertySet`

```rust
#[derive(Debug, Clone)]
pub struct PropertySet {
    pub(crate) properties: Vec<Property>,
    dyn_view: Box<[DynProperty<'static>]>, // same order as `properties`
}
```

- `find_or_create` builds `dyn_view` after `sort_properties`, before interning. A lookup that
  finds an existing set throws its copy away. That happens once per callsite or `..set`, not per
  log.
- Write `PartialEq`, `Eq` and `Hash` by hand over `properties` only, so the view never changes
  identity or interning. `DynProperty<'static>` is `Send + Sync`, which `Intern` requires.
- `dyn_properties(&'static self) -> &'static [DynProperty<'static>]` returns the view.
  `merge_properties` uses `.to_vec()`. Values keep pointing at the `StaticStringRef` bytes, so
  the `static_values` mask is unaffected.

This keeps `EventSink::on_log` unchanged. No sink implementation (console, local, composite,
HTTP, in-memory, the test sinks) needs to change.

### Analytics

Add a `"TaggedLogStaticStrEvent"` arm in `log_entry_from_value`. Move the body of the
`TaggedLogString` arm (desc, level/target overrides, properties, `LogEntry`) into a helper that
takes `msg: &'a str`. The two arms call it with `obj.get("msg")` and `desc.get("fmt_str")`
respectively. Analytics doesn't unescape anything: the process only emits the static event when
`fmt_str` is the exact message.

### Wire compatibility

The new event is a new type name. An analytics service older than this change parses the block
(the layout is in the stream metadata), but each `TaggedLogStaticStrEvent` hits the `_` arm: it
logs a warning and the entry is dropped. Upgrade analytics before instrumented processes. The
CHANGELOG entry says so. Unreal instrumentation is unaffected: it doesn't emit the new event.

## Implementation Steps

### Phase 0: Baseline (done)

1. Already captured on `main` at `f14a9bfe0`, before any change, as criterion baseline `before`:
   `cargo bench -p micromegas-tracing --bench properties --bench with_dispatch -- --save-baseline before`
   from `rust/`. Numbers are in Benchmarks. Don't re-save the baseline after making changes.

### Phase 1: Property set view

2. `rust/tracing/src/property_set.rs`: add `dyn_view`, the manual `PartialEq`/`Eq`/`Hash`, and
   change `dyn_properties` to return the slice.
3. `rust/tracing/src/dyn_deps.rs`: in `merge_properties`, use `static_set.dyn_properties().to_vec()`.

### Phase 2: Event and dispatch

4. `rust/tracing/src/logs/log_events.rs`: add `TaggedLogStaticStrEvent`.
5. `rust/tracing/src/logs/block.rs`: add it to `LogMsgQueue` and to `extract`.
6. `rust/tracing/src/dispatch.rs`: add `is_static_msg`. Use it in `log`, `log_tagged_to_sink` and
   `log_dyn_tagged_to_sink`, and pass the cached slice to `on_log` in `log_tagged_to_sink`.
7. Update the in-process readers that match `LogMsgQueueAny`:
   - `rust/tracing/tests/utils.rs:70` (exhaustive match): add the arm.
   - `rust/tracing/tests/property_macro_tests.rs:203`: record the new event as `tagged: true`
     with its set id.
   - `rust/tracing/tests/stream_flush_tests.rs:110` and `rust/tracing/tests/dyn_deps_tests.rs`
     (`:479`, `:543`): match both tagged log events wherever they read `properties`.

### Phase 3: Analytics

8. `rust/analytics/src/log_entry.rs`: factor out the tagged-log helper and add the
   `TaggedLogStaticStrEvent` arm.

### Phase 4: Tests

9. Add the tests listed in Testing Strategy.

### Phase 5: Measure after

10. From `rust/`, on the same machine and under the same conditions:
    `cargo bench -p micromegas-tracing --bench properties --bench with_dispatch -- --baseline before`.
    If a case moves by more than the 3-5% noise in a direction this change doesn't explain, run
    it again before recording it.
11. Fill the "After" column of the Benchmarks table in this plan, and add the headline medians
    to the CHANGELOG entry, following the dyn-deps entry's format.

### Phase 6: Changelog

12. `CHANGELOG.md` under Unreleased, in the **Tracing** style:
    - The new event, and that analytics must be upgraded first.
    - The `LogStaticStrEvent` fix for inlined literal arguments and escaped braces.
    - Before/after medians.
    - **Minor breaking change:** `PropertySet::dyn_properties` returns a slice;
      `LogMsgQueue` gains a variant (exhaustive matches on `LogMsgQueueAny` must add it).

## Files to Modify

- `rust/tracing/src/property_set.rs`
- `rust/tracing/src/dyn_deps.rs`
- `rust/tracing/src/logs/log_events.rs`
- `rust/tracing/src/logs/block.rs`
- `rust/tracing/src/dispatch.rs`
- `rust/analytics/src/log_entry.rs`
- `rust/tracing/tests/utils.rs`, `property_macro_tests.rs`, `stream_flush_tests.rs`,
  `dyn_deps_tests.rs`
- `rust/analytics/tests/log_tests.rs`
- `CHANGELOG.md`

## Trade-offs

- **Unescape in analytics (the issue's proposal) vs. an exact-match test in the process.**
  Unescaping fixes `{{`, but not inlined literal arguments: `fmt_str` `value {}` can't be turned
  into `value 3` after the fact. Comparing `as_str()` with `fmt_str` fixes both, costs a short
  `memcmp`, and needs no analytics logic. Escaped-brace messages are rare, so the formatted
  fallback they take doesn't matter.
- **Cached slice on the set vs. changing `EventSink::on_log` to take the set.** Changing the
  trait would touch every sink and give them two property shapes to merge (set plus dynamic
  slice). The cached view costs one boxed slice per interned set, which is bounded by
  callsites, and no sink changes.
- **Derived POD event vs. a manual `InProcSerialize` like `TaggedLogString`.** The event has no
  variable-length field, so the derive is enough and the parser needs no custom reader.

## Testing Strategy

No-DB unit tests only. Nothing here reproduces a bug seen in the wild, so there's no live-DB
test.

**`rust/tracing/tests/property_macro_tests.rs`**, using its capturing sink and the
`LogMsgQueueAny` walk:

- `info!(a = "1", "static")` and `info!(..set, "static")` push `TaggedLogStaticStrEvent` with the
  interned set id. The sink's `on_log` still receives the set's properties.
- `info!(id = %x, "static")` pushes `TaggedLogStaticStrEvent` with a dynamic set id whose
  `dyn_deps.set_properties` contains `id`.
- `info!(a = "1", "value {}", 3)`, `info!(a = "1", "a {{b}}")` and `info!(a = "1", "v {}", n)`
  push `TaggedLogString` whose `msg` is `value 3`, `a {b}` and the formatted value.
- Untagged: `info!("static")` pushes `LogStaticStrEvent`, while `info!("value {}", 3)` and
  `info!("a {{b}}")` push `LogStringEvent` with `value 3` and `a {b}`. This pins the
  `LogStaticStrEvent` fix.

**`rust/tracing` property-set unit test** (existing property-set test file, or the dyn-deps
tests): `dyn_properties()` matches `get_properties()` name for name and value for value, in the
same order, with value pointers equal to the `StaticStringRef` ids. Two `find_or_create` calls
with the same properties return the same `&'static PropertySet`.

**`rust/analytics/tests/log_tests.rs`**: following the existing `TaggedLogString` round-trip test
(`:249`), build a block with a `TaggedLogStaticStrEvent` on an interned set, and another on a
dynamic set (as `:385` does). Parse both with `log_entry_from_value` and assert `msg` equals
`fmt_str` and the properties come through. Also check a `level` / `target` override property,
since the shared helper carries that logic.

The existing suites (`dyn_deps_tests`, `stream_flush_tests`, analytics log and corrupt-block
tests) cover the rest of the extract and parse plumbing once updated.

## Benchmarks

Command: `cargo bench -p micromegas-tracing --bench properties --bench with_dispatch`, from
`rust/`. Medians. "Before" is `main` at `f14a9bfe0` (criterion baseline `before`).

| Case | Before | After |
|---|---|---|
| log/static | 16.53 ns | |
| log/formatted | 61.42 ns | |
| log/literals | 46.60 ns | |
| log/set | 47.20 ns | |
| log/dyn_low_card | 71.58 ns | |
| log/dyn_high_card | 181.83 ns | |
| log/dyn_mixed | 297.28 ns | |
| extract/log/static | 44.20 µs (17 B/event) | |
| extract/log/set | 271.40 µs (40 B/event) | |
| extract/log/dyn_low_card | 266.26 µs (40 B/event) | |
| extract/log/dyn_high_card | 355.06 µs (40 B/event) | |
| extract/log/dyn_mixed | 602.34 µs (40 B/event) | |
| contention/log/static 1/4/8 | 16.14 ns / 320.03 ns / 902.71 ns | |
| dispatch/log | 446 ps | |

Expected: `log/literals`, `log/set` and the `log/dyn_*` cases get faster, because they all use a
static `"static"` message. Their object bytes/event drop from 40, and their `extract` times
follow. `log/static`, `log/formatted`, the metric cases and `dispatch/*` are controls and should
stay within noise. `log/static` gains only the `memcmp`.

## Documentation

No `mkdocs/` page documents log event types or this behavior. `CHANGELOG.md` only.
