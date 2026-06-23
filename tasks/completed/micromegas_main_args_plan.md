# `#[micromegas_main]` Optional Arguments Plan

## Overview

Extend the `#[micromegas_main]` proc macro to expose the full set of commonly needed
`TelemetryGuardBuilder` options as attribute parameters, so callers can configure telemetry
inline without falling back to a manual builder.

## Current State

*(Pre-implementation baseline — the changes described in this plan have been applied.)*

The macro lives in `rust/micromegas-proc-macros/src/lib.rs` and accepts two parameters today:

- `interop_max_level = "…"` → `with_interop_max_level_override()`
- `max_level_override = "…"` → `with_max_level_override()`

Everything else was hardcoded in the expanded output *(snippet from pre-implementation state)*:

```rust
// rust/micromegas-proc-macros/src/lib.rs:114-119 (pre-implementation)
let mut builder_calls = vec![
    quote! { .with_ctrlc_handling() },
    quote! { .with_local_sink_max_level(LevelFilter::Debug) },
    quote! { .with_process_property("version"…) },
    quote! { .with_auth_from_env() },
];
```

`TelemetryGuardBuilder` (`rust/telemetry-sink/src/lib.rs`) already supports all the options
below but they were unreachable through the macro.

## Design

### New parameters

| Parameter | Rust type | Default | Builder call |
|---|---|---|---|
| `ctrlc_handling` | `bool` | `true` | `with_ctrlc_handling()` (conditionally) |
| `local_sink_enabled` | `bool` | `true` | `with_local_sink_enabled(false)` |
| `local_sink_max_level` | level string | `"debug"` (macro override; builder default is `Info`) | `with_local_sink_max_level(…)` |
| `install_log_capture` | `bool` | `false` | `with_install_log_capture(true)` |
| `system_metrics` | `bool` | `true` | `with_system_metrics_enabled(false)` |
| `telemetry_url` | string | — | `with_telemetry_sink_url(…)` |
| `api_key` | string | — | `with_request_decorator(…)` |

The two existing parameters (`interop_max_level`, `max_level_override`) are unchanged.

### `api_key` precedence

When `api_key` is provided as an attribute argument it must win over the env-var lookup.
Implementation: emit `with_request_decorator(…)` **instead of** `with_auth_from_env()`.
When `api_key` is absent, keep `with_auth_from_env()` as today.

### Parsing approach

The macro already parses `AttributeArgs` with a `match` on `NestedMeta`. Extend the same
loop with new arms:

- `Lit::Bool` for the four bool parameters
- `Lit::Str` for `local_sink_max_level`, `telemetry_url`, and `api_key`

The error message in the catch-all `_ => panic!(…)` must be updated to list all supported
parameters.

### Code-generation approach

Build `builder_calls: Vec<TokenStream>` (current pattern), driven by the parsed values. The
vec is seeded with `with_process_property("version"…)` (unconditional, preserved from today)
*before* the calls below:

1. `with_ctrlc_handling()` — emit only when `ctrlc_handling != false`
2. `with_local_sink_enabled(false)` — emit only when `local_sink_enabled == false`
3. `with_local_sink_max_level(…)` — always emit (default `LevelFilter::Debug`); note this is an intentional macro-level override of `TelemetryGuardBuilder::default()`'s `LevelFilter::Info`, preserving the current hardcoded behavior rather than silently changing it
4. `with_install_log_capture(true)` — emit only when `install_log_capture == true`
5. `with_system_metrics_enabled(false)` — emit only when `system_metrics == false`
6. `with_telemetry_sink_url(…)` — emit when `telemetry_url` is set
7. Auth — when `api_key` is set, emit:
   ```
   .with_request_decorator(std::boxed::Box::new(move || std::sync::Arc::new(micromegas::telemetry_sink::api_key_decorator::ApiKeyRequestDecorator::new(#api_key.to_string()))))
   ```
   The token stream must fully-qualify `std::sync::Arc` (and `std::boxed::Box`): the macro
   expands into the user's `main()` body and emits no `use` statements, and `Arc` is not in
   the std prelude (nor re-exported by the tracing prelude), so a bare `Arc::new(…)` would
   fail to resolve. The macro is native-only — `TelemetryGuardBuilder` lives entirely inside
   `#[cfg(not(target_arch = "wasm32"))] mod native` in `telemetry-sink/src/lib.rs`, so the
   whole emitted chain (not just the `api_key` branch) only compiles on non-wasm targets.
   Full wasm support for the macro is out of scope; no per-branch cfg gating is needed.
   When `api_key` is absent, emit `with_auth_from_env()` unconditionally (as today).
