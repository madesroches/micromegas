# CLI Entry Points & Cleanup Plan

## Overview

Make `micromegas-query` and `micromegas-logout` available as installed CLI commands via `pip install micromegas`, and delete the legacy single-purpose CLI scripts that are superseded by the general-purpose `query.py`.

## Current State

The `python/micromegas/cli/` directory contains:

- **`connection.py`** — shared authentication helper used by CLI scripts
- **`query.py`** — general-purpose SQL query tool (bare `import connection`)
- **`logout.py`** — clears OIDC tokens (already registered as `micromegas_logout`)
- **`query_processes.py`** — lists processes (bare `import connection`)
- **`query_process_log.py`** — queries logs for a process (bare `import connection`)
- **`query_process_metrics.py`** — queries metrics for a process (bare `import connection`)
- **`write_perfetto.py`** — exports Perfetto traces (try/except import)
- **`__init__.py`** — package marker

`pyproject.toml` has one entry point today:
```toml
[tool.poetry.scripts]
micromegas_logout = "micromegas.cli.logout:main"
```

The four legacy scripts are not registered as entry points. Their functionality is covered by `query.py` which can run arbitrary SQL.

### Documentation references to update

- `mkdocs/docs/query-guide/python-api.md:596-604` — "Other CLI Tools" section lists all four scripts
- `doc/unreal-observability/unreal-observability.md:159,174` — shows `query_processes.py` usage examples

## Implementation Steps

### 1. Fix import in `query.py`

Change `import connection` to `from micromegas.cli import connection` so it works as an installed package entry point (`query.py:3`).

### 2. Update entry points in `pyproject.toml`

Replace the scripts section with:
```toml
[tool.poetry.scripts]
micromegas-logout = "micromegas.cli.logout:main"
micromegas-query = "micromegas.cli.query:main"
```

Note: changing `micromegas_logout` → `micromegas-logout` for consistency (hyphens). Poetry/pip supports both; hyphens are the conventional style for CLI commands.

### 3. Delete legacy scripts

Remove these files:
- `python/micromegas/cli/query_processes.py`
- `python/micromegas/cli/query_process_log.py`
- `python/micromegas/cli/query_process_metrics.py`
- `python/micromegas/cli/write_perfetto.py`

### 4. Update documentation

**`mkdocs/docs/query-guide/python-api.md`** — Replace the "Other CLI Tools" section (lines 596-604) to document the two installed commands:
- `micromegas-query` with usage examples
- `micromegas-logout`

Remove references to deleted scripts.

**`doc/unreal-observability/unreal-observability.md`** — Replace `query_processes.py` examples (lines 159, 174) with equivalent `micromegas-query` usage.

### 5. Update CLAUDE.md

Update the "SQL Query CLI" section to reflect the new command name (`micromegas-query` instead of `poetry run python query.py`).

## Files to Modify

- `python/micromegas/cli/query.py` — fix import
- `python/micromegas/pyproject.toml` — update entry points

## Files to Delete

- `python/micromegas/cli/query_processes.py`
- `python/micromegas/cli/query_process_log.py`
- `python/micromegas/cli/query_process_metrics.py`
- `python/micromegas/cli/write_perfetto.py`

## Files to Update (docs)

- `mkdocs/docs/query-guide/python-api.md`
- `doc/unreal-observability/unreal-observability.md`
- `CLAUDE.md`

## Testing Strategy

- `cd python/micromegas && poetry install` — verify entry points are created
- `poetry run micromegas-query --help` — verify the command works
- `poetry run micromegas-logout --help` — verify the command works
- `poetry run pytest` — ensure no tests break
