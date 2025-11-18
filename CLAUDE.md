# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Critical Rules
- **NEVER COMMIT UNLESS EXPLICITLY ASKED**
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
- **Test**: `poetry run pytest`
- **Format**: `poetry run black <file>` (REQUIRED before commit)

### Grafana Plugin (from `grafana/` directory)
- **IMPORTANT**: Use `yarn`, NOT `npm` (project uses yarn as package manager)
- **Install**: `yarn install`
- **Build**: `yarn build`
- **Dev build**: `yarn dev`
- **Lint**: `yarn lint:fix` (REQUIRED before commit)
- **Test server**: `yarn server` (starts local Grafana with plugin)

### Analytics Web App (from `analytics-web-app/` directory)
- **IMPORTANT**: Use `yarn`, NOT `npm` (project uses yarn as package manager)
- **Install**: `yarn install`
- **Dev**: `yarn dev` (starts Next.js dev server on port 3000)
- **Build**: `yarn build` (production build)
- **Lint**: `yarn lint` (REQUIRED before commit)
- **Type check**: `yarn type-check`
- **Test**: `yarn test`
- **Quick start**: `./start_analytics_web.py` (starts both backend and frontend)
- **Backend**: `cd rust && cargo run --bin analytics-web-srv` (runs on port 8000)

### Service Management (for testing and development)
- **Start Services**: `python3 local_test_env/ai_scripts/start_services.py`
  - Starts PostgreSQL, telemetry-ingestion-srv (port 9000), flight-sql-srv (port 50051), and telemetry-admin
  - Services run in background with logs in `/tmp/`
  - PIDs saved to `/tmp/micromegas_pids.txt`
- **Stop Services**: `python3 local_test_env/ai_scripts/stop_services.py`
  - Stops all services and cleans up log files
- **Service Logs**: 
  - Ingestion: `tail -f /tmp/ingestion.log`
  - Analytics: `tail -f /tmp/analytics.log` 
  - Admin: `tail -f /tmp/admin.log`
- **Service URLs**:
  - Ingestion server: http://127.0.0.1:9000
  - Analytics server: flight-sql port 50051 (no HTTP endpoint)

## Architecture

Micromegas: unified observability platform for logs, metrics, and traces.

**Core crates**: `tracing/` (instrumentation), `analytics/` (DataFusion queries), `public/` (user-facing)
**Services**: `telemetry-ingestion-srv/` (HTTP ingestion), `flight-sql-srv/` (SQL queries)
**Flow**: Apps → HTTP ingestion → PostgreSQL metadata + object storage → FlightSQL queries
- there should be a venv available to run python code
- follow @AI_GUIDELINES.md
- prefer to script using python over shell scripts
- never commit if I did not ask explicitly