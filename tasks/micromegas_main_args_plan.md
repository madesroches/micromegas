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

Everything else is hardcoded in the expanded output:

```rust
// rust/micromegas-proc-macros/src/lib.rs:114-119
let mut builder_calls = vec![
    quote! { .with_ctrlc_handling() },
    quote! { .with_local_sink_max_level(LevelFilter::Debug) },
    quote! { .with_process_property("version"…) },
    quote! { .with_auth_from_env() },
];
```

`TelemetryGuardBuilder` (`rust/telemetry-sink/src/lib.rs`) already supports all the options
below but they are unreachable through the macro.

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

Build `builder_calls: Vec<TokenStream>` (current pattern), driven by the parsed values:

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

The `ApiKeyRequestDecorator` is referenced fully-qualified in generated code:
`micromegas::telemetry_sink::api_key_decorator::ApiKeyRequestDecorator`

`api_key_decorator` is already `pub mod` in `telemetry-sink/src/lib.rs` and reaches the
umbrella crate via `pub use micromegas_telemetry_sink::*` in `rust/public/src/lib.rs:127`.
No additional re-exports needed.

## Files to Modify

- `rust/micromegas-proc-macros/src/lib.rs` — all parsing and code-gen changes, plus
  updating the public rustdoc on `micromegas_main` (lines 24–51): add the 7 new attributes
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

- Before writing any tests:
  - Install `cargo-expand` if not already present: `cargo install --locked cargo-expand` (required by `macrotest` at runtime). A `cargo install --locked cargo-expand` step must be added to the `native` job in `.github/workflows/rust.yml`, following the same pattern as the existing `cargo-machete` install step — `--locked` alone is sufficient for reproducibility, and no explicit `--version` pin is needed — without it, `macrotest` tests crash on fresh GitHub-hosted runners.
  - Create `rust/micromegas-proc-macros/tests/` (the project convention in CLAUDE.md requires tests under the crate's `tests/` folder — inline `#[test]` in `src/lib.rs` is not allowed).
  - Add to `[dev-dependencies]` in `rust/micromegas-proc-macros/Cargo.toml` (none are present today; trybuild and macrotest compile fixture crates that resolve paths like `micromegas_telemetry_sink::api_key_decorator::…` and `tokio::runtime::Builder` against the host crate's dev-dependencies). List them alphabetically per the Cargo.toml convention:
    - `macrotest = "1"` — explicit version, since it is not in `[workspace.dependencies]` (matching the existing `wiremock = "0.6"` pattern in `public/Cargo.toml`).
    - `micromegas-telemetry-sink.workspace = true` — already in `[workspace.dependencies]`. Depend on `micromegas-telemetry-sink` directly (not the umbrella `micromegas` crate) to avoid the dev-dependency cycle `proc-macros → micromegas (dev) → proc-macros (normal)`, since `micromegas` depends on `micromegas-proc-macros` (`rust/public/Cargo.toml:46`). The fixtures' own imports therefore use `micromegas_telemetry_sink::…` rather than `micromegas::telemetry_sink::…`. Note this is independent of the macro's generated output, which must keep resolving against the end user's crate as `micromegas::telemetry_sink::…` (see the Code-generation section); only the test fixtures resolve against these dev-dependencies.
    - `micromegas-tracing = { workspace = true, features = ["tokio"] }` — already in `[workspace.dependencies]`. Required for trybuild fixtures to compile: the fixtures declare a `mod micromegas` preamble that re-exports `micromegas_tracing::runtime`, which `micromegas-telemetry-sink` does not re-export transitively.
    - `tokio = { workspace = true }` — already in `[workspace.dependencies]`.
    - `trybuild = "1"` — explicit version, same rationale as `macrotest`.
- Add a compile-test (using `trybuild`) covering:
  - Default (no args) — existing behaviour unchanged
  - Each bool flag flipped from its default
  - `local_sink_max_level = "info"`
  - `telemetry_url` set
  - `api_key` + `telemetry_url` together
- Add a `macrotest` expansion snapshot test for the `api_key` case: write a `.rs` fixture that sets `api_key`, then run `cargo test` once to let `macrotest::expand` generate the corresponding `.expanded.rs` snapshot file. After the snapshot is generated, inspect it (or use a separate `#[test]` that calls `std::fs::read_to_string` on the snapshot path) to assert that it contains `ApiKeyRequestDecorator` and does not contain `with_auth_from_env`. This is the correct `macrotest` workflow; `macrotest` has no API to assert on snapshot contents directly — it only compares the full expanded output against the saved file.
- Run `cargo test` in `rust/micromegas-proc-macros/` and in `rust/` (workspace) after the change.
- Run `cargo clippy --workspace -- -D warnings` and `cargo fmt --check`.

## Open Questions

None — design confirmed with user.
