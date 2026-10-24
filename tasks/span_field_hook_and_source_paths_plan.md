# Span Field Hook and Relative Source Paths Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1725

## Overview

A host that captures `tracing` spans with `TracingCaptureLayer` cannot control what leaves the
process. This plan adds two things. The first is an optional hook on the layer that can drop or
rewrite each span field value before it is stored. The second is a trim of absolute `file!()`
paths, applied where `micromegas-tracing` writes source locations as block dependencies, so
registry, git and out-of-workspace crates no longer leak the user's home directory. With both, a
host can use the stock layer for spans instead of keeping its own span bridge.

## Current State

**Span fields.** `rust/telemetry-sink/src/tracing_interop.rs`:
- `TracingCaptureLayer` (line 98) has two public fields, `max_level` and `capture_spans`, and is
  built with a struct literal (`install_tracing_interop`, line 375; the tests in
  `telemetry-sink/tests/tracing_interop_tests.rs` and `tracing_interop_reentrancy_tests.rs`).
- `SpanFieldVisitor` (line 116) records every field except `log.*` into
  `Vec<(&'static str, String)>`, through `upsert`. `record_str` copies the value and
  `record_debug` formats it.
- `on_new_span` (line 165) records `attrs` into a fresh vec and `on_record` (line 214) records
  into a scratch vec outside the extensions lock, then upserts it into `CapturedSpan.fields`.
  `on_close` turns the fields into `DynProperty` end-event properties.
- Both callbacks can reach the callsite's `&'static Metadata<'static>` (`attrs.metadata()`,
  `span.metadata()`).

**Source paths.** The three `ExtractDeps` implementations in `rust/tracing/src/` write a
location's `file` twice: as a `Utf8StaticStringDependency` keyed by pointer in `recorded_deps`,
and as the `file` pointer of the metadata record.
- `spans/block.rs`: `record_scope_event_dependencies` (line 83 and line 92) and
  `record_span_location_dependencies` (line 114 and line 122).
- `logs/block.rs`: `record_log_event_dependencies` (line 58 and line 68).
- `metrics/block.rs`: `record_metric_event_dependencies` (line 52 and line 61).

The `file` value is the raw `file!()` (or `tracing::Metadata::file()` for captured spans, which
`span_location` copies into a leaked `SpanLocation`). The analytics side resolves the record's
`file` pointer against the string dependencies, so the two only need to agree.

## Design

### 1. Span field hook

New public type and field in `tracing_interop.rs`:

```rust
/// Decides what is stored for one span field value: `None` drops the field, `Some` keeps the
/// returned value.
pub type SpanFieldHook = Box<
    dyn for<'v> Fn(&'static tracing::Metadata<'static>, &str, &'v str) -> Option<Cow<'v, str>>
        + Send
        + Sync,
>;

pub struct TracingCaptureLayer {
    pub max_level: LevelFilter,
    pub capture_spans: bool,
    /// Called for each span field value before it is stored.
    pub span_field_hook: Option<SpanFieldHook>,
}
```

Arguments are the callsite metadata, the field name and the value as text (the `Debug` form for
non-string values, exactly what is stored today).

`SpanFieldVisitor` gains `metadata: &'static Metadata<'static>` and `hook: Option<&SpanFieldHook>`.
Its `set` path becomes, after the existing `log.*` skip:
- no hook: store as today;
- `None`: remove any entry already stored under that name, store nothing;
- `Some(Cow::Borrowed(s))` where `s` is the whole input: keep the value without copying again;
- any other `Some`: store the returned text.

`record_str` calls the hook on the borrowed `&str` before copying, so a dropped value costs no
allocation. `record_debug` formats first, as today, then calls the hook.

Because a later `Span::record` must be able to drop a field, the visitor needs to express removal.
The scratch vec in `on_record` therefore becomes `Vec<(&'static str, Option<String>)>`, and the
merge loop removes the entry on `None` and upserts on `Some`. In `on_new_span` the visitor writes
straight into the field list, where removing an entry and never adding one have the same result.

