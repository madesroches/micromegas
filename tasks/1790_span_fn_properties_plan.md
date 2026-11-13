# `#[span_fn]` Span Properties Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1790

## Overview

Let `#[span_fn]` attach properties to the span it creates, with values computed from the
function's arguments when the function is called:

```rust
#[span_fn(view = view.get_view_set_name())]
async fn materialize_partition(view: Arc<dyn View>, /* ... */) -> Result<()> { ... }
```

Today the only way to get a property on a whole-function span is to wrap the body in
`instrument_named!(async { ... }, "name", |_, span| span.set_property(..))`, which re-indents the
body and adds a nesting level. The attribute form keeps the body untouched. Functions that give no
properties expand exactly as today.

## Current State

- `rust/tracing/proc-macros/src/lib.rs:56` — `TraceArgs` parses an optional single `Literal`
  (`alternative_name`). `span_fn` (`:197`) has three expansions:
  1. **async-trait method** (returns `Pin<..>`, `:204`): wraps the `Box::pin(async move {..})`
     argument (or the whole body) in `InstrumentedFuture::new(.., &_SCOPE_DESC)`.
  2. **async fn** (`:244`): strips `async`, returns `impl Future`, wraps the body in
     `InstrumentedFuture::new(async move #body, &_SCOPE_DESC)`.
  3. **sync fn** (`:262`): inserts `span_scope!(_METADATA_FUNC, concat!(module_path!(), "::", name));`.

  The expansions use unqualified names (`static_span_desc!`, `InstrumentedFuture`, `span_scope!`)
  that resolve through `micromegas_tracing::prelude::*`.
- Span properties are recorded on the span's **end** event (`tasks/completed/1691_span_end_properties_plan.md`).
  - Sync: `span_scope!(name = <expr>, k = v, ..)` (`rust/tracing/src/macros.rs:64`) goes through the
    `__props!` muncher (`:863`) and `__span_guard_call!` (`:1067`), which builds a
    `SpanProperties` (`rust/tracing/src/spans/span_properties.rs`) and returns
    `ThreadSpanGuard::with_properties` (`rust/tracing/src/guards.rs:168`). `%v` becomes
    `set_property(key, &v)` (`__span_set_property!`, `macros.rs:1115`) — any `Display`, no
    allocation. Literal values are interned once per callsite; `name` is reserved as a key.
  - Async: `InstrumentedFuture<F, H>` (`rust/tracing/src/spans/instrumented_future.rs:245`) only
    takes properties through an end hook `H: SpanEndHook<F::Output>`; `NoHook` (`:127`) and the
    blanket `FnOnce(&O, &mut SpanProperties)` impl (`:136`) are the two hooks. On completion it
    emits `on_end_async_scope_tagged` when the hook returns `Some(properties)`.
- Materialization spans on `main` (`rust/analytics/src/lakehouse/batch_update.rs`):
  `materialize_partition` (`:141`), `materialize_partition_range` (`:233`),
  `regenerate_partition_range` (`:280`), `regenerate_partition` (`:330`) — all `#[span_fn]` async
  fns taking `view: Arc<dyn View>`. The finer-grained spans from #1788 live in PR #1789, which is
  still open and not on `main`.

## Design

### Attribute syntax

```
#[span_fn]                                  // unchanged
#[span_fn("alt_name")]                      // unchanged
#[span_fn(key = expr, ..)]                  // new
#[span_fn("alt_name", key = expr, ..)]      // new
```

- A key is an identifier or a string literal (`"http.method" = ..`), like the `log!`/`span_scope!`
  property lists. A leading string literal is the alternative name only when it is not followed
  by `=` (`input.peek2(Token![=])`).
- Each value is any expression whose type implements `Display`, evaluated once when the function
  is called (before the body runs), so it can only reference arguments and items in scope.
  It is borrowed, not moved, so referencing an argument does not consume it.
- A trailing comma is accepted.

### Expansion

The proc macro forwards each property as `key = %(expr)` to the existing `__props!` machinery, so
key handling (`name` reserved, string-literal keys, `__prop_key!`) is shared with `span_scope!`
rather than re-implemented. With no properties, every case expands exactly as today (zero cost).

