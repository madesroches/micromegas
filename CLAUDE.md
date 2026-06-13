# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Critical Rules
- **NEVER push without a direct, unambiguous instruction** — this includes `git push`, `git push --force`, and creating PRs. Local commits are fine; publishing them is not.
- **NEVER commit directly to `main`** — always work on a branch.
- **NEVER dismiss Dependabot alerts** — leave them open until fixed by code/dependency changes
- follow @AI_GUIDELINES.md
- **Project Structure**: Run cargo commands from `rust/` directory (main workspace at `rust/Cargo.toml`)

## Source control rules
- **Local commits on branches are allowed** without asking — useful as rollback points during iterative/looped work. Keep messages short and scoped.
- **Pushing requires an explicit, direct instruction** ("push", "open a PR", "publish"). Ambiguous phrasing ("ship it", "you can wrap up") does not count — ask if unsure.
- **Commit Messages**: NEVER include AI-generated credits or co-author tags
- **Pull Requests**: Always run `git log --oneline main..HEAD` before creating PRs
- unless asked, don't amend commits

## Scripting
- prefer to script using python over shell scripts
- use the poetry venv in python/micromegas run python code


## Code Style

### Rust
- Dependencies in alphabetical order in Cargo.toml files
- Use `expect()` with descriptive messages instead of `unwrap()`
- Run `cargo fmt` before any commit
- Use inline format arguments: `format!("value: {variable}")`
- Import proc macros through parent crate: `micromegas_tracing::prelude::*`
- Always use `prelude::*` when importing from prelude modules
- Unit tests should not be with the lib implementation, unit tests should be under the tests folder of the crate

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
- **IMPORTANT**: Use `yarn`, NOT `npm` (project uses Yarn 4 / Berry via corepack — run `corepack enable` once on a new machine)
- **Install**: `yarn install`
- **Build**: `yarn build`
- **Dev build**: `yarn dev`
- **Lint**: `yarn lint:fix` (REQUIRED before commit)
- **Test server**: `yarn server` (starts local Grafana with plugin)

### Analytics Web App (from `analytics-web-app/` directory)
- **IMPORTANT**: Use `yarn`, NOT `npm` (project uses Yarn 4 / Berry via corepack — run `corepack enable` once on a new machine)
- **Install**: `yarn install`
- **Dev**: `yarn dev` (starts Vite dev server on port 3000)
- **Build**: `yarn build` (production build to `dist/`)
- **Lint**: `yarn lint` (REQUIRED before commit)
- **Type check**: `yarn type-check`
- **Test**: `yarn test`
- **Quick start**: `./start_analytics_web.py` (starts both backend and frontend)
- **Backend**: `cd rust && cargo run --bin analytics-web-srv` (runs on port 8000)

### Service Management (for testing and development)
- **Start Services** (split mode): `python3 local_test_env/ai_scripts/start_services.py`
  - Starts PostgreSQL, telemetry-ingestion-srv (port 9000), flight-sql-srv (port 50051), and telemetry-admin
  - Services run in background with logs in `/tmp/`
  - PIDs saved to `/tmp/micromegas_pids.txt`
- **Start Services** (monolith mode): `python3 local_test_env/ai_scripts/start_services.py --monolith`
  - Starts PostgreSQL + single `micromegas-monolith` process (ports 9000, 50051, 3000)
  - Logs in `/tmp/monolith.log`
- **Stop Services**: `python3 local_test_env/ai_scripts/stop_services.py`
  - Stops all services and cleans up log files
- **Run monolith directly** (from `rust/`):
  ```
  cargo run --bin micromegas-monolith -- \
    --roles all \
    --listen-endpoint-http 127.0.0.1:9000 \
    --frontend-dir ../analytics-web-app/dist \
    --disable-auth
  ```
- **Service Logs**:
  - Ingestion: `tail -f /tmp/ingestion.log`
  - Analytics: `tail -f /tmp/analytics.log`
  - Admin: `tail -f /tmp/admin.log`
  - Monolith: `tail -f /tmp/monolith.log`
- **Service URLs**:
  - Ingestion server: http://127.0.0.1:9000
  - Analytics server: flight-sql port 50051 (no HTTP endpoint)
  - Web app (monolith): http://127.0.0.1:3000

### SQL Query CLI
- **Query**: `micromegas-query "SELECT * FROM list_partitions() LIMIT 5"`
  - Installed via `pip install micromegas` (or `poetry install` in dev)
  - Use this tool to run arbitrary SQL queries against the analytics service
  - Accepts optional `--begin` and `--end` for time range (relative like `1h`, `24h`, `7d` or ISO format)
  - Accepts `--format` for output: `table` (default), `csv`, `json`
  - Example: `micromegas-query "SELECT time, level, msg FROM log_entries LIMIT 10" --begin 1h --format csv`
- **Logout**: `micromegas-logout` (clears cached OIDC tokens)

## Branding

Logo and color scheme assets are in the `branding/` folder:
- **Brand sheet**: `micromegas-brand-sheet.svg` (full reference with color palette)
- **Logos**: horizontal, vertical, icon variants for dark/light backgrounds
- **Colors**: Rust orange (#bf360c), Blue (#1565c0), Wheat (#ffb300), Dark bg (#0a0a0f)

### Documentation Site (from `mkdocs/` directory)
- **Install**: `python -m venv venv && source venv/bin/activate && pip install -r docs-requirements.txt`
- **Serve locally**: `python serve.py` (live-reload dev server)
- **Build**: `python build-docs.py`
- **Docs source**: `mkdocs/docs/` (Markdown files)
- **Config**: `mkdocs/mkdocs.yml` (navigation, plugins, theme)

## Architecture

Micromegas: unified observability platform for logs, metrics, and traces.

**Core crates**: `tracing/` (instrumentation), `analytics/` (DataFusion queries), `public/` (user-facing)
**Services**: `telemetry-ingestion-srv/` (HTTP ingestion), `flight-sql-srv/` (SQL queries)
**Flow**: Apps → HTTP ingestion → PostgreSQL metadata + object storage → FlightSQL queries