8. After the auth branch, the chain ends with the existing unconditional
   `with_max_level_override(…)` and `with_interop_max_level_override(…)` calls driven by the
   two pre-existing parameters.

The `ApiKeyRequestDecorator` is referenced fully-qualified in generated code:
`micromegas::telemetry_sink::api_key_decorator::ApiKeyRequestDecorator`

`api_key_decorator` is already `pub mod` in `telemetry-sink/src/lib.rs` and reaches the
umbrella crate via `pub use micromegas_telemetry_sink::*` in `rust/public/src/lib.rs:127`.
No additional re-exports needed.

## Files to Modify

- `rust/micromegas-proc-macros/src/lib.rs` — all parsing and code-gen changes, plus
  updating the public rustdoc on `micromegas_main` (lines 9–64): add the 7 new attributes
  to the `# Parameters` section (with type/default) and extend the `# Examples` block to
  demonstrate at least one new parameter (e.g., `telemetry_url`/`api_key`). *(done)*

## Trade-offs

- **All-in-one attribute vs. separate config struct**: a config struct would be more ergonomic
  for many parameters but requires stabilising a public type in a proc-macro crate, which adds
  API surface. Attribute key-value pairs keep the macro self-contained and match the
  `#[tokio::main]` precedent users already know.
- **`api_key` in source code**: hardcoding a secret in source is acceptable for some
  distribution scenarios (embedded keys, internal tools). The parameter name is deliberately
  `api_key`, not `api_key_env`, to make it clear it is a literal value.

## Testing Strategy

*(Implemented — see `rust/micromegas-proc-macros/src/lib.rs`.)*

Tests use inline `#[cfg(test)]` in `src/lib.rs` rather than a `tests/` folder or external test harnesses.
Proc-macro crates only export proc-macro items; integration tests in `tests/` cannot call internal helper
functions, so inline tests are the only way to unit-test token generation without spawning cargo.

The core logic is extracted into `fn expand_micromegas_main(args: TokenStream, input: TokenStream) -> TokenStream`
(using `proc_macro2` types), which is directly callable from the inline test module via `use super::*`.
The public `#[proc_macro_attribute]` entry point converts `proc_macro::TokenStream` ↔ `proc_macro2::TokenStream`
and delegates. No dev-dependencies are needed.

Tests (all run in-process, sub-millisecond):
- `default_produces_standard_calls` — `with_auth_from_env`, `with_ctrlc_handling`, `with_local_sink_max_level` present
- `api_key_replaces_env_auth` — `ApiKeyRequestDecorator` present, `with_auth_from_env` absent
- `ctrlc_handling_false_omits_call` — `with_ctrlc_handling` absent
- `telemetry_url_emits_call` — `with_telemetry_sink_url` present
- `local_sink_disabled_emits_call` — `with_local_sink_enabled` present
- `system_metrics_false_emits_call` — `with_system_metrics_enabled` present
- `bad_ctrlc_type_panics` — `#[should_panic(expected = "ctrlc_handling must be a bool literal")]`
- `unknown_arg_panics` — `#[should_panic(expected = "Unsupported attribute argument")]`

The `cargo-expand` CI step and all trybuild/macrotest dev-dependencies have been removed as they are no longer needed.

## Open Questions

None — design confirmed with user.
