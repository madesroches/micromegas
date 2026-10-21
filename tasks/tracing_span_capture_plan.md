# TracingCaptureLayer Span Capture Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1704

## Overview

`TracingCaptureLayer` forwards `tracing` events only; spans are dropped. Map a `tracing` span's
lifetime (`on_new_span` → `on_close`) to an async span begin/end pair, carrying a span id, the
parent's span id, depth, and the span's fields as end-event properties. The analytics side
(`async_events`, `process_spans(..., 'async')`) then reconstructs the tree with no changes. This
lets applications whose libraries emit `tracing` spans (e.g. OTel GenAI-semconv spans) use
micromegas as a native backend with trace trees.

## Current State

- `rust/telemetry-sink/src/tracing_interop.rs` implements only `enabled` and `on_event`; the
  layer is `TracingCaptureLayer { max_level }`, installed by `install_tracing_interop`.
- Field capture is already solved for events: `FieldFormatVisitor` records fields into a reusable
  thread-local `Scratch` and builds `DynProperty` slices.
- Async span emission API (`rust/tracing/src/dispatch.rs`):
  - `on_begin_async_named_scope_dyn(&'static SpanLocation, name, parent_span_id, depth) -> u64`
    allocates the span id from `G_ASYNC_SPAN_COUNTER`.
  - `on_end_async_named_scope_tagged(span_id, parent_span_id, &'static SpanLocation, name, depth,
    &'static PropertySet, &[DynProperty])` falls back to the plain dyn end when there are no
    properties. `PropertySet::empty()` is available for the static part.
- A root async span has `parent_span_id = 0` and `depth = 0` (`ASYNC_CALL_STACK` starts as
  `vec![0]`, `instrumented_future.rs:18`). `InstrumentedNamedFuture` uses `depth = stack.len()-1`
  relative to the *current* async stack entry, i.e. a child of a root is depth 1.
- Span locations are keyed by the address of the `&'static SpanLocation`
  (`rust/tracing/src/spans/block.rs:104`), so each distinct callsite needs one stable,
  never-moved `SpanLocation`.
- The begin event is written to the thread block current at begin; the end event is written to
  whichever block is current at end, possibly on another thread. Analytics already handles this
  for native async spans.

## Design

### Span lifecycle mapping

Store per-span state in the `tracing_subscriber` registry via span extensions:

```
struct CapturedSpan {
    span_id: u64,
    parent_span_id: u64,
    depth: u32,
    location: &'static SpanLocation,
    fields: Vec<(&'static str, String)>,
}
```

- `on_new_span(attrs, id, ctx)`:
  1. Resolve the parent with `attrs.parent()` / contextual parent (`ctx.span(id).parent()`), taking
     the parent's `CapturedSpan` extension: `parent_span_id = parent.span_id`,
     `depth = parent.depth + 1`; a parent with no `CapturedSpan` extension → `0` / `0`. `enabled`
     already filters span metadata, so a level-filtered span never reaches the registry and
     `on_new_span` needs no level check of its own. Only contextual children attach to the nearest
     enabled ancestor; a child with an explicit `parent:` naming a filtered span becomes a root
     (`0` / `0`).
  2. Record the `attrs` fields into a visitor (see below) and stash them in the extension —
     fields known at creation are needed at end, and `on_record` may add more.
  3. Call `on_begin_async_named_scope_dyn(location, span.name(), parent_span_id, depth)`; store the
     returned id.
- `on_record(id, values, ctx)`: append the recorded fields to `CapturedSpan::fields`, overwriting by field name (late-bound
  `Empty` fields are the common OTel pattern).
- `on_close(id, ctx)`: take the extension, call `on_end_async_named_scope_tagged(span_id,
  parent_span_id, location, name, depth, PropertySet::empty(), &dyn_properties)`.

`Metadata::name()` is used as the name for both the begin and end events (analytics reads them
independently). The span name is not stored in the extension; it is re-read from `ctx.span(id)`.

Using `on_close` (not `on_exit`) matches the issue's "open/close" wording and the async model: a
`tracing` span entered/exited repeatedly across `.await`s is one logical operation, whereas
enter/exit would produce one span per poll of an instrumented future and break parent/child
semantics for tasks migrating across threads. `on_enter`/`on_exit` are not mapped.

### Span location

`attrs.metadata()` is not `'static`; build the location from `ctx.span(id).metadata()`, which is
`&'static Metadata<'static>` and unique per callsite. Keep a global
`Mutex<HashMap<callsite identifier, &'static SpanLocation>>` (keyed by the `Metadata` pointer) that
leaks one `SpanLocation { lod, target, module_path, file, line }` per callsite on first use. The
set of callsites is bounded by the program text, so the leak is bounded. `module_path` and `file`
use `unwrap_or_default()` and `line` uses `unwrap_or(0)`, since tracing exposes them as Options.
`lod: Verbosity` is mapped from the tracing level by a new helper (the existing helper returns
`Level`); default mapping: ERROR/WARN → Min, INFO → Med, DEBUG/TRACE → Max.

