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
**File**: `rust/analytics/src/dfext/json_table_provider.rs`

A standalone utility function to create a DataFusion `TableProvider` from a JSON file:

```rust
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::datasource::listing::{ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl};

/// Creates a TableProvider for a JSON file with pre-computed schema
///
/// This function infers the schema once and returns a TableProvider that can be
/// registered in multiple SessionContexts without re-inferring the schema.
///
/// # Arguments
/// * `url` - URL to the JSON file (e.g., "file:///path/to/data.json" or "s3://bucket/data.json")
///
/// # Returns
/// Returns an `Arc<dyn TableProvider>` ready for registration
pub async fn json_table_provider(url: &str) -> Result<Arc<dyn TableProvider>> {
    let ctx = SessionContext::new();

    // Create listing options with JSON format
    let file_format = Arc::new(JsonFormat::default());
    let listing_options = ListingOptions::new(file_format);

    // Parse the URL as a listing table URL
    let table_url = ListingTableUrl::parse(url)?;

    // Create ListingTable configuration and infer schema
    let mut config = ListingTableConfig::new(table_url)
        .with_listing_options(listing_options);
    config = config.infer_schema(&ctx.state()).await?;

    // Create ListingTable with the inferred schema
    let listing_table = ListingTable::try_new(config)?;

    Ok(Arc::new(listing_table))
}
```

Users can call this function to get a pre-configured table provider for their JSON files.

**Important Notes:**
- Accepts URLs with schemes (e.g., `file://`, `s3://`, `gs://`)
- Files should have `.json` extension for DataFusion's default file discovery
- Schema is inferred once during construction and cached

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

## Implementation Status

### ✅ Phase 1: Core Trait and Helper - COMPLETED
1. ✅ Created `session_configurator.rs` in `lakehouse/` with:
   - `SessionConfigurator` trait with `Debug` bound
   - `NoOpSessionConfigurator` implementation
2. ✅ Created `json_table_provider.rs` in `dfext/` with:
   - `json_table_provider()` helper function accepting `&str` URL parameter
   - Updated to use `ListingTableConfig::infer_schema()` with session state
   - No explicit file extension filtering (relies on `.json` convention)
3. ✅ Added modules to their respective mod.rs files
4. ✅ Exported the trait and function from `analytics/lib.rs`

### ✅ Phase 2: Integration - COMPLETED
1. ✅ Modified `make_session_context()` to accept `Arc<dyn SessionConfigurator>`
2. ✅ Updated `query()` function signature to pass through the parameter
3. ✅ Updated `FlightSqlServiceImpl::new()` to accept and store the session configurator
4. ✅ Updated `FlightSqlServiceImpl` query methods to pass it to `make_session_context()`
5. ✅ Updated `flight_sql_srv.rs` main function to pass `Arc::new(NoOpSessionConfigurator)` as default
6. ✅ **Updated `SqlBatchView` to accept and use `SessionConfigurator`** - enables JSON tables in batch views!
7. ✅ **Updated `ExportLogView` to accept and use `SessionConfigurator`** - enables JSON tables in export views!

### ✅ Phase 3: Testing & Documentation - COMPLETED
1. ✅ Created example `SessionConfigurator` implementation using `json_table_provider()` in tests
2. ✅ Added integration tests in `rust/analytics/tests/json_table_test.rs` that:
   - Test `NoOpSessionConfigurator`
   - Test `json_table_provider()` helper
   - Test custom `SessionConfigurator` with JSON tables
   - Test multiple JSON tables and joins
3. ✅ Documented the trait API and `json_table_provider()` function in rustdoc
4. ✅ Updated `sql_view_test.rs` to pass `SessionConfigurator` to `SqlBatchView`
5. ✅ Added `tempfile` dev dependency for integration tests

