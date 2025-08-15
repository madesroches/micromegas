# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Critical Rules
- **Commit Messages**: NEVER include AI-generated credits or co-author tags
- **Pull Requests**: Always run `git log --oneline main..HEAD` before creating PRs
- **Project Structure**: Run cargo commands from `rust/` directory (main workspace at `rust/Cargo.toml`)

## Code Style

### Rust
- Dependencies in alphabetical order in Cargo.toml files
- Use `expect()` with descriptive messages instead of `unwrap()`
- Run `cargo fmt` before any commit
- Use inline format arguments: `format!("value: {variable}")`
- Import proc macros through parent crate: `micromegas_tracing::prelude::*`
- Always use `prelude::*` when importing from prelude modules

### General
- Follow existing code conventions and patterns
- Check for existing libraries/frameworks before assuming availability
- Never expose secrets or keys
- Use Unix line endings (LF) in all files
- Always run tests after making changes

## Essential Commands

### Rust (from `rust/` directory)
- **Build**: `cargo build`
- **Test**: `cargo test` (use `-- --nocapture` to see println! output)
- **Format**: `cargo fmt` (REQUIRED before commit)
- **Lint**: `cargo clippy --workspace -- -D warnings`
- **CI**: `python3 ../build/rust_ci.py`

### Python (from `python/micromegas/` directory)
- **Install**: `poetry install`
- **Test**: `pytest`
- **Format**: `black <file>` (REQUIRED before commit)

## Architecture

Micromegas: unified observability platform for logs, metrics, and traces.

**Core crates**: `tracing/` (instrumentation), `analytics/` (DataFusion queries), `public/` (user-facing)
**Services**: `telemetry-ingestion-srv/` (HTTP ingestion), `flight-sql-srv/` (SQL queries)
**Flow**: Apps → HTTP ingestion → PostgreSQL metadata + object storage → FlightSQL queries