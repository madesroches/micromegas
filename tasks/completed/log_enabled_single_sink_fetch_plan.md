# Check log_enabled and Fetch the Sink Once per Log Call Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1667

## Overview

An enabled log call with dynamic properties checks `log_enabled` twice and fetches the event sink
three times. Each fetch takes the sink `RwLock` read lock and clones its `Arc`. Both are atomic
read-modify-writes on cache lines that every logging thread shares. This plan changes every macro log
path so that it checks `on_log_enabled` once and calls `get_sink()` once. That cuts per-call
overhead and cross-thread cache-line traffic. The wire format, the SQL surface and the
`EventSink` trait don't change.

## Current State

`Dispatch::get_sink` (`rust/tracing/src/dispatch.rs:528`) takes `self.sink.try_read()` and
clones the `Arc<dyn EventSink>`. If `try_read` fails, it allocates an `Arc<NullEventSink>`.

Per-call sink fetches in `rust/tracing/src/dispatch.rs`:

| Path | Who calls it | `get_sink()` calls | `on_log_enabled` calls |
|---|---|---|---|
| `Dispatch::log` (`:827`) | `plain` macro arm | 2 (`log_enabled` + `on_log`) | 1 |
| `Dispatch::log_tagged` (`:854`) | `log_dyn_tagged` with an empty dynamic slice | 2 | 1 |
| `Dispatch::log_dyn_tagged` (`:908`), `nogate` arm (literals / `..set` only) | `nogate` macro arm | 2 (via `log_tagged` when the slice is empty, otherwise its own) | 1 |
| `Dispatch::log_dyn_tagged`, `gate` arm (dynamic values) | `gate` macro arm, after `dispatch::log_enabled` | **3** (macro gate + `log_enabled` + `on_log`) | **2** |
| `Dispatch::log_interop` / `log_interop_dyn_tagged` | `log`/`tracing` interop, capi | 1 | 0 (interop callers gate on level only, or not at all) |

The `gate` arm (`rust/tracing/src/macros.rs:557`) has to check `dispatch::log_enabled` before
it builds the dynamic slice, because evaluating a dynamic value can allocate. It then calls
`dispatch::log_dyn_tagged`, which checks again.

`rust/tracing/src/dispatch_wasm.rs` mirrors the public free functions. Its sink is a plain
`Arc` field with no lock, so it already makes a single access per call apart from the duplicated
gate check.

## Design

### 1. One sink fetch inside each `Dispatch` log method

In `Dispatch::log`, `log_tagged` and `log_dyn_tagged`, fetch the sink once at the top and use it
for both the enabled check and `on_log`. `Dispatch::log` does this inline, with no worker:

```rust
let sink = self.get_sink();
if !sink.on_log_enabled(desc) { return; }
let time = now();
sink.on_log(desc, ..., time, args);
```

`log_dyn_tagged` currently delegates to `log_tagged` when the dynamic slice is empty. Split the
body of `log_tagged` and `log_dyn_tagged` into a private `*_to_sink(&self, sink: &dyn EventSink, ...)`
worker that does no enabled check. Those methods then become "fetch, check, call worker". The
empty-slice delegation calls the `log_tagged` worker directly instead of re-entering the
checking method.

### 2. A pre-checked entry point for the `gate` macro arm

Add a hidden public token type and two free functions in `dispatch.rs`:

```rust
/// A sink that has accepted a log callsite; obtained from `enabled_log_sink`.
#[doc(hidden)]
pub struct EnabledLogSink {
    dispatch: &'static Dispatch,
    sink: Arc<dyn EventSink>,
}

#[doc(hidden)]
#[inline(always)]
pub fn enabled_log_sink(desc: &LogMetadata) -> Option<EnabledLogSink>;

#[doc(hidden)]
#[inline(always)]
pub fn log_dyn_tagged_enabled(
    sink: EnabledLogSink,
    desc: &'static LogMetadata,
    properties: &'static PropertySet,
    dyn_properties: &[DynProperty<'_>],
    args: fmt::Arguments<'_>,
);
```

`enabled_log_sink` gets the dispatch from `G_DISPATCH.get()`, fetches the sink once, returns `None`
when there is no dispatch or the sink rejects the callsite, and otherwise stores both the
`&'static Dispatch` and the `Arc` in the token. `log_dyn_tagged_enabled` calls
`token.dispatch.log_dyn_tagged_to_sink(&*token.sink, ...)`, since the worker is a `&self` method
that needs `self.log_stream` and `self.flush_log_buffer()`. Because the token can only be built by
`enabled_log_sink`, it is impossible to skip the check.

The `gate` arm becomes:

```rust
if $lvl <= STATIC_MAX_LEVEL && $lvl <= max_level() {
    if let Some(sink) = $crate::dispatch::enabled_log_sink(&LOG_DESC) {
        $crate::dispatch::log_dyn_tagged_enabled(sink, &LOG_DESC, $set, $slice, format_args!(...));
    }
}
```

The dynamic slice is still built only after the gate passes, so the lazy-evaluation guarantee
(`property_macro_tests.rs:611-619`) holds. An enabled gated call now does one `get_sink()` and
one `on_log_enabled`, down from three and two.

