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

#### 2. JSON Helper Function
**File**: `rust/analytics/src/lakehouse/json_table_provider.rs`

A standalone utility function to create a DataFusion `TableProvider` from a JSON file:

```rust
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::datasource::listing::{ListingOptions, ListingTable, ListingTableUrl};

/// Creates a TableProvider for a JSON file with pre-computed schema
///
/// This function infers the schema once and returns a TableProvider that can be
/// registered in multiple SessionContexts without re-inferring the schema.
pub async fn json_table_provider(
    path: impl AsRef<str>,
) -> Result<Arc<dyn TableProvider>> {
    let path_str = path.as_ref();

    // Create listing options with JSON format
    let file_format = Arc::new(JsonFormat::default());
    let listing_options = ListingOptions::new(file_format)
        .with_file_extension("json");

    // Parse the path as a listing table URL
    let table_path = ListingTableUrl::parse(path_str)?;

    // Create ListingTable with schema inference
    // Schema is inferred once here and cached in the ListingTable
    let listing_table = ListingTable::try_new(table_path, listing_options)
        .await?;

    Ok(Arc::new(listing_table))
}
```

Users can call this function to get a pre-configured table provider for their JSON files.

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

### Phase 1: Core Trait and Helper
1. Create `session_configurator.rs` with:
   - `SessionConfigurator` trait
   - `NoOpSessionConfigurator` implementation
2. Create `json_table_provider.rs` with:
   - `json_table_provider()` helper function
3. Add both modules to `lakehouse/mod.rs`
4. Export the trait and function from `analytics/lib.rs`

### Phase 2: Integration
1. Modify `make_session_context()` to accept `Arc<dyn SessionConfigurator>`
2. Update `query()` function signature to pass through the parameter
3. Update `FlightSqlServiceImpl::new()` to accept and store the session configurator
4. Update `FlightSqlServiceImpl` query methods to pass it to `make_session_context()`
5. Update `flight_sql_srv.rs` main function to pass `Arc::new(NoOpSessionConfigurator)` as default

### Phase 3: Testing & Documentation
1. Create example `SessionConfigurator` implementation using `json_table_provider()` in tests
2. Add integration test that queries custom JSON table
3. Document the trait API and `json_table_provider()` function in rustdoc
4. Update the design doc with final API

## Performance Considerations

**Schema Inference Cost**:
- `register_json()` performs schema inference every time it's called
- With many session contexts per query, this becomes expensive
- Solution: Call `json_table_provider()` once at startup to infer schema

**When to Infer**:
- During configurator construction at server startup
- `ListingTable::try_new()` infers schema during construction
- Cache the `Arc<dyn TableProvider>` with pre-computed schema
- No temporary context needed - direct `ListingTable` creation

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

The key insight is to use `ListingTable` directly with a pre-computed schema, avoiding schema inference on every `SessionContext` allocation.

Users implement `SessionConfigurator` and use the provided `json_table_provider()` helper:

```rust
use anyhow::Result;
use datafusion::execution::context::SessionContext;
use micromegas::analytics::lakehouse::SessionConfigurator;
use micromegas::analytics::lakehouse::json_table_provider::json_table_provider;

struct MyConfigurator {
    example_table: Arc<dyn TableProvider>,
}

impl MyConfigurator {
    /// Initialize with schema inference (done once at startup)
    pub async fn new() -> Result<Self> {
        // Infer schema once during initialization using helper
        let example_table = json_table_provider("/path/to/data.json").await?;

        Ok(Self { example_table })
    }
}

#[async_trait::async_trait]
impl SessionConfigurator for MyConfigurator {
    async fn configure(&self, ctx: &SessionContext) -> Result<()> {
        // Register with pre-computed table (fast, no inference)
        ctx.register_table("example", self.example_table.clone())?;
        Ok(())
    }
}
```

The `json_table_provider()` function:
1. Creates `ListingOptions` with `JsonFormat` (same as `register_json`)
2. Parses the file path as `ListingTableUrl`
3. Creates `ListingTable` which infers schema once during construction
4. Returns an `Arc<dyn TableProvider>` ready for registration

This replicates `register_json` behavior without the temporary context:
- Direct creation of `ListingTable` with schema inference
- Natural async flow with DataFusion's built-in I/O
- Schema is inferred once and cached in the `ListingTable`
- No temporary context needed

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
