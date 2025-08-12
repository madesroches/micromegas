# AI Assistant Guidelines

## Code Style and Conventions

### Rust
- **Dependencies**: Always maintain alphabetical order within dependency blocks in Cargo.toml files
- **Error Handling**: Use `expect()` with descriptive messages instead of `unwrap()`
- **Testing**: Use `cargo test -- --nocapture` to see println! output during tests
- **Formatting**: Always run `cargo fmt` before any commit to ensure consistent code formatting
- **Format Strings**: Use inline format arguments `format!("value: {variable}")` instead of `format!("value: {}", variable)`
- **Proc Macros**: Use proc macros through their parent crate (e.g., `micromegas_tracing::prelude::*`) rather than importing proc macro crates directly
- **Prelude Imports**: Always use `prelude::*` when importing from a prelude module

### General
- Follow existing code conventions and patterns in the codebase
- Check for existing libraries/frameworks before assuming availability
- Maintain security best practices - never expose secrets or keys
- Use existing utilities and patterns found in neighboring files
- Keep commit messages short
- **Commit Messages**: NEVER include AI-generated credits or co-author tags in commit messages
- **Pull Requests**: Always run `git log --oneline main..HEAD` before creating PRs to list all commits in the branch

## Project Structure
- Main Cargo.toml is located at `rust/Cargo.toml`
- Run cargo commands from the `rust/` directory
- Workspace dependencies should be added to the root Cargo.toml

## Testing
- Always run tests after making changes to verify functionality

## Essential Commands

### Rust Development (from `rust/` directory)
- **Build**: `cargo build`
- **Test**: `cargo test` (use `cargo test -- --nocapture` to see println! output)
- **Format**: `cargo fmt` (REQUIRED before any commit)
- **Lint**: `cargo clippy --workspace -- -D warnings`
- **CI Pipeline**: `python3 ../build/rust_ci.py` (runs format check, clippy, and tests)

### Python Development (from `python/micromegas/` directory)
- **Install dependencies**: `poetry install`
- **Test**: `pytest`

## Architecture Overview

Micromegas is a unified observability platform with these core components:

### Core Rust Crates (rust/)
- **`tracing/`**: High-performance instrumentation library (20ns overhead per event)
  - Supports logs, metrics, spans with async futures instrumentation
  - Uses proc macros: `#[micromegas_main]`, `#[span_fn]`
  - Thread-local storage for minimal performance impact
- **`analytics/`**: DataFusion-powered analytics engine for the data lake
  - Lakehouse module for materialized views and partitioned storage
  - Arrow-based data processing and transformations
- **`telemetry-sink/`**: Event sinks for sending data to ingestion services
- **`ingestion/`**: Database and object store integration for data persistence
- **`public/`**: Main user-facing crate combining all components

### Services
- **`telemetry-ingestion-srv/`**: HTTP service accepting telemetry data, stores metadata in PostgreSQL and payloads in object storage (S3/GCS)
- **`flight-sql-srv/`**: Apache Arrow FlightSQL service for querying data using DataFusion
- **`telemetry-admin-cli/`**: Administrative CLI tool

### Data Flow
1. Applications use `micromegas-tracing` to emit telemetry (logs/metrics/spans)
2. Data flows to ingestion service via HTTP
3. Metadata stored in PostgreSQL, raw payloads in object storage (Parquet format)
4. Analytics service provides SQL queries via FlightSQL
5. Materialized views optimize frequent queries

### Key Design Principles
- **High-frequency data collection**: Up to 100k events/second per process
- **Cost-efficient storage**: Raw data in cheap object storage, metadata in PostgreSQL
- **On-demand processing**: ETL only when querying data
- **Unified observability**: Logs, metrics, and traces in single queryable format

## Development Notes

- Main workspace root is `rust/Cargo.toml` - run cargo commands from `rust/` directory
- Dependencies must be alphabetically ordered in Cargo.toml files
- Use `expect()` with descriptive messages instead of `unwrap()`
- Import proc macros through parent crates (e.g., `micromegas_tracing::prelude::*`)
- Python client available in `python/micromegas/` using Poetry for dependency management
- Unreal Engine integration available in `unreal/` directory

## Environment Variables (for services)
- `MICROMEGAS_SQL_CONNECTION_STRING`: PostgreSQL connection
- `MICROMEGAS_OBJECT_STORE_URI`: S3/GCS bucket URI for payload storage