### ✅ Phase 4: Completion - COMPLETED
1. ✅ Fixed all remaining compilation errors in:
   - `metadata.rs` - Added `NoOpSessionConfigurator` parameter
   - `batch_partition_merger.rs` - Added `NoOpSessionConfigurator` parameter
   - `merge.rs` - Added `NoOpSessionConfigurator` parameter
   - `log_stats_view.rs` - Added `NoOpSessionConfigurator` parameter
   - `processes_view.rs` - Added `NoOpSessionConfigurator` parameter
   - `streams_view.rs` - Added `NoOpSessionConfigurator` parameter
2. ✅ Added `Debug` bound to `SessionConfigurator` trait
3. ✅ Ran `cargo fmt && cargo clippy --workspace -- -D warnings` - all passed
4. ✅ All tests passing (4/4 in json_table_test.rs)
5. ✅ Updated design doc with final implementation notes

## ✅ IMPLEMENTATION COMPLETE

All phases completed successfully. The SessionConfigurator feature is now fully integrated and tested.

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
use micromegas_analytics::dfext::json_table_provider::json_table_provider;
use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;

struct MyConfigurator {
    example_table: Arc<dyn TableProvider>,
}

impl MyConfigurator {
    /// Initialize with schema inference (done once at startup)
    pub async fn new() -> Result<Self> {
        // Infer schema once during initialization using helper
        // Note: URLs must include scheme (file://, s3://, etc.)
        let example_table = json_table_provider("file:///path/to/data.json").await?;

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
1. Creates `ListingOptions` with `JsonFormat` (no file extension filter needed)
2. Parses the URL as `ListingTableUrl` (supports `file://`, `s3://`, `gs://`, etc.)
3. Creates `ListingTableConfig` and calls `infer_schema()` to compute schema
4. Creates `ListingTable` with the inferred schema
5. Returns an `Arc<dyn TableProvider>` ready for registration

Key implementation details:
- Uses `ListingTableConfig::infer_schema()` with session state
- Schema is inferred once and cached in the `ListingTable`
- Files should have `.json` extension for DataFusion's default file discovery
- No explicit file extension filter needed when files follow convention
- Accepts full URLs with schemes rather than plain file paths

## Benefits

1. **Extensible**: Users can implement the trait to add any data source
2. **Code-Driven**: No magic auto-discovery, explicit registration
3. **Type-Safe**: Trait-based approach with compile-time checks
4. **Flexible**: Support JSON, CSV, Parquet, or any DataFusion TableProvider
5. **SQL Integration**: Join custom tables with telemetry data
6. **Development Aid**: Useful for testing and prototyping
7. **View Integration**: `SqlBatchView` and `ExportLogView` can access custom tables in their queries

## Key Feature: Views Can Access Custom Tables

Both `SqlBatchView` and `ExportLogView` now accept a `SessionConfigurator` parameter:

```rust
// Create a configurator with custom tables
let configurator = Arc::new(MyJsonConfigurator::new().await?);

// SqlBatchView can now query custom tables in count_src_query, extract_query, and merge_partitions_query
let batch_view = SqlBatchView::new(
    runtime,
    view_set_name,
    min_event_time_column,
    max_event_time_column,
    count_src_query,  // Can reference custom JSON tables!
    extract_query,    // Can reference custom JSON tables!
    merge_partitions_query,  // Can reference custom JSON tables!
    lake,
    view_factory,
    configurator,  // Pass custom configurator
    update_group,
    max_partition_delta_from_source,
    max_partition_delta_from_merge,
    merger_maker,
).await?;

// ExportLogView can now query custom tables in count_src_query and extract_query
let export_view = ExportLogView::new(
    runtime,
    view_set_name,
    count_src_query,   // Can reference custom JSON tables!
    extract_query,     // Can reference custom JSON tables!
    exporter,
    lake,
    view_factory,
    configurator,  // Pass custom configurator
    update_group,
    max_partition_delta_from_source,
    max_partition_delta_from_merge,
).await?;
```

This enables powerful use cases like:
- Joining telemetry data with reference data from JSON files
- Filtering telemetry based on configuration tables
- Enriching log exports with lookup tables

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
