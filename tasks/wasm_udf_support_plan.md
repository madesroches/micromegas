# WASM Query Engine UDF Support Plan

## Overview

Add JSONB and histogram UDF support to the WASM query engine (`datafusion-wasm`) so that client-side SQL queries in the browser can use the same analytical functions available on the server. Currently the WASM engine creates a bare `SessionContext` with no custom functions, limiting it to built-in DataFusion SQL.

## Current State

### WASM Query Engine
- `rust/datafusion-wasm/src/lib.rs` creates a `SessionContext` with no registered UDFs
- Dependencies: `arrow`, `datafusion`, `micromegas-tracing`, `micromegas-telemetry-sink`, `wasm-bindgen`
- Compiles to `wasm32-unknown-unknown`

### Server-Side UDF Registration
- `rust/analytics/src/lakehouse/query.rs:180` — `register_extension_functions()` registers all non-lakehouse UDFs
- These include JSONB functions, histogram functions, and property functions

### JSONB UDFs (`rust/analytics/src/dfext/jsonb/`)
- `jsonb_parse` — JSON string → JSONB binary
- `jsonb_format_json` — JSONB → JSON string
- `jsonb_get` — extract nested value by key
- `jsonb_as_string`, `jsonb_as_i64`, `jsonb_as_f64` — type casts
- `jsonb_object_keys` — extract object keys
- Dependencies: `datafusion`, `jsonb` crate, `micromegas_tracing::warn` (one file)

### Histogram UDFs (`rust/analytics/src/dfext/histogram/`)
- `make_histogram` (UDAF) — create histogram from values
- `sum_histograms` (UDAF) — merge histograms
- `expand_histogram` (UDTF) — histogram → rows of (bin_center, count)
- `quantile_from_histogram`, `variance_from_histogram`, `count_from_histogram`, `sum_from_histogram` — scalar accessors
- Dependencies: `datafusion` only (no external crates beyond std)

### Property UDFs (`rust/analytics/src/properties/`)
- `property_get`, `properties_to_dict`, `properties_to_array`, `properties_to_jsonb`, `properties_length`
- Dependencies: `micromegas_transit`, `micromegas_telemetry`, `anyhow`, deep `crate::` references
- **Not candidates for WASM extraction** — heavy internal dependencies, and properties are decoded server-side before IPC transfer

## Problem

The `micromegas-analytics` crate cannot be compiled to WASM — it depends on `sqlx`, `tokio`, `object_store`, `moka`, `arrow-flight`, and other non-WASM-compatible crates. We need to extract the WASM-compatible UDFs into a shared crate.

## Design

Extract the JSONB and histogram UDF modules into a new crate `micromegas-datafusion-extensions` that compiles to both native and `wasm32-unknown-unknown`. Both `micromegas-analytics` and `micromegas-datafusion-wasm` depend on this crate.

```
                    micromegas-datafusion-extensions
                   (jsonb + histogram UDFs)
                   /                      \
    micromegas-analytics          micromegas-datafusion-wasm
    (re-exports, adds lakehouse     (registers UDFs in WASM
     and property UDFs)              SessionContext)
```

### New Crate: `rust/datafusion-extensions/`

**Dependencies** (all WASM-compatible):
- `datafusion` (already used by datafusion-wasm)
- `jsonb` (pure Rust, no system deps)
- `micromegas-tracing` (already WASM-compatible via `dispatch_wasm.rs`)

**Modules** — moved from `rust/analytics/src/dfext/`:
- `jsonb/` — all 5 submodules (cast, format_json, get, keys, parse)
- `histogram/` — all 7 submodules (histogram_udaf, sum_histograms_udaf, accumulator, accessors, quantile, variance, expand)

**Public API**:
```rust
// rust/datafusion-extensions/src/lib.rs
pub mod jsonb;
pub mod histogram;

/// Register all extension UDFs on a SessionContext.
pub fn register_extension_udfs(ctx: &datafusion::prelude::SessionContext);
```

### Changes to `micromegas-analytics`

- Remove `src/dfext/jsonb/` and `src/dfext/histogram/` source files
- Add `micromegas-datafusion-extensions` as a dependency
- Re-export via `pub use micromegas_datafusion_extensions::{jsonb, histogram}` in `src/dfext/mod.rs`
- Update `register_extension_functions()` in `query.rs` to call `micromegas_datafusion_extensions::register_extension_udfs(ctx)` for the jsonb/histogram UDFs, then register property UDFs locally

### Changes to `micromegas-datafusion-wasm`

- Add `micromegas-datafusion-extensions` as a dependency
- Call `micromegas_datafusion_extensions::register_extension_udfs(&ctx)` in `WasmQueryEngine::new()`

## Implementation Steps

### Phase 1: Create the `micromegas-datafusion-extensions` crate

