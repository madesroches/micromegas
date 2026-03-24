# LakehouseContext::from_env() Convenience Constructor Plan

Issue: [#969](https://github.com/madesroches/micromegas/issues/969)

## Overview

Add a `LakehouseContext::from_env()` async constructor that reads `MICROMEGAS_SQL_CONNECTION_STRING` and `MICROMEGAS_OBJECT_STORE_URI` from environment variables, connects to the data lake, runs the lakehouse migration, creates the DataFusion runtime, and returns an `Arc<LakehouseContext>`. This eliminates a 6-line initialization sequence duplicated across binaries.

## Current State

The initialization pattern is repeated in two places that create a `LakehouseContext`:

**telemetry-admin-cli** (`rust/telemetry-admin-cli/src/telemetry_admin.rs:68-77`):
```rust
let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
    .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
    .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
let data_lake = Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
migrate_lakehouse(data_lake.db_pool.clone()).await
    .with_context(|| "migrate_lakehouse")?;
let runtime = Arc::new(make_runtime_env()?);
let lakehouse = Arc::new(LakehouseContext::new(data_lake.clone(), runtime.clone()));
```

**FlightSqlServer builder** (`rust/public/src/servers/flight_sql_server.rs:142-154`):
```rust
let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
    .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
    .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
let data_lake = Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
migrate_lakehouse(data_lake.db_pool.clone()).await
    .with_context(|| "migrate_lakehouse")?;
let runtime = Arc::new(make_runtime_env()?);
let lakehouse = Arc::new(LakehouseContext::new(data_lake.clone(), runtime));
```

Note: `telemetry-ingestion-srv` uses `connect_to_remote_data_lake()` and does **not** create a `LakehouseContext`, so it is not affected.

## Design

Add a single `from_env` method to `LakehouseContext`:

```rust
impl LakehouseContext {
    /// Reads MICROMEGAS_SQL_CONNECTION_STRING and MICROMEGAS_OBJECT_STORE_URI,
    /// connects to the data lake, runs lakehouse migrations, and creates the
    /// runtime environment.
    pub async fn from_env() -> Result<Arc<Self>> {
        let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
            .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
        let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
            .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
        let data_lake =
            Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
        migrate_lakehouse(data_lake.db_pool.clone())
            .await
            .with_context(|| "migrate_lakehouse")?;
        let runtime = Arc::new(make_runtime_env()?);
        Ok(Arc::new(Self::new(data_lake, runtime)))
    }
}
```

The `analytics` crate already depends on `micromegas-ingestion`, so `connect_to_data_lake` is available. The `migrate_lakehouse` and `make_runtime_env` functions are already in the same `lakehouse` module.

### Why `Arc<Self>` return type

Both call sites immediately wrap the result in `Arc`. Returning `Arc<Self>` matches the ergonomics of every consumer and is consistent with the existing `StaticTablesConfigurator::from_env()` pattern which also returns an `Arc`.

### Callers still need `data_lake` separately

Both the telemetry-admin and the FlightSQL builder use `data_lake` independently after creating the lakehouse context (for `default_view_factory`, `LivePartitionProvider`, etc.). They can retrieve it via `lakehouse.lake()` which returns `&Arc<DataLakeConnection>`, so no additional return value is needed.

## Implementation Steps

1. **Add `from_env` to `LakehouseContext`** (`rust/analytics/src/lakehouse/lakehouse_context.rs`)
   - Add `use anyhow::Context;` and `use super::migration::migrate_lakehouse;` and `use super::runtime::make_runtime_env;` and `use micromegas_ingestion::data_lake_connection::connect_to_data_lake;`
   - Add the `from_env` async method to the existing `impl LakehouseContext` block

2. **Update telemetry-admin-cli** (`rust/telemetry-admin-cli/src/telemetry_admin.rs`)
   - Replace lines 68-77 with `let lakehouse = LakehouseContext::from_env().await?;`
   - Get `data_lake` via `lakehouse.lake().clone()` where needed (for `view_factory`, etc.)
   - Get `runtime` via `lakehouse.runtime().clone()` where needed
   - Remove unused imports: `connect_to_data_lake`, `migrate_lakehouse`, `make_runtime_env`

3. **Update FlightSqlServer builder** (`rust/public/src/servers/flight_sql_server.rs`)
   - Replace lines 142-154 with `let lakehouse = LakehouseContext::from_env().await?;`
   - Get `data_lake` via `lakehouse.lake().clone()` (already an `Arc`)
   - Keep the info log about metadata cache
   - Remove unused imports for `connect_to_data_lake`, `migrate_lakehouse`, `make_runtime_env`

4. **Run `cargo fmt` and `cargo clippy`** from `rust/`

5. **Run `cargo test`** to verify no regressions

## Files to Modify

- `rust/analytics/src/lakehouse/lakehouse_context.rs` — add `from_env` method
- `rust/telemetry-admin-cli/src/telemetry_admin.rs` — simplify initialization
- `rust/public/src/servers/flight_sql_server.rs` — simplify initialization

## Trade-offs

**Alternative: Return `(Arc<LakehouseContext>, Arc<DataLakeConnection>)` tuple**
Rejected because callers can already access the data lake via `lakehouse.lake()`. An extra return value adds complexity for no benefit.

**Alternative: Accept connection string parameters instead of reading env vars**
Rejected because every caller reads from the same env vars. A parameterized version can be added later if needed, but `from_env` is the 90% case and what the issue requests.

**Alternative: Place this in the `public` crate instead of `analytics`**
Rejected because all the constituent functions (`connect_to_data_lake`, `migrate_lakehouse`, `make_runtime_env`, `LakehouseContext::new`) are already dependencies of the `analytics` crate. Putting `from_env` on the struct itself is the most discoverable location.

## Testing Strategy

- `cargo test` — existing tests in `analytics` and integration tests should continue to pass
- Manual verification: start services with `start_services.py` and confirm flight-sql-srv and telemetry-admin work correctly
- The method itself is a thin composition of already-tested functions, so no new unit test is needed

## Open Questions

None — the scope is well-defined and the pattern is already established by `StaticTablesConfigurator::from_env`.