- **sync fn**: insert
  `let __span_fn_guard = span_scope!(name = concat!(module_path!(), "::", #name), #(#props),*);`
  as the first statement. Binding to a named local keeps the guard alive to the end of the body.
- **async fn**: body becomes
  `span_fn_future!(concat!(..), async move #body, #(#props),*)`.
- **async-trait method**: the extracted `async move {..}` block (or `async move #body` fallback)
  goes through the same `span_fn_future!`, wrapped in `Box::pin(..)`. The properties are evaluated
  in the outer, synchronous body, where the method's arguments (including `self`) are in scope.

`span_fn_future!` is a new `#[doc(hidden)] #[macro_export]` declarative macro in
`rust/tracing/src/macros.rs`, re-exported from the prelude so the proc macro can name it
unqualified like the other names it emits:

```rust
macro_rules! span_fn_future {
    ($name:expr, $future:expr, $($props:tt)+) => {
        $crate::__props!(@m span [future $name, $future] [] [] [] [] [] $($props)+)
    };
}
```

and `__span_guard_call!` gains a `[future $name:expr, $future:expr]` context:

```rust
([future $name:expr, $future:expr] $set:expr, [$($d:tt)*]) => {{
    $crate::static_span_desc!(_METADATA_NAMED, $name);
    let mut properties = $crate::spans::SpanProperties::new($set);
    $crate::__span_set_properties!(properties $($d)*);
    $crate::spans::InstrumentedFuture::with_properties($future, &_METADATA_NAMED, properties)
}};
```

### `InstrumentedFuture::with_properties`

New constructor in `instrumented_future.rs`, with a new hook type that carries properties
captured at creation:

```rust
pub struct EntryProperties(SpanProperties);

impl<O> SpanEndHook<O> for EntryProperties {
    fn properties_at_end(self, _output: &O) -> Option<SpanProperties> {
        if self.0.is_empty() { None } else { Some(self.0) }
    }
}

impl<F> InstrumentedFuture<F, EntryProperties> {
    pub fn with_properties(future: F, desc: &'static SpanMetadata, properties: SpanProperties) -> Self {
        Self::with_hook(future, desc, EntryProperties(properties))
    }
}
```

This coexists with the blanket `FnOnce` impl the same way `NoHook` does. `SpanProperties` is
`Send` (`SmallVec` of `&'static str`/`u32`/`u8`), so the future stays `Send` when the body is,
which `#[async_trait]` requires.

### Applying it

Add `view = view.get_view_set_name()` to the four `#[span_fn]` materialization functions in
`batch_update.rs` (see Decisions for the #1789 spans).

## Implementation Steps

1. `rust/tracing/src/spans/instrumented_future.rs`: add `EntryProperties` and
   `InstrumentedFuture::with_properties`.
2. `rust/tracing/src/macros.rs`: add the `[future ..]` arm to `__span_guard_call!` and the
   `span_fn_future!` macro; add `span_fn_future` to the prelude in `rust/tracing/src/lib.rs`.
3. `rust/tracing/proc-macros/src/lib.rs`:
   - Extend `TraceArgs` with `properties: Vec<(PropKey, Expr)>` where `PropKey` is an `Ident` or
     `LitStr`; parse as described above.
   - Parse the leading alternative name as a `syn::LitStr` and use `.value()`. Today it is a
     `proc_macro2::Literal` whose `to_string()` keeps the quotes, so `#[span_fn("foo")]` yields the
     span name `module::"foo"`; this also strips those stray quotes from existing
     `#[span_fn("x")]` span names (a span rename, observable in data).
   - In each of the three cases, branch on `properties.is_empty()`: keep today's expansion when
     empty, emit the property forms otherwise.
   - Add a "Span Properties" section to the `span_fn` doc comment with an example (async and
     sync), and note values are evaluated at entry and recorded on the span's end event.
4. `rust/tracing/src/lib.rs`: add a short mention of the `#[span_fn(key = expr)]` form to the
   crate-level "# Span properties" section.
5. `rust/tracing/Cargo.toml`: add dev-dependency `async-trait.workspace = true`; then write the
   tests in `rust/tracing/tests/span_properties_tests.rs` (see Testing Strategy).
6. `rust/analytics/src/lakehouse/batch_update.rs`: add `view = view.get_view_set_name()` to the
   four materialization spans.
7. `CHANGELOG.md`: Unreleased entry under **Tracing**, including that `#[span_fn("x")]` span names
   no longer contain stray quote characters. No `mkdocs/` page documents `#[span_fn]`'s arguments.

## Files to Modify

- `rust/tracing/proc-macros/src/lib.rs`
- `rust/tracing/src/macros.rs`
- `rust/tracing/src/lib.rs`
- `rust/tracing/src/spans/instrumented_future.rs`
- `rust/tracing/Cargo.toml` (dev-dependency `async-trait.workspace = true`)
- `rust/tracing/tests/span_properties_tests.rs`
- `rust/analytics/src/lakehouse/batch_update.rs`
- `CHANGELOG.md`

## Decisions

- Property values always go through `%` (Display), so literal values are not interned.

- The #1788 spans (PR #1789) are not on `main`; this branch adds `view` to the four
  materialization spans already on `main`. Whichever of #1789 and this PR merges second adds
  `view` to the #1789 spans (`fetch_sql_partition_spec`, `make_batch_partition_spec`,
  `verify_overlapping_partitions`, `create_merged_partition`, `SqlPartitionSpec::write`) where
  the function has a view name in its arguments.

