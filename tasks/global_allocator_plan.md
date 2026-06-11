# Global Allocator Plan

Issue: https://github.com/madesroches/micromegas/issues/1129

## Overview

Replace the default system allocator with a high-performance allocator across all
production service binaries. The system allocator (glibc malloc on Linux) is a
measurable bottleneck under multi-threaded, high-churn allocation workloads — all
long-running services (ingestion, FlightSQL analytics, analytics web) experience
this. The swap is a single declaration per binary with no API changes.

**Chosen allocator: `tikv-jemallocator` (jemalloc).** See Trade-offs for the
jemalloc vs snmalloc vs mimalloc analysis.

## Current State

No `#[global_allocator]` is declared anywhere in the workspace; every binary uses
Rust's default system allocator. The five production entry points are:

| Crate | Entry point |
|---|---|
| `telemetry-ingestion-srv` | `rust/telemetry-ingestion-srv/src/main.rs` |
| `flight-sql-srv` | `rust/flight-sql-srv/src/flight_sql_srv.rs` |
| `analytics-web-srv` | `rust/analytics-web-srv/src/main.rs` |
| `http-gateway` | `rust/http-gateway/src/http_gateway_srv.rs` |
| `telemetry-admin-cli` | `rust/telemetry-admin-cli/src/telemetry_admin.rs` |

No allocator crate (jemalloc/mimalloc/snmalloc) appears in `rust/Cargo.toml`'s
`[workspace.dependencies]`.

## Design

### Crate choice

`tikv-jemallocator` (jemalloc 5.x) — maintained fork of `jemallocator` by TiKV/Meta.
Exposes a single `Jemalloc` struct that implements `GlobalAlloc`. Chosen over
snmalloc and mimalloc; see Trade-offs.

### Allocation declaration pattern

Each binary's entry-point file gets:

```rust
#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
```

jemalloc does not support Windows. The `cfg` guard alone only removes the
static — it does not stop `jemalloc-sys` from compiling — so the dependency is
also target-gated (see Workspace dependency below). All production services run
on Linux; the guard is defensive (there is currently no Windows CI).

### Scope

Apply to all five production service entry points listed above.
`telemetry-admin-cli` counts as a service because it runs as a long-lived
maintenance daemon via its `crond` subcommand (as started by
`local_test_env/ai_scripts/start_services.py`). Exclude:
- `telemetry-generator` (test load-generator, not a service)
- `micromegas-uri-handler` (client-side CLI tool, not a long-running service)
- `update-perfetto-protos`, `write-perfetto`, `validate-perfetto` (dev / example tools)

### Workspace dependency

Add to `rust/Cargo.toml` `[workspace.dependencies]`, after `thread-id` and
before `tokio` (note: the file currently has `thrift` before `thread-id`;
`tikv` sorts after both):

```toml
tikv-jemallocator = "0.6"
```

Each service crate's `Cargo.toml` then adds a target-gated dependency, matching
the `cfg` guard on the static (a plain `[dependencies]` entry would still
compile `jemalloc-sys` on Windows, where it fails to build):

```toml
[target.'cfg(not(target_os = "windows"))'.dependencies]
tikv-jemallocator.workspace = true
```

## Implementation Steps

1. **Add workspace dependency** — `rust/Cargo.toml`: insert `tikv-jemallocator`
   in `[workspace.dependencies]` in alphabetical order.

2. **telemetry-ingestion-srv**
   - `rust/telemetry-ingestion-srv/Cargo.toml`: add `tikv-jemallocator.workspace = true`
     under `[target.'cfg(not(target_os = "windows"))'.dependencies]`
   - `rust/telemetry-ingestion-srv/src/main.rs`: add allocator declaration
     after any `//!` inner doc comments, before the `use` statements (same
     placement in steps 3–6).

3. **flight-sql-srv**
   - `rust/flight-sql-srv/Cargo.toml`: add dependency
   - `rust/flight-sql-srv/src/flight_sql_srv.rs`: add allocator declaration

4. **analytics-web-srv**
   - `rust/analytics-web-srv/Cargo.toml`: add dependency
   - `rust/analytics-web-srv/src/main.rs`: add allocator declaration

5. **http-gateway**
   - `rust/http-gateway/Cargo.toml`: add dependency
   - `rust/http-gateway/src/http_gateway_srv.rs`: add allocator declaration

