# StaticTablesConfigurator Auto-Discovery Plan

## Overview

Add a reusable `StaticTablesConfigurator` that auto-discovers JSON and CSV files under an object store URL at startup and registers each as a queryable DataFusion table. This eliminates the need for custom `SessionConfigurator` implementations with hardcoded file paths — drop a file at the URL, restart the service, and it becomes queryable.

Covers [#946](https://github.com/madesroches/micromegas/issues/946).

## Current State

- `SessionConfigurator` trait exists at `rust/analytics/src/lakehouse/session_configurator.rs` — any implementation can register arbitrary tables into a `SessionContext`.
- `json_table_provider()` at `rust/analytics/src/dfext/json_table_provider.rs` creates a `ListingTable`-backed `TableProvider` for JSONL files with pre-computed schema inference.
- `flight-sql-srv` (`rust/flight-sql-srv/src/flight_sql_srv.rs:52`) currently hardcodes `NoOpSessionConfigurator`.
- No CSV table provider exists in the codebase. DataFusion 52.3 ships `datafusion::datasource::file_format::csv::CsvFormat` which follows the same `ListingTable` pattern as `JsonFormat`.
- Object store listing is available via `object_store.list(Some(prefix))` (used in `verify_files_exist()` at `json_table_provider.rs:36`).

## Design

### New module: `csv_table_provider`

Mirror `json_table_provider.rs` using `CsvFormat` instead of `JsonFormat`. Same signature and pattern:

```
pub async fn csv_table_provider(
    ctx: &SessionContext,
    url: &str,
) -> Result<Arc<dyn TableProvider>>
```

Reuse the existing `verify_files_exist()` — extract it into a shared helper or duplicate (it's 15 lines). Since both providers import from the same `dfext` module, extract `verify_files_exist` to a shared spot (e.g. make it `pub(crate)` in `json_table_provider.rs` or move to a small `dfext::file_utils` module).

### New module: `static_tables_configurator`

```rust
#[derive(Debug)]
pub struct StaticTablesConfigurator {
    tables: Vec<(String, Arc<dyn TableProvider>)>,
}
```

**Constructor** — `async fn new(ctx: &SessionContext, url: &str) -> Result<Self>`:

1. Parse the URL via `object_store::parse_url()` to get an `ObjectStore` handle and a path (the path portion acts as the listing prefix).
2. Call `object_store.list(Some(&path))` to enumerate all files under that path.
3. For each file, match extension:
   - `.json` / `.jsonl` → `json_table_provider(ctx, &file_url)`
   - `.csv` → `csv_table_provider(ctx, &file_url)`
   - Other extensions → log a warning, skip
4. Table name = filename stem (e.g., `event_schemas.json` → `event_schemas`).
5. Collect `(name, provider)` pairs into `self.tables`.
6. Log errors per file but continue (resilient — one bad file doesn't block the rest).

**`SessionConfigurator` impl** — iterates `self.tables` and calls `ctx.register_table(name, provider)` for each.

The constructor needs a `SessionContext` (or at minimum a `RuntimeEnv` + `SessionState`) for schema inference. Since `flight-sql-srv` already creates a `RuntimeEnv`, we can construct a temporary `SessionContext` from it to register the object store and infer schemas. Alternatively, accept a `&SessionContext` directly — the existing `json_table_provider` already takes one.

### Update `flight-sql-srv` binary

1. Read optional env var `MICROMEGAS_STATIC_TABLES_URL` (e.g., `s3://other-bucket/tables/` or `file:///data/tables/`). This is a standalone URL, independent from `MICROMEGAS_OBJECT_STORE_URI`.
2. If set: construct a temporary `SessionContext` with the runtime, call `StaticTablesConfigurator::new(ctx, &url)` which parses the URL and registers its own object store internally.
3. If unset: fall back to `NoOpSessionConfigurator` (fully backward compatible).

## Implementation Steps

### Phase 1: CSV table provider

1. Extract `verify_files_exist` from `json_table_provider.rs` into a `pub(crate)` function (keep it in the same file or a small `file_utils` sub-module under `dfext/`).
2. Create `rust/analytics/src/dfext/csv_table_provider.rs` mirroring `json_table_provider.rs` with `CsvFormat`.
3. Add `pub mod csv_table_provider;` to `rust/analytics/src/dfext/mod.rs`.

### Phase 2: StaticTablesConfigurator

4. Create `rust/analytics/src/lakehouse/static_tables_configurator.rs` with:
   - `StaticTablesConfigurator` struct
   - `async fn new(...)` constructor doing URL listing + dispatch
   - `SessionConfigurator` impl
5. Add `pub mod static_tables_configurator;` to `rust/analytics/src/lakehouse/mod.rs`.

### Phase 3: flight-sql-srv integration

6. In `rust/flight-sql-srv/src/flight_sql_srv.rs`:
   - Read `MICROMEGAS_STATIC_TABLES_URL` env var (optional).
   - If set, create `StaticTablesConfigurator` and use it instead of `NoOpSessionConfigurator`.
   - Log the number of discovered tables at startup.

### Phase 4: Tests

7. Add `rust/analytics/tests/csv_table_test.rs` — mirror `json_table_test.rs` with CSV temp files.
8. Add `rust/analytics/tests/static_tables_test.rs`:
   - Create a temp directory with mixed `.json`, `.csv`, and `.txt` files.
   - Verify auto-discovery picks up JSON and CSV, skips `.txt`.
   - Verify table names match filename stems.
   - Verify error resilience (one bad file doesn't block others).

## Files to Modify

| File | Action |
|------|--------|
| `rust/analytics/src/dfext/json_table_provider.rs` | Make `verify_files_exist` `pub(crate)` |
| `rust/analytics/src/dfext/csv_table_provider.rs` | **New** — CSV table provider |
| `rust/analytics/src/dfext/mod.rs` | Add `csv_table_provider` module |
| `rust/analytics/src/lakehouse/static_tables_configurator.rs` | **New** — auto-discovery configurator |
| `rust/analytics/src/lakehouse/mod.rs` | Add `static_tables_configurator` module |
| `rust/flight-sql-srv/src/flight_sql_srv.rs` | Read env var, wire up configurator |
| `rust/analytics/tests/csv_table_test.rs` | **New** — CSV provider tests |
| `rust/analytics/tests/static_tables_test.rs` | **New** — auto-discovery tests |

## Trade-offs

**Object store URL handling**: The env var is a single standalone URL (e.g., `s3://other-bucket/tables/`), independent from the main object store. The constructor uses `object_store::parse_url()` to split store and path, same pattern as `BlobStorage::connect()`. This allows static tables to live in a completely different bucket or store type than the telemetry data.

**Schema inference timing**: Schema is inferred once at startup for each file. If files change on disk, a restart is required. This is acceptable for "static" reference tables and matches the issue's design ("restart the service, and it becomes queryable").

**verify_files_exist sharing**: Could move to a separate `dfext::file_utils` module, but since it's a small function and only used by two callers in the same parent module, making it `pub(crate)` in `json_table_provider.rs` and importing from there is simpler. If a third caller appears, refactor then.

**CsvFormat options**: DataFusion's `CsvFormat` supports options like `has_header`, delimiter, quote char. The initial implementation will use defaults (header=true, comma-delimited). If needed later, these could be configurable via file naming conventions or a sidecar config, but that's out of scope.

## Documentation

- `mkdocs/docs/admin/` — Add a new page or section for configuring static tables via `MICROMEGAS_STATIC_TABLES_URL` env var.
- `mkdocs/docs/query-guide/index.md` — Mention static tables alongside the existing data views table.
- `mkdocs/docs/architecture/index.md` — Brief mention that the analytics layer can serve static JSON/CSV files as tables.

## Testing Strategy

1. **Unit tests for csv_table_provider**: Create temp `.csv` files, verify schema inference and query results (mirror existing `json_table_test.rs` pattern).
2. **Integration tests for StaticTablesConfigurator**: Create a temp directory with mixed file types, verify correct auto-discovery, table name derivation, and resilience to bad files.
3. **Negative tests**: Non-existent path returns empty configurator (no tables, no error). Malformed files log errors but don't crash.
4. **cargo clippy && cargo fmt** before any commit.

## Resolved Questions

1. **`MICROMEGAS_STATIC_TABLES_URL` is a standalone URL** — fully independent from `MICROMEGAS_OBJECT_STORE_URI`. This allows pointing to a different bucket, store type, or filesystem path (e.g., `s3://other-bucket/tables/` or `file:///data/tables/`). `object_store::parse_url()` splits it into a store handle and a path for listing.
2. **Table name collisions with built-in views**: warn and skip. The configurator should check registered table names in the `SessionContext` before calling `register_table()` and log a warning if the name is already taken, rather than shadowing built-in views.
