# SessionConfigurator for Analytics Server

## Goal

Allow the analytics server to register custom tables (e.g., JSON files, CSV, or other data sources) as queryable tables through DataFusion. This enables users to query custom data alongside existing telemetry data without requiring a full ingestion pipeline.

## Current Architecture

### Key Components

1. **ViewFactory** (`rust/analytics/src/lakehouse/view_factory.rs`):
   - Manages view sets and global views
   - Provides access to built-in tables: `log_entries`, `measures`, `thread_spans`, `async_events`, `processes`, `streams`, `blocks`, `log_stats`
   - Global views are automatically registered in the SessionContext

2. **Session Context Creation** (`rust/analytics/src/lakehouse/query.rs:213-249`):
   - `make_session_context()` creates a DataFusion SessionContext
   - Registers object store, custom functions, and global views
   - Each global view is registered as a table using `register_table()`

3. **FlightSQL Service** (`rust/flight-sql-srv/src/flight_sql_srv.rs:30-80`):
   - Initializes runtime environment, data lake connection, and view factory
   - Creates FlightSqlServiceImpl with these components

## Design

### Components to Add

#### 1. SessionConfigurator Trait
**File**: `rust/analytics/src/lakehouse/session_configurator.rs`

An async trait that allows custom session context configuration:

```rust
use anyhow::Result;
use datafusion::execution::context::SessionContext;

/// Trait for configuring a SessionContext with additional tables and settings
#[async_trait::async_trait]
pub trait SessionConfigurator: Send + Sync {
    /// Configure the given SessionContext (e.g., register custom tables)
    async fn configure(&self, ctx: &SessionContext) -> Result<()>;
}

/// Default no-op implementation
#[derive(Debug, Clone)]
pub struct NoOpSessionConfigurator;

#[async_trait::async_trait]
impl SessionConfigurator for NoOpSessionConfigurator {
    async fn configure(&self, _ctx: &SessionContext) -> Result<()> {
        Ok(())
    }
}
```

This trait allows users to implement their own session configuration logic:
- Load JSON files from a directory
- Register CSV files
- Create in-memory tables
- Register any DataFusion TableProvider
- Configure session settings

#### 2. JSON Table Provider (Optional)
**File**: `rust/analytics/src/lakehouse/json_table_provider.rs`

A concrete implementation of `SessionConfigurator` for JSON files:
- Pre-computes schema at initialization
- Registers tables using `ListingTable` with cached schema
- Example implementation for users to reference or use directly

### Integration Points

#### In `query.rs`

Modify `make_session_context()` signature to accept a `SessionConfigurator`:

```rust
pub async fn make_session_context(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    configurator: Arc<dyn SessionConfigurator>,
) -> Result<SessionContext> {
    // ... existing code ...

    // Apply custom configuration
    configurator.configure(&ctx).await?;

    Ok(ctx)
}
```

#### In `flight_sql_srv.rs`

Accept a `SessionConfigurator` when creating the service (use default no-op for now):

```rust
// After view_factory creation
let session_configurator = Arc::new(NoOpSessionConfigurator) as Arc<dyn SessionConfigurator>;

let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
    runtime,
    data_lake,
    partition_provider,
    view_factory,
    session_configurator, // New parameter
)?);
```

## Implementation Plan

### Phase 1: Core Trait
1. Create `session_configurator.rs` with the `SessionConfigurator` trait
2. Add the module to `lakehouse/mod.rs`
3. Export the trait from `analytics/lib.rs`

### Phase 2: Integration
1. Modify `make_session_context()` to accept `Arc<dyn SessionConfigurator>`
2. Update `query()` function signature to pass through the parameter
3. Update `FlightSqlServiceImpl::new()` to accept and store the session configurator
4. Update `FlightSqlServiceImpl` query methods to pass it to `make_session_context()`
5. Update `flight_sql_srv.rs` main function to pass `Arc::new(NoOpSessionConfigurator)` as default

### Phase 3: Testing & Documentation
1. Create example JSON table provider in tests
2. Add integration test that queries custom JSON table
3. Document the trait API in rustdoc
4. Update the design doc with final API