6. **telemetry-admin-cli**
   - `rust/telemetry-admin-cli/Cargo.toml`: add dependency
   - `rust/telemetry-admin-cli/src/telemetry_admin.rs`: add allocator declaration

7. **Format & lint** — `cargo fmt` + `cargo clippy --workspace -- -D warnings`
   from `rust/`.

## Files to Modify

| File | Change |
|---|---|
| `rust/Cargo.toml` | Add `tikv-jemallocator` workspace dependency |
| `rust/telemetry-ingestion-srv/Cargo.toml` | Add dep |
| `rust/telemetry-ingestion-srv/src/main.rs` | Add `#[global_allocator]` |
| `rust/flight-sql-srv/Cargo.toml` | Add dep |
| `rust/flight-sql-srv/src/flight_sql_srv.rs` | Add `#[global_allocator]` |
| `rust/analytics-web-srv/Cargo.toml` | Add dep |
| `rust/analytics-web-srv/src/main.rs` | Add `#[global_allocator]` |
| `rust/http-gateway/Cargo.toml` | Add dep |
| `rust/http-gateway/src/http_gateway_srv.rs` | Add `#[global_allocator]` |
| `rust/telemetry-admin-cli/Cargo.toml` | Add dep |
| `rust/telemetry-admin-cli/src/telemetry_admin.rs` | Add `#[global_allocator]` |

## Trade-offs

**Allocator comparison for DataFusion/Arrow workloads**

| Allocator | Crate | Notes |
|---|---|---|
| jemalloc | `tikv-jemallocator` | Used by Polars, DuckDB, ClickHouse; excellent heap-profiling (opt-in via the `profiling` cargo feature; see Testing Strategy); does not compile on Windows |
| snmalloc | `snmalloc-rs` | DataFusion docs recommend it; wins DataFusion TPC-DS benchmarks over mimalloc; best RSS return-to-OS; least production ecosystem validation |
| mimalloc | `mimalloc` | What DataFusion CLI ships; cross-platform; can retain RSS under bursty workloads (1.5 GB → 3.2 GB observed in DataFusion benchmarks) |

**Why jemalloc**: strong OLAP production track record (Polars chose it over
mimalloc on Linux/macOS citing "outperforms on all tasks"; DuckDB v1.5.3 bundled
it to fix glibc's failure to return freed pages). snmalloc's DataFusion benchmark
wins are real but it lacks the same depth of production validation and the
`malloc_conf` observability tooling is weaker. mimalloc's RSS retention behavior
is a concern for long-running services. Note that jemalloc's heap profiling is
not available with `tikv-jemallocator`'s default features — it requires building
with the `profiling` cargo feature (which compiles jemalloc with
`--enable-prof`); the planned default-feature dependency keeps the door open for
this without enabling it up front.

**Future**: once jemalloc is in place, a head-to-head with snmalloc on a
representative `micromegas-query` workload is the natural next step — the swap
is still one line.

**Per-binary vs library**: the `#[global_allocator]` attribute lives in binary
crates, not library crates, so library crates are unaffected. This is intentional
— library crates should not impose an allocator on their consumers.

**Conditional compilation**: the `#[cfg(not(target_os = "windows"))]` guard on
the static plus the target-gated dependency together prevent breakage if any
service is ever built on Windows or in CI on a Windows runner. Both are needed:
the cfg attribute removes the declaration, but only the target gate keeps
`jemalloc-sys` out of the build graph on Windows.

## Testing Strategy

- `python3 build/rust_ci.py native` — verifies format, clippy, tests, and
  `cargo machete` (relevant here since a new dependency is added); the bare
  invocation would also run the unrelated wasm pipeline.
- Smoke test: start services via `local_test_env/ai_scripts/start_services.py`,
  run a representative query with `micromegas-query`, confirm no crashes.
- Heap profiling (optional, future): not available with the default features
  planned here. Requires building `tikv-jemallocator` with
  `features = ["profiling"]` (compiles jemalloc with `--enable-prof`), then
  enabling profiles at runtime via `_RJEM_MALLOC_CONF=prof:true` (the sys crate
  prefixes symbols with `_rjem_` by default; add the
  `unprefixed_malloc_on_supported_platforms` feature to use plain
  `MALLOC_CONF`). To be done as a separate step when before/after allocation
  profiles are needed.