- Stripping the stray quotes from `#[span_fn("x")]` span names is accepted as a rename: the
  quotes are a bug and the only in-repo user is `rust/tracing/tests/test_macros.rs:103`.

## Testing Strategy

Unit tests in `rust/tracing/tests/span_properties_tests.rs`, using `init_in_memory_tracing` and
the existing `sorted`/`owned` helpers, `#[serial]`. `thread_ends` only matches thread-span ends
and returns no names, so the async and alt-name tests match `TaggedEndAsyncSpanEvent` /
`EndAsyncSpanEvent` (and the thread tagged end) directly, as
`instrument_with_emits_a_tagged_unnamed_end` does, reading the span name from the event's desc:

- **sync**: `#[span_fn(id = id, kind = "x")] fn f(id: u32)` → one `TaggedEndThreadSpanEvent` with
  `[("id", "7"), ("kind", "x")]`.
- **async fn**: `#[span_fn(view = name)] async fn g(name: &str) -> usize` run on a current-thread
  runtime → `TaggedEndAsyncSpanEvent` with `("view", ..)`, and the output is returned unchanged.
- **async-trait method**: a trait impl with `#[span_fn(n = self.n)]` → tagged async end with the
  property (proves `self` is reachable and the future is `Send`).
- **alternative name + properties**: `#[span_fn("alt", k = 1)]` → the span's name ends in `::alt`
  and carries `k`.
- **no properties**: extend `nothing_set_emits_the_plain_end_and_old_forms_are_unchanged`
  with calls to `#[span_fn]`, `#[span_fn("alt")]` and an async `#[span_fn]`, asserting plain
  `EndThreadSpanEvent` / `EndAsyncSpanEvent` ends. The async call needs a current-thread tokio
  runtime, and the end/begin count assertions are updated for the added calls.
- **argument not consumed**: a sync fn using a `String` argument as a property and then moving it
  in the body compiles (covered by writing the test that way).

## Manual Verification

Checks the daemon wiring end to end, which unit tests cannot reach:

1. `python3 local_test_env/ai_scripts/start_services.py`, let the maintenance daemon materialize
   for a few minutes.
2. Find the daemon process id: `micromegas-query "SELECT process_id FROM processes WHERE exe LIKE '%telemetry-maintenance-srv%' ORDER BY start_time DESC LIMIT 1"`.
3. `micromegas-query "SELECT name, property_get(properties, 'view') AS view, count(*) FROM process_spans('<daemon process id>', 'async') WHERE name LIKE '%partition%' GROUP BY 1, 2" --begin 1h`
   → rows carry the view set names (e.g. `log_entries`, `measures`).