## Performance Considerations

**Schema Inference Cost**:
- `register_json()` performs schema inference every time it's called
- With many session contexts, this becomes expensive
- Solution: Infer schema once at provider initialization, reuse in `register_listing_table()`

**When to Infer**:
- During provider construction (e.g., `JsonTableProvider::new()`)
- At server startup, not per-query
- Cache the `SchemaRef` and `ListingOptions`

**Registration Cost**:
- `register_listing_table()` with pre-computed schema is fast
- Only creates table metadata, doesn't read data
- Acceptable per-query overhead

## Technical Details

### DataFusion JSON Support

DataFusion has built-in JSON support through:
- `datafusion::prelude::SessionContext::read_json()`
- Supports **JSONL (newline-delimited JSON)** format only
- Each line must contain a complete JSON object
- Automatically infers schema from JSON data

### Implementation Details: Using ListingTable with Pre-computed Schema

The key insight is to use `ListingTable` directly with a pre-computed schema, avoiding schema inference on every `SessionContext` allocation. Looking at `SessionContext::register_json`, it internally creates a `ListingTable`.

The trait implementation should:
1. **At initialization time**: Infer schema once from JSON file(s)
2. **At registration time**: Use `register_listing_table` with the pre-computed schema

```rust
use anyhow::Result;
use datafusion::execution::context::SessionContext;
use datafusion::datasource::listing::ListingOptions;
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::arrow::datatypes::SchemaRef;
use std::sync::Arc;

struct JsonTableProvider {
    table_name: String,
    table_path: String,
    schema: SchemaRef,  // Pre-computed schema
    options: ListingOptions,
}

impl JsonTableProvider {
    /// Initialize with schema inference (done once at startup)
    pub async fn new(name: String, path: String) -> Result<Self> {
        // Infer schema once during initialization
        let format = JsonFormat::default();
        let options = ListingOptions::new(Arc::new(format));

        // Use temporary context to infer schema
        let temp_ctx = SessionContext::new();
        let schema = temp_ctx
            .register_json(&name, &path, Default::default())
            .await?;
        let schema = temp_ctx.table(&name).await?.schema().inner().clone();

        Ok(Self {
            table_name: name,
            table_path: path,
            schema,
            options,
        })
    }
}

#[async_trait::async_trait]
impl SessionConfigurator for JsonTableProvider {
    async fn configure(&self, ctx: &SessionContext) -> Result<()> {
        // Register with pre-computed schema (fast, no inference)
        ctx.register_listing_table(
            &self.table_name,
            &self.table_path,
            self.options.clone(),
            Some(self.schema.clone()),
            None,
        )
        .await?;

        Ok(())
    }
}
```

## Benefits

1. **Extensible**: Users can implement the trait to add any data source
2. **Code-Driven**: No magic auto-discovery, explicit registration
3. **Type-Safe**: Trait-based approach with compile-time checks
4. **Flexible**: Support JSON, CSV, Parquet, or any DataFusion TableProvider
5. **SQL Integration**: Join custom tables with telemetry data
6. **Development Aid**: Useful for testing and prototyping

## Usage Scenarios

1. **Reference Data**: Load lookup tables from JSON/CSV files
2. **Configuration**: Query application configuration as tables
3. **Test Data**: Inject test data for integration tests
4. **External Data**: Integrate data from external sources
5. **Materialized Views**: Register pre-computed aggregations

## API Design Considerations

- **Required Parameter**: `Arc<dyn SessionConfigurator>` is always required, but defaults to `NoOpSessionConfigurator`
- **No-Op Default**: Provides a simple default implementation that does nothing
- **Explicit Configuration**: Makes configuration explicit rather than hidden in optionals
- **Async Trait**: Allows loading data from network or async I/O
- **SessionContext Access**: Full access to DataFusion API for flexibility
- **Error Handling**: Returns `Result<()>` for proper error propagation
- **Naming**: "Configurator" better reflects that this can configure any aspect of the session, not just tables