`dispatch_wasm.rs` gets a token type of the same name and two functions with the same signatures.
The wasm token holds `&'static WasmDispatch` and uses `d.sink` by reference, with no `Arc` clone.
Only the type name and signatures must match the native ones, not the layout, because `lib.rs`
compiles exactly one dispatch module per target. The macros compile unchanged on wasm.

The public `log_enabled` function and the `log_enabled!` macro stay as they are. The interop
paths already fetch the sink once and are not touched.

### Out of scope

- A lock-free, refcount-free sink read (`ArcSwap`, or an immutable sink plus an atomic shutdown
  flag) goes with the broader contention work (#1669). After this change there is exactly one
  `get_sink()` per call, so that later change only has a single site to replace.
- The `NullEventSink` allocation when `try_read` fails. That can only happen while `shutdown`
  holds the write lock, so it is not a hot-path cost.

## Implementation Steps

1. **Baseline benchmark** (on the branch, before any code change), from `rust/`:
   `cargo bench -p micromegas-tracing --bench properties -- --save-baseline before '^(log|contention)/'`.
   Record the `log/*` and `contention/*` timings.
2. `rust/tracing/src/dispatch.rs`:
   - Add the private `log_tagged_to_sink` and `log_dyn_tagged_to_sink` workers. Each
     takes `sink: &dyn EventSink`, does no enabled check, and has the current bodies with
     `self.get_sink()` replaced by `sink`.
   - Rewrite `Dispatch::log` to fetch the sink once inline, and `log_tagged` and `log_dyn_tagged`
     as "fetch once, check, call worker". Make the empty-slice branch call `log_tagged_to_sink`.
   - Add `EnabledLogSink` (holding `&'static Dispatch` and the sink `Arc`), `enabled_log_sink` and
     `log_dyn_tagged_enabled`.
3. `rust/tracing/src/dispatch_wasm.rs`: add the wasm token type (holding `&'static WasmDispatch`) and the two functions.
4. `rust/tracing/src/macros.rs`: switch the `gate` arm to `enabled_log_sink` /
   `log_dyn_tagged_enabled`.
5. Add the test from **Testing Strategy**.
6. **After benchmark**:
   `cargo bench -p micromegas-tracing --bench properties -- --baseline before '^(log|contention)/'`.
   Record the before/after table in the PR description. The cases to watch most are
   `log/dyn_low_card`, `log/dyn_high_card` and `log/dyn_mixed`, which use the gated path, and
   `contention/*`. `log/static`, `log/literals` and `log/set` should also improve slightly (2 → 1
   fetch).
7. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p micromegas-tracing`.
   Also run the wasm CI: `python3 ../build/rust_ci.py wasm`.

## Files to Modify

- `rust/tracing/src/dispatch.rs`
- `rust/tracing/src/dispatch_wasm.rs`
- `rust/tracing/src/macros.rs`
- `rust/tracing/tests/property_macro_tests.rs`
- `CHANGELOG.md` (handled by `/pr`)

## Trade-offs

- **Token type vs. a bare "unchecked" entry point.** A `log_dyn_tagged_unchecked` free function
  called after `log_enabled` would remove the second check, but it still fetches the sink twice,
  and any caller could skip the check by mistake. The token carries the already-fetched sink
  (one fetch) and can only come from a successful check.
- **Closure-based gate (`log_gated(&DESC, |sink| ...)`)** was rejected. The dynamic slice borrows
  temporaries built in the macro's scope, and moving that construction into a closure makes the
  macro expansion and the borrow lifetimes harder to follow, with no benefit over the
  `if let Some(sink)` form.
- **Changing `EventSink`** (e.g. merging `on_log_enabled` into `on_log`) would also remove a
  virtual call, but it breaks every sink implementation and the lazy-evaluation contract of the
  gate. Not needed for this issue.

## Documentation

No user-facing documentation changes. The new items are `#[doc(hidden)]` macro plumbing.

## Testing Strategy

Unit tests in `rust/tracing/tests/property_macro_tests.rs`, using its existing
`CapturingSink`/`DispatchGuard` harness:

- Add an `enabled_checks: usize` counter to `CapturedState`, incremented in
  `CapturingSink::on_log_enabled`, plus a `DispatchGuard::enabled_checks()` accessor.
- New `#[serial]` test `log_enabled_checked_once_per_call`. For each macro form, log once and assert that the
  `enabled_checks()` delta around the call is exactly 1 and the log was captured. The
  forms are plain (`info!("m")`), literals (`info!(a = "1", "m")`), `..set` only, `..set` plus a literal (`info!(a = "1", ..set, "m")`), a dynamic value
  (`info!(id = %i, "m")`), a literal mixed with a dynamic value, and `..set` plus a dynamic value.
  This pins the double check in the gated path (it is 2 today).
- The existing lazy-evaluation test (`:611-619`) keeps covering "a disabled gated call does not
  evaluate dynamic values".

The number of `get_sink()` calls can't be observed through the sink. It follows from the
structure (one fetch per entry point), and the benchmark confirms the effect.