1. Create `rust/datafusion-extensions/Cargo.toml` with dependencies: `datafusion`, `jsonb`, `micromegas-tracing`
2. Move `rust/analytics/src/dfext/jsonb/` → `rust/datafusion-extensions/src/jsonb/`
3. Move `rust/analytics/src/dfext/histogram/` → `rust/datafusion-extensions/src/histogram/`
4. Create `rust/datafusion-extensions/src/lib.rs` with module declarations and `register_extension_udfs()` function
5. Fix import paths (`crate::` → appropriate new paths)
6. Add `micromegas-datafusion-extensions` to workspace `Cargo.toml` members and `[workspace.dependencies]`

### Phase 2: Update `micromegas-analytics`

7. Add `micromegas-datafusion-extensions` dependency to `rust/analytics/Cargo.toml`
8. Remove moved source files from `rust/analytics/src/dfext/`
9. Update `rust/analytics/src/dfext/mod.rs` to re-export from `micromegas-datafusion-extensions`
10. Update `rust/analytics/src/lakehouse/query.rs` — replace direct UDF construction with `micromegas_datafusion_extensions::register_extension_udfs()`; keep property UDF registration local
11. Update any other `use crate::dfext::{jsonb, histogram}` paths in analytics

### Phase 3: Wire up WASM engine

12. Add `micromegas-datafusion-extensions` dependency to `rust/datafusion-wasm/Cargo.toml`
13. Call `micromegas_datafusion_extensions::register_extension_udfs(&ctx)` in `WasmQueryEngine::new()` after creating the session context
14. Add WASM integration tests for JSONB and histogram UDFs

### Phase 4: Verify

15. `cargo build` — native build
16. `cargo test` — all existing tests pass
17. `wasm-pack test --headless --chrome` in `rust/datafusion-wasm/` — WASM tests pass
18. `cargo clippy --workspace -- -D warnings`
19. `cargo fmt`

## Files to Modify

**New files:**
- `rust/datafusion-extensions/Cargo.toml`
- `rust/datafusion-extensions/src/lib.rs`
- `rust/datafusion-extensions/src/binary_column_accessor.rs` (moved from analytics)
- `rust/datafusion-extensions/src/jsonb/` (moved from analytics)
- `rust/datafusion-extensions/src/histogram/` (moved from analytics)

**Modified files:**
- `rust/Cargo.toml` — add workspace member and dependency
- `rust/analytics/Cargo.toml` — add `micromegas-datafusion-extensions` dependency
- `rust/analytics/src/dfext/mod.rs` — re-export instead of owning modules
- `rust/analytics/src/lakehouse/query.rs` — delegate to `micromegas_datafusion_extensions`
- `rust/datafusion-wasm/Cargo.toml` — add `micromegas-datafusion-extensions` dependency
- `rust/datafusion-wasm/src/lib.rs` — register UDFs on context creation
- `rust/datafusion-wasm/tests/wasm_integration.rs` — add UDF tests

**Removed files:**
- `rust/analytics/src/dfext/binary_column_accessor.rs` (moved)
- `rust/analytics/src/dfext/jsonb/*.rs` (moved)
- `rust/analytics/src/dfext/histogram/*.rs` (moved)

## Trade-offs

**Chosen: Extract to shared crate**
- Clean separation of WASM-compatible code
- Single source of truth for UDF implementations
- Both server and client always have identical function behavior

**Alternative considered: Duplicate UDF code in datafusion-wasm**
- Rejected — violates DRY, risks divergence between server and client behavior

**Alternative considered: Feature-flag analytics crate for WASM**
- Rejected — analytics has too many non-WASM deps; feature-flagging would require wrapping most of the crate

**Property UDFs excluded from extraction**
- They depend on `micromegas_transit`, `micromegas_telemetry`, and internal analytics types
- Properties are decoded server-side before IPC transfer, so they're not needed in WASM context
- Can be extracted later if needed

## Testing Strategy

1. **Existing analytics tests** — must continue passing after the refactor (no behavior change)
2. **New WASM integration tests** in `rust/datafusion-wasm/tests/wasm_integration.rs`:
   - `test_jsonb_parse_and_format` — register table with JSON strings, use `jsonb_parse()` and `jsonb_format_json()`
   - `test_jsonb_get_and_cast` — parse JSON, extract fields with `jsonb_get()`, cast with `jsonb_as_string()`/`jsonb_as_i64()`
   - `test_histogram_create_and_query` — register numeric data, create histogram with `make_histogram()`, extract with accessor UDFs
   - `test_histogram_expand` — verify `expand_histogram()` UDTF works in WASM
3. **CI** — `python3 ../build/rust_ci.py` covers both native and (if configured) WASM targets

## Dependencies Within `dfext`

The `jsonb/format_json.rs` module imports `crate::dfext::binary_column_accessor::create_binary_accessor`. This utility (`rust/analytics/src/dfext/binary_column_accessor.rs`) depends only on `anyhow`, `datafusion::arrow`, and `std` — fully WASM-compatible. It must be moved to the new crate alongside the jsonb module.

The `string_column_accessor.rs` is **not** used by jsonb or histogram modules and stays in analytics.

## Open Questions

None — crate name resolved as `micromegas-datafusion-extensions`.