The hook runs exactly where `Debug` formatting already runs: inside the visitor, outside the
extensions lock. So a hook that itself emits `tracing` events or spans is no worse than a `Debug`
impl that does, which the reentrancy tests already cover.

`install_tracing_interop` sets `span_field_hook: None`. A host that wants the hook builds the
layer itself, as it already must to set `capture_spans: false`.

### 2. Relative source paths

New module `rust/tracing/src/source_path.rs` (`pub mod source_path;` in `lib.rs`; it is pure code,
so it is not gated on `wasm32`):

```rust
/// Returns the part of a `file!()` path worth sending: a suffix of `path`, so no allocation.
pub fn trim_source_path(path: &str) -> &str
```

Rules. A path counts as absolute when it starts with `/` or `\`, or with a drive letter followed
by `:` and a separator. Both `/` and `\` count as separators, because the path comes from the
build host, which may differ from the target.
- relative (workspace members, empty string): unchanged.
- absolute and containing a `registry<sep>src<sep>` component sequence (first match, which must
  start at a component boundary): drop everything through the next component (the index
  directory, e.g. `index.crates.io-<hash>`) →
  `tokio-1.40.0/src/runtime/task.rs`.
- absolute and containing `git<sep>checkouts<sep>`: drop everything through it →
  `<repo>-<hash>/<rev>/src/lib.rs`. The repo name is kept, and so are the checkout hash and rev
  directories, because a suffix cannot skip them (see Trade-offs).
- any other absolute path: the file name after the last separator.
- a marker with nothing usable after it falls through to the file-name rule.

Applying it: in each of the four record functions, build the dependency from the trimmed slice and
use the dependency's pointer for the record, so the two agree by construction:

```rust
let file = Utf8StaticStringDependency::from(trim_source_path(log_desc.file));
if recorded_deps.insert(file.ptr as u64) { deps.push(file); }
// ...
file: file.ptr,
```

`trim_source_path` on `&'static str` returns `&'static str` through lifetime elision, so the
pointer stays valid for the process lifetime. Records are already deduplicated per block through
`recorded_deps`, so the scan runs once per callsite per block.

The `file` column keeps its type, so this is not a SQL break. Existing data is unchanged; only new
blocks carry trimmed paths.

## Implementation Steps

1. **Path trim.** Create `rust/tracing/src/source_path.rs` with `trim_source_path` and register it
   in `rust/tracing/src/lib.rs`.
2. Use it in `rust/tracing/src/spans/block.rs` (both record functions),
   `rust/tracing/src/logs/block.rs` and `rust/tracing/src/metrics/block.rs`, taking the record's
   `file` from the dependency's pointer.
3. Add `rust/tracing/tests/source_path_tests.rs` and the extraction tests (see Testing Strategy).
4. **Field hook.** In `rust/telemetry-sink/src/tracing_interop.rs`, add `SpanFieldHook` and the
   `span_field_hook` field, extend `SpanFieldVisitor` with the metadata and hook, switch the
   `on_record` scratch vec to `Option<String>` values, and apply removal in both merge paths.
5. Set `span_field_hook: None` in `install_tracing_interop` and in every struct literal in
   `telemetry-sink/tests/tracing_interop_tests.rs` and `tracing_interop_reentrancy_tests.rs` (the
   compiler lists them).
6. Add the hook tests to `telemetry-sink/tests/tracing_interop_tests.rs`.
7. Update docs and `CHANGELOG.md`.

## Files to Modify

- `rust/tracing/src/source_path.rs` (new)
- `rust/tracing/src/lib.rs`
- `rust/tracing/src/spans/block.rs`
- `rust/tracing/src/logs/block.rs`
- `rust/tracing/src/metrics/block.rs`
- `rust/tracing/tests/source_path_tests.rs` (new)
- `rust/telemetry-sink/src/tracing_interop.rs`
- `rust/telemetry-sink/tests/tracing_interop_tests.rs`
- `rust/telemetry-sink/tests/tracing_interop_reentrancy_tests.rs`
- `mkdocs/docs/when-to-use/index.md`
- `CHANGELOG.md`

## Trade-offs

- **Hook scope: span fields only.** The issue's gap is the span path (`on_new_span` /
  `on_record`). Event fields are also rendered into the message text, so a hook there needs its
  own design (rewrite the message too, or not). That is left out to keep this change to what the
  issue asks; the field is named `span_field_hook` so that a later event hook does not change its
  meaning.
- **`Option<Box<dyn Fn>>` field vs a builder.** The layer is built with struct literals
  everywhere. Adding a field breaks each literal at compile time, which is the preferred way to
  surface a Rust API change, and avoids adding a builder for one optional value. `Box` is enough
  because the layer is not cloned.
- **`Cow` return.** It lets keep and truncate (a prefix of the input) avoid an allocation, while a
  rewrite can still return owned text.
- **Git checkout paths keep `<repo>-<hash>/<rev>/`.** The issue asks to strip through `<rev>/` and
  keep the repo name. A suffix cannot do both, and allocating (or leaking) a rebuilt string per
  callsite would break the no-allocation, `&'static`-pointer property that the dependency scheme
  relies on. The checkout hash and rev are derived from the URL and commit, not from the user's
  machine, so keeping them leaks nothing personal.
