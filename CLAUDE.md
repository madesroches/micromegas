# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

For an architecture overview (core crates, services, data flow), see `.github/copilot-instructions.md`.

## Critical Rules
- **NEVER push without a direct, unambiguous instruction** — this includes `git push`, `git push --force`, and creating PRs. Local commits are fine; publishing them is not.
- **NEVER commit directly to `main`** — always work on a branch.
- **NEVER dismiss Dependabot alerts** — leave them open until fixed by code/dependency changes

## Source control rules
- **Local commits on branches are allowed** without asking — useful as rollback points during iterative/looped work. Keep messages short and scoped.
- **Pushing requires an explicit, direct instruction** ("push", "open a PR", "publish"). Ambiguous phrasing ("ship it", "you can wrap up") does not count — ask if unsure.
- **Commit Messages**: NEVER include AI-generated credits or co-author tags
- **Pull Requests**: Always run `git log --oneline main..HEAD` before creating PRs
- unless asked, don't amend commits

## Interface stability

**The SQL layer is the stable interface. Rust APIs are not.**

- **SQL surface — keep it compatible.** View and table schemas (column names, types, and
  order), view/table names, and UDF/UDTF signatures and their result columns are what users
  build dashboards and saved queries on. Additive changes are fine: append a new column
  **last** so `SELECT *` and positional readers keep working. Renaming or removing a column,
  reordering existing ones, or changing a type breaks dashboards silently — avoid it, or stage
  it deliberately.
- **Rust API surface — change it freely.** Making a private item `pub`, widening a function
  signature, adding a struct field, or altering a trait is all acceptable. The project is still
  niche, so a clean design beats a compatible one, and contorting a design to spare a handful of
  call sites costs more than the break does. Prefer the shape that makes the compiler enumerate
  every affected call site — a silently defaulted value is the more expensive failure. Record
  the break in `CHANGELOG.md` with its **Minor breaking change** clause; don't design around it.
- An internal `SCHEMA_VERSION` bump is **not** a SQL break — it changes the partition
  file-schema hash to force a rebuild, while the queryable Arrow schema stays identical.
- Data and wire formats are separate from both: stored payloads and partition metadata still
  need their migration / `SCHEMA_VERSION` handling.

## Scripting
- prefer to script using python over shell scripts
- use the poetry venv in python/micromegas run python code

## Code Style
- Use Unix line endings (LF) in all files

## Essential Commands

### Service Management (for testing and development)
- **Start Services** (split mode): `python3 local_test_env/ai_scripts/start_services.py`
  - Starts PostgreSQL, telemetry-ingestion-srv (port 9000), flight-sql-srv (port 50051), and telemetry-maintenance-srv
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
  - Maintenance: `tail -f /tmp/daemon.log`
  - Monolith: `tail -f /tmp/monolith.log`
- **Service URLs**:
  - Ingestion server: http://127.0.0.1:9000
  - Analytics server: flight-sql port 50051 (no HTTP endpoint)
  - Web app (monolith): http://127.0.0.1:3000

### SQL Query CLI
- **Query**: `micromegas-query "SELECT * FROM list_partitions() LIMIT 5"`
  - Installed via `pip install micromegas` (or `poetry install` in dev)
  - Use this tool to run arbitrary SQL queries against the analytics service
  - Accepts optional `--begin` and `--end` for time range (relative like `1h`, `24h`, `7d` or RFC 3339 like `2024-01-01T00:00:00Z`)
  - Accepts `--format` for output: `table` (default), `csv`, `json`
  - Accepts `--profile` to select a named connection profile from `~/.micromegas/config.json`
  - Example: `micromegas-query "SELECT time, level, msg FROM log_entries LIMIT 10" --begin 1h --format csv`
- **Logout**: `micromegas-logout` clears every cached token file (`tokens.json` plus all `tokens-<profile>.json`); pass `--profile <name>` to clear only that profile's token

## Branding

Logo and color scheme assets are in the `branding/` folder:
- **Brand sheet**: `micromegas-brand-sheet.svg` (full reference with color palette)
- **Logos**: horizontal, vertical, icon variants for dark/light backgrounds
- **Colors**: Rust orange (#bf360c), Blue (#1565c0), Wheat (#ffb300), Dark bg (#0a0a0f)

## Other

- Unreal Engine integration is available in the `unreal/` directory.