### Properties

Span fields become dynamic properties (`DynProperty`). The owned span visitor has no message
buffer, skips `log.*` fields, and on record replaces an existing entry with the same name rather
than appending (`FieldFormatVisitor` never dedupes). Because values must outlive
`on_new_span` until `on_close`, the extension owns the data as `Vec<(&'static str, String)>`
(allocated per span; acceptable since span creation is far less frequent than events and the
thread-local scratch cannot be held across callbacks). Factor the visitor so the event path keeps
its zero-alloc scratch behaviour and the span path uses an owned variant.

### Re-entrancy

Sinks may emit `tracing` spans/events while capturing. Callbacks must not hold the registry
extension lock across dispatch calls and must tolerate the thread-local being unavailable, as
`on_event` does. Take the data out of the extension, release the borrow, then dispatch.

### Opt-out

Add `capture_spans: bool` to `TracingCaptureLayer` (default `true` in `install_tracing_interop`)
so deployments that only want logs are unaffected.

## Implementation Steps

1. `rust/telemetry-sink/src/tracing_interop.rs`: add `capture_spans` field, `CapturedSpan`
   extension type, the callsite→`SpanLocation` registry, an owned field visitor for spans, and
   `on_new_span` / `on_record` / `on_close`. Require `S: for<'a> LookupSpan<'a>` on the `Layer`
   impl.
2. Update `install_tracing_interop` and every construction site of `TracingCaptureLayer` for the new
   field: `rust/telemetry-sink/tests/tracing_interop_tests.rs` (struct literals at lines 55, 91,
   180) and `rust/telemetry-sink/tests/tracing_interop_reentrancy_tests.rs:88`.
3. Add tests (below).
4. Docs and `CHANGELOG.md`.

## Files to Modify

- `rust/telemetry-sink/src/tracing_interop.rs`
- `rust/telemetry-sink/tests/tracing_interop_tests.rs`
- `rust/telemetry-sink/tests/tracing_interop_reentrancy_tests.rs`
- `rust/telemetry-sink/Cargo.toml` (only if `tracing-subscriber` needs the `registry` feature)
- `mkdocs/docs/when-to-use/index.md` (see Documentation)
- `CHANGELOG.md`

## Decisions

- Public `TracingCaptureLayer` gains a field (Rust API break, acceptable per the project's
  interface-stability rule); record it in `CHANGELOG.md` as a **Minor breaking change**. No SQL
  surface changes.

## Documentation

Update `mkdocs/docs/when-to-use/index.md`: the lines at ~13 and ~252 say interop "captures existing
`tracing` events as logs" and become stale; revise them and add a short passage there (the
`native/index.md` page covers only the C ABI, so it is not the destination) describing: spans become async spans, parent/child
and depth, fields as end-event properties, `capture_spans` opt-out. Add `CHANGELOG.md` entry.

## Testing Strategy

`tracing_interop_tests.rs` currently has only a log-block helper. Add a thread-block helper
(modeled on `rust/tracing/tests/span_properties_tests.rs:215-245`) matching
`BeginAsyncDynNamedSpanEvent`, `EndAsyncDynNamedSpanEvent` and `TaggedEndAsyncNamedSpanEvent`.
Thread blocks reach the sink only after `init_thread_stream()` and a flush; the cross-thread close
test must do both on each thread.

All no-DB unit tests in `rust/telemetry-sink/tests/tracing_interop_tests.rs` using
`init_in_memory_tracing` and `tracing::subscriber::with_default`, inspecting the captured thread
blocks for async begin/end events:

- A single span yields one begin and one end with matching `span_id`, `parent_span_id = 0`,
  `depth = 0`.
- Nested spans: child's `parent_span_id` equals parent's `span_id`, `depth` increments, end events
  reference the same ids.
- Fields at creation and via `on_record` appear as end-event properties; later record overrides.
- A level-filtered parent: a contextual child (created while an enabled ancestor is current)
  attaches to the nearest captured ancestor, or becomes root (`parent_span_id = 0`, `depth = 0`)
  if none.
- `capture_spans = false` emits nothing.
- Span closed on a different thread than opened still produces a matched end event.
- Re-entrancy: a sink that creates a span during capture does not panic or deadlock (added to
  `tracing_interop_reentrancy_tests.rs`, a separate binary with its own `ReentrantSink`,
  `set_global_default` and `init_event_dispatch`).
- Two spans from the same callsite record the same `SpanLocation` address in the block (events
  store `SpanLocation` only as an address).

## Manual Verification

1. Start services (`python3 local_test_env/ai_scripts/start_services.py`), run an example
   instrumented with nested `tracing::info_span!`s using `install_tracing_interop`.
2. `micromegas-query "SELECT * FROM process_spans('<process-id>', 'async') ORDER BY begin"`
   → expect rows with the nested names and increasing `depth`. Not automated because it needs the
   full ingestion/analytics stack; the capture logic itself is unit-tested above.
