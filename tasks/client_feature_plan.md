# Lightweight `client` Feature for the `micromegas` Crate Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1662

## Overview

Applications that only *query* a micromegas deployment must enable the `server` feature of the
`micromegas` crate to get `micromegas::client::flightsql_client::Client`, which drags in the whole
service stack (DataFusion, sqlx, object_store, the ingestion/auth/object-cache/perfetto crates,
foyer, ...). This plan adds a `client` feature, implied by `server`, that exposes only the FlightSQL
client and its factory on top of Arrow + Flight + tonic, and adds a CI check so a server-only import
cannot creep back into the client modules.

## Current State

- `rust/public/Cargo.toml` has two feature states: none (instrumentation only) and `server`
  (everything). All of `anyhow`, `arrow-flight`, `async-trait`, `futures`, `http`, `tonic` are
  optional deps enabled only by `server`.
- `rust/public/src/lib.rs:188` gates `pub mod client` on `server`; the re-exports `arrow_flight` and
  `tonic` (`lib.rs:126-139`) are also `server`-only.
- `rust/public/src/client/mod.rs` declares five modules:
  - `flightsql_client` — uses `arrow_flight`, `tonic`, `futures`, `anyhow`,
    `datafusion::arrow::{RecordBatch, SchemaRef}`, and `micromegas_analytics::time::TimeRange`.
  - `flightsql_client_factory` — uses `tonic`, `http`, `async_trait`, `anyhow`.
  - `frame_budget_reporting`, `perfetto_trace_client`, `query_processes` — depend on
    `micromegas-analytics` helpers and/or the DataFusion engine.
- `TimeRange` (`rust/analytics/src/time.rs:12-23`) is just two `DateTime<Utc>` fields and a `new`
  constructor, with `Clone, Copy, Debug, PartialEq, Eq, Hash` derives. Its only dependency is
  `chrono`, but it lives in `micromegas-analytics`, which pulls in DataFusion, sqlx and the lakehouse.
- `micromegas-telemetry` (`rust/telemetry/`) already depends on `chrono` unconditionally and is an
  always-on dependency of `micromegas`, so it is a free home for `TimeRange`.
- There is no `arrow` workspace dependency; arrow types reach the client via the `datafusion`
  re-export. `rust/datafusion-wasm/Cargo.toml:19` pins `arrow = { version = "59.2", ... }`, and
  `build/check_wasm_deps.py` requires a workspace entry for a shared dep to use the same version
  string.
