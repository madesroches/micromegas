# Copilot Instructions for Micromegas

Micromegas is a unified observability platform for logs, metrics, and traces built for high-volume environments (100k+ events/second).

## Architecture Overview

### Core Components
- **`rust/tracing/`**: High-performance instrumentation library (20ns overhead per event)
- **`rust/analytics/`**: DataFusion-powered analytics engine with lakehouse storage
- **`rust/telemetry-ingestion-srv/`**: HTTP service for telemetry ingestion (port 9000)
- **`rust/flight-sql-srv/`**: Apache Arrow FlightSQL service for queries (port 50051)
- **`rust/telemetry-sink/`**: Event sinks for sending data to ingestion services
- **`rust/public/`**: Main user-facing crate combining all components

### Data Flow Pattern
1. Applications use `micromegas-tracing` to emit telemetry
2. Data flows to ingestion service via HTTP
3. Metadata stored in PostgreSQL, payloads in object storage (Parquet)
4. Analytics service provides SQL queries via FlightSQL

## Development Conventions

### Project Structure
- **Main workspace**: `rust/Cargo.toml` - run all cargo commands from `rust/` directory
- **Dependencies**: Must be alphabetically ordered in all Cargo.toml files
- **Workspace deps**: Add to root Cargo.toml, reference in member crates

### Rust Code Patterns
- **Error handling**: Use `expect("descriptive message")` instead of `unwrap()`
- **Format strings**: Use inline args `format!("value: {variable}")` not `format!("value: {}", variable)`
- **Proc macros**: Import through parent crate `micromegas_tracing::prelude::*`, not directly
- **Prelude imports**: Always use `prelude::*` when importing from prelude modules

### Async Instrumentation Patterns
```rust
// Use proc macros for automatic instrumentation
#[micromegas_main]  // Drop-in replacement for tokio::main
async fn main() -> Result<()> { ... }

#[span_fn]  // Works for both sync and async functions
async fn process_data() -> Result<()> { ... }

// Manual async instrumentation
use micromegas_tracing::prelude::*;
some_future.instrument(&static_span_desc!("operation_name")).await
```

### Essential Commands

#### Development (from `rust/` directory)
- **Build**: `cargo build`
- **Test**: `cargo test` (use `-- --nocapture` for println! output)
- **Format**: `cargo fmt` (REQUIRED before commit)
- **Lint**: `cargo clippy --workspace -- -D warnings`
- **CI Pipeline**: `python3 ../build/rust_ci.py`

#### Service Management (for testing)
- **Start all services**: `python3 local_test_env/ai_scripts/start_services.py`
  - PostgreSQL + ingestion-srv (9000) + flight-sql-srv (50051) + admin CLI
  - Logs in `/tmp/ingestion.log` and `/tmp/analytics.log`
- **Stop all services**: `python3 local_test_env/ai_scripts/stop_services.py`

### Critical Rules
- **Commits**: NEVER include AI-generated credits or co-author tags
- **PRs**: Run `git log --oneline main..HEAD` before creating PRs
- **Line endings**: Always use Unix (LF) line endings
- **Dependencies**: Keep alphabetical order in Cargo.toml files

### Performance Considerations
- Thread-local storage minimizes instrumentation overhead
- High-frequency telemetry designed for 100k+ events/second
- Async span tracking uses `InstrumentedFuture` wrapper for zero-cost abstractions
- Cost-efficient storage: metadata in PostgreSQL, bulk data in object storage

### Environment Variables (services)
- `MICROMEGAS_SQL_CONNECTION_STRING`: PostgreSQL connection
- `MICROMEGAS_OBJECT_STORE_URI`: S3/GCS bucket for payload storage

### Testing Strategy
- Use `serial_test` crate for tests requiring exclusive access to global state
- Test async instrumentation with tokio time manipulation
- Run full CI with `python3 ../build/rust_ci.py` before major changes