- **Trim at block extraction, not in the macros.** One place per stream covers the micromegas
  macros, `tracing` interop spans and any future source, which is what the issue asks for, and the
  callsite statics stay untouched.
- **Always on, no option.** As the issue asks; nobody needs the absolute home path in telemetry.

## Decisions

## Documentation

- `mkdocs/docs/when-to-use/index.md` (the `tracing` interop paragraph, line 252): one sentence
  saying that `TracingCaptureLayer::span_field_hook` can drop or rewrite span field values before
  they are stored, and that source file paths of crates outside the workspace are trimmed to a
  relative form.
- `CHANGELOG.md` Unreleased: an entry for both changes, with **Minor breaking change:**
  `TracingCaptureLayer` gains a public `span_field_hook` field.

## Testing Strategy

All of it is unit-level, with no DB or service.

`rust/tracing/tests/source_path_tests.rs`, table-driven over `trim_source_path`:
- Unix registry path → `tokio-1.40.0/src/runtime/task.rs`; old-style `github.com-<hash>` index
  dir → same shape.
- Windows registry path (`C:\Users\u\.cargo\registry\src\index.crates.io-x\tokio-1.40.0\src\a.rs`)
  → `tokio-1.40.0\src\a.rs`.
- Git checkout → `micromegas-1a2b/5e6f7a8/rust/tracing/src/lib.rs`.
- Other absolute paths (`/home/u/dep/src/lib.rs`, `D:\dep\src\lib.rs`) → `lib.rs`.
- Relative paths, including a relative path containing `registry/src/`, and the empty string →
  unchanged.
- A component that only ends with the marker (`/x/myregistry/src/...`) is not treated as the
  registry.
- The result is a subslice: its end pointer equals the input's end pointer.

In the same file, one extraction test per stream: build a `LogMsgQueue`, `MetricsMsgQueue` and
`ThreadEventQueue` holding one event whose static metadata has an absolute registry `file`, call
`extract`, and assert that the metadata record's `file` pointer equals the trimmed slice's pointer
and that a `Utf8StaticStringDependency` with that pointer and the trimmed length was emitted. This
pins the pointer agreement at the four call sites, which a mismatch would break silently: the
analytics side would fail to resolve the string.

`rust/telemetry-sink/tests/tracing_interop_tests.rs`, reusing `captured_async_spans`:
- A hook that drops `prompt`, truncates `args` to 4 characters (borrowed) and uppercases `model`
  (owned): the end properties show `prompt` absent, `args` truncated and `model` rewritten. The
  hook also asserts it received the span's metadata (name `work`).
- A later `span.record` of a dropped field removes a value stored at creation, and a later
  `record` of a kept value restores the field.
- `span_field_hook: None` leaves the existing field tests unchanged (covered by the existing
  tests once they set the field).

## Open Questions

None.