- Precedent: `tasks/completed/telemetry_server_feature_flag_plan.md` (issue #855) introduced the
  `server` feature in the same way.

### Dependency facts that shape the acceptance criteria

- `arrow-flight` 59.3 depends on `tonic` with features `["transport", "codegen", "router"]`, and
  tonic's `router` feature pulls `axum`. So `axum` is in the tree of **any** Flight client and cannot
  be excluded by this crate.
- `cargo tree -p micromegas --no-default-features` already contains `tower-http` today (through the
  telemetry sink's HTTP stack), so it is also not excludable.

The CI check therefore forbids the crates that matter and that this crate controls: `datafusion`,
`sqlx`, `object_store`, `foyer`, `micromegas-analytics`, `micromegas-ingestion`, `micromegas-auth`,
`micromegas-object-cache`, `micromegas-otel-ingestion`, `micromegas-perfetto`. `axum` and
`tower-http` are left out of the forbidden list on purpose (see Decisions).

## Design

### 1. Move `TimeRange` to `micromegas-telemetry`

- New file `rust/telemetry/src/time_range.rs` containing the `TimeRange` struct and its `new`
  constructor, unchanged (same derives, same public fields).
- `rust/telemetry/src/lib.rs`: add `pub mod time_range;` (not feature-gated).
- `rust/analytics/src/time.rs`: replace the definition with
  `pub use micromegas_telemetry::time_range::TimeRange;`. Every existing path
  (`micromegas_analytics::time::TimeRange`, `micromegas::analytics::time::TimeRange`) keeps working
  and names the same type, so the client accepts values built by server code.
- Client-only callers name it as `micromegas::telemetry::time_range::TimeRange` (the `telemetry`
  module already re-exports `micromegas_telemetry::*`).

### 2. Add an `arrow` workspace dependency

`rust/Cargo.toml`: `arrow = { version = "59.2", default-features = false }` (alphabetical, before
`arrow-flight`). The version string matches `datafusion-wasm` so `check_wasm_deps.py` stays green,
and it resolves to the same `arrow` crate DataFusion uses, so `arrow::RecordBatch` and
`datafusion::arrow::RecordBatch` stay the same type in server builds. The client only needs
`arrow-array`/`arrow-schema`, both always on, so no default features are needed.

### 3. `client` feature in `rust/public/Cargo.toml`

```toml
client = [
    "dep:anyhow",
    "dep:arrow",
    "dep:arrow-flight",
    "dep:async-trait",
    "dep:futures",
    "dep:http",
    "dep:tonic",
]
server = [
    "client",
    "dep:async-stream",
    ...  # existing list, minus the entries now covered by "client"
]
```

Add `arrow = { workspace = true, optional = true }` to the optional deps. Group the comments as
"Client (optional)" and "Server-only (optional)" sections; within each, keep the alphabetical order.

### 4. Module gating

`rust/public/src/lib.rs`:

```rust
/// re-exports (client)
#[cfg(feature = "client")]
pub use arrow;
#[cfg(feature = "client")]
pub use arrow_flight;
#[cfg(feature = "client")]
pub use tonic;

/// re-exports (server-only)
#[cfg(feature = "server")]
pub use axum;
... (datafusion, object_store, prost, sqlx unchanged)

#[cfg(feature = "client")]
pub mod client;
```

`rust/public/src/client/mod.rs`: `flightsql_client` and `flightsql_client_factory` are
unconditional within the module; `frame_budget_reporting`, `perfetto_trace_client` (and its
`SpanTypes` re-export) and `query_processes` get `#[cfg(feature = "server")]`. No separate
`client-analytics` feature: no caller has asked for those helpers without the server stack.

`rust/public/src/client/flightsql_client.rs`: import `arrow::{array::RecordBatch,
datatypes::SchemaRef}` and `micromegas_telemetry::time_range::TimeRange` instead of the
`datafusion` / `micromegas_analytics` paths. No signature changes.

### 5. CI guard

Two new sequential steps in `run_native()` in `build/rust_ci.py`, after Clippy Linting:

1. `("Client Feature Clippy", "cargo clippy -p micromegas --no-default-features --features client -- -D warnings", None)`
   — compiles the client modules alone, so any server-only import fails the build; `-D warnings`
   also catches imports that become unused only under `client`.
2. `("Client Feature Dependency Check", "python3 build/check_client_deps.py", repo_root)` — runs
   `cargo tree -p micromegas --no-default-features --features client -e normal --prefix none` (cwd
   `rust/`) and fails if any forbidden crate name appears.

`build/check_client_deps.py` follows `check_wasm_deps.py`'s shape: a `FORBIDDEN_CRATES` tuple, a
pure function `find_forbidden(tree_output: str) -> list[str]` that parses the crate name (first token
of each line) and returns the sorted forbidden names present, and a `main()` that runs `cargo tree`
via `subprocess.run(..., check=True, capture_output=True, text=True)` and exits 1 with the list if
it is non-empty.

## Implementation Steps

1. Create `rust/telemetry/src/time_range.rs`; add `pub mod time_range;` to
   `rust/telemetry/src/lib.rs`; replace the definition in `rust/analytics/src/time.rs` with the
   re-export.
2. Add the `arrow` workspace dep in `rust/Cargo.toml`.
3. Edit `rust/public/Cargo.toml`: add the `arrow` optional dep, the `client` feature, make `server`
   imply `client` and drop the duplicated entries. Change the `flightsql_client_factory_scheme_tests`
   `required-features` to `["client"]`.
4. Edit `rust/public/src/lib.rs` (re-exports and `pub mod client` gating) and
   `rust/public/src/client/mod.rs` (gate the three analytics-dependent modules on `server`).
5. Update imports in `rust/public/src/client/flightsql_client.rs`.
6. Add `build/check_client_deps.py` and `build/test_check_client_deps.py`; add the two steps to
   `build/rust_ci.py`.
7. `.github/workflows/rust.yml`: add `build/check_client_deps.py` and
   `build/test_check_client_deps.py` to both `paths` filters, and run the new test file in the
   existing pytest step (`python3 -m pytest build/test_rust_ci.py build/test_check_client_deps.py`).
8. Docs: add a "Cargo features" section to the crate docs in `rust/public/src/lib.rs` (see
   Documentation).
9. From `rust/`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, the two new commands, and
   `cargo machete`.

## Files to Modify

- `rust/Cargo.toml`
- `rust/telemetry/src/lib.rs`, `rust/telemetry/src/time_range.rs` (new)
- `rust/analytics/src/time.rs`
- `rust/public/Cargo.toml`
- `rust/public/src/lib.rs`
- `rust/public/src/client/mod.rs`
- `rust/public/src/client/flightsql_client.rs`
- `build/rust_ci.py`
- `build/check_client_deps.py` (new), `build/test_check_client_deps.py` (new)
- `.github/workflows/rust.yml`
- `CHANGELOG.md` (handled by `/pr`)

## Decisions

- `axum` and `tower-http` are not in the forbidden list: `axum` arrives through `arrow-flight` →
  `tonic/router`, and `tower-http` is already in the featureless tree via the telemetry sink. The
  issue's acceptance list is adjusted accordingly.
- No `client-analytics` feature; the analytics-dependent helpers stay on `server`.
- Move `TimeRange` to `micromegas-telemetry` (issue option 1) rather than add a client-local type or a new crate (avoids a new publish target).
- Direct `arrow` dep, because `arrow-flight` does not re-export `RecordBatch`/`SchemaRef` at a stable path.

## Documentation

- `rust/public/src/lib.rs` crate docs: add a short "Cargo features" section listing the
  default (instrumentation only), `client` (FlightSQL client, re-exports `arrow`, `arrow_flight`,
  `tonic`; `TimeRange` at `micromegas::telemetry::time_range::TimeRange`) and `server` (everything,
  implies `client`).
- `CHANGELOG.md`: entry for the new feature, plus a note that `TimeRange` now lives in
  `micromegas-telemetry` (re-exported from its old path, so no break).
- No mkdocs page covers the Rust query client today, so none needs updating.

## Testing Strategy

- `build/test_check_client_deps.py` (pytest, no toolchain): `find_forbidden` on canned
  `cargo tree --prefix none` output — clean output returns `[]`; output containing `datafusion v55.2.0`
  and `sqlx v0.8.6` returns both; a crate whose name merely contains a forbidden name as a prefix
  (e.g. `datafusion-wasm-bindgen`, or `sqlx-core` vs `sqlx`) is matched only on the exact name;
  `(*)` duplicate markers and feature suffixes are tolerated.
- The `Client Feature Clippy` CI step is the build-level regression guard for the client modules.
- Existing tests: `flightsql_client_factory_scheme_tests` now runs under `client`; the full
  workspace `cargo nextest run` confirms `server` behaves as before.
