# Notebooks as Code Plan

## Overview

Add CLI tooling to manage micromegas web notebooks (screens) as files in git repositories. PostgreSQL remains the default and only runtime storage — this is a purely client-side sync tool. Users opt in per-notebook by pulling it to disk, editing it, and pushing it back.

## Current State

### Storage
- Notebooks live in the `screens` PG table: `name` (PK), `screen_type`, `config` (JSONB), `created_by`, `updated_by`, `created_at`, `updated_at`
- `config` is an opaque JSONB blob containing cells, time range, and notebook-specific fields
- Names are normalized: lowercase, hyphens, 3-100 chars (`rust/analytics-web-srv/src/app_db/models.rs:119-144`)

### REST API
- CRUD at `/api/screens/{name}` — the web app uses cookie-based OIDC auth (`analytics-web-app/src/lib/api.ts`)
- `GET /api/screens` returns all screens; `GET /api/screens/{name}` returns one
- `POST /api/screens` creates; `PUT /api/screens/{name}` updates config; `DELETE /api/screens/{name}` deletes

### Existing Import/Export
- Web UI export produces a single JSON file with `{version, exported_at, screens[]}` (`analytics-web-app/src/lib/screens-api.ts:175-186`)
- Import supports skip/overwrite/rename conflict strategies
- This is a manual, browser-based operation — no CLI equivalent exists

### Python CLI
- Entry points in `python/micromegas/pyproject.toml`: `micromegas-query`, `micromegas-logout`
- CLI modules in `python/micromegas/micromegas/cli/`
- Auth via OIDC (`micromegas/auth/oidc.py`) — supports browser login and client credentials
- The Python client only speaks FlightSQL (gRPC). There is **no HTTP client** for the analytics-web-srv REST API

## Prerequisites

### Bearer Token Auth for analytics-web-srv

The web server currently only accepts authentication via `id_token` httpOnly cookies set during the browser OIDC flow (`rust/analytics-web-srv/src/auth.rs:785-834`). The `cookie_auth_middleware` reads the token from the cookie jar and wraps it in a `CookieTokenRequestParts` adapter that hardcodes `authorization_header()` to `None`.

However, the underlying auth infrastructure already supports Bearer tokens:
- `micromegas_auth::types::HttpRequestParts` correctly extracts `Authorization: Bearer <token>` headers (`rust/auth/src/types.rs:70-88`)
- `OidcAuthProvider::validate_request()` works with any `RequestParts` implementation
- The Python OIDC providers already produce ID tokens suitable for Bearer auth

**Required change:** Modify `cookie_auth_middleware` to check the `Authorization: Bearer` header first, then fall back to the cookie. This is a small change — create a new `RequestParts` adapter that checks the header first:

```rust
// In cookie_auth_middleware, before extracting from cookie:
let token = if let Some(bearer) = req.headers()
    .get(header::AUTHORIZATION)
    .and_then(|h| h.to_str().ok())
    .and_then(|h| h.strip_prefix("Bearer "))
{
    bearer.to_string()
} else {
    // Fall back to cookie
    jar.get(ID_TOKEN_COOKIE)
        .ok_or(AuthApiError::Unauthorized)?
        .value()
        .to_string()
};
```

This enables CLI tools and CI pipelines to authenticate using the same OIDC tokens without a browser session.

## Design

### File Format: JSON, One File Per Notebook

Each notebook is a single `.json` file containing:

```json
{
  "name": "my-notebook",
  "screen_type": "notebook",
  "config": {
    "timeRangeFrom": "now-5m",
    "timeRangeTo": "now",
    "cells": [ ... ]
  }
}
```

**Why JSON over YAML:**
- The config is already JSON in PG and in the existing export format
- No lossy round-trip — what you pull is what you push
- The web app's "Source View" (`NotebookSourceView.tsx`) already shows raw JSON
- JSON diffs well enough when pretty-printed with sorted keys
- No new dependency (YAML parser) needed

**Formatting for git-friendliness:**
- Pretty-printed with 2-space indent (`json.dumps(obj, indent=2, sort_keys=False)`)
- Keys within `config` are NOT sorted (cell order matters), but top-level keys are in a fixed order: `name`, `screen_type`, `config`
- Trailing newline

**What's excluded from the file:**
- `created_by`, `updated_by`, `created_at`, `updated_at` — these are server-side metadata, not part of the notebook content. They'd create noisy diffs on every pull.

### Directory Structure

```
notebooks/           # default directory, configurable
  my-notebook.json
  performance-dashboard.json
  ...
```

- Filename = notebook name + `.json` (the name field inside must match)
- Flat structure — no subdirectories (notebook names are already unique, flat identifiers)
- The directory can live anywhere in the repo; it's just a convention

**No manifest file.** The set of git-managed notebooks is simply "whatever `.json` files are in the directory." This keeps things simple and avoids sync issues between a manifest and the actual files.

### Source-Control Tracking

When a notebook is managed via git, users editing it in the web UI need to know their changes may be overwritten on the next CI push. This requires the server to know which notebooks are tracked.

**Approach:** Add a `source_url` column to the `screens` table:

```sql
ALTER TABLE screens ADD COLUMN source_url VARCHAR(1024) DEFAULT NULL;
```

- `NULL` = not tracked, normal web-editable notebook (default — preserves current behavior)
- A URL = managed by source control, pointing to the file in the repo's web interface (e.g., `https://github.com/org/repo/blob/main/notebooks/my-notebook.json`)

This is more useful than a boolean flag — the user can click through to see the canonical version, file history, and who changed it.

**CLI flag:**
```
micromegas-notebooks push --all --source-url-template "https://github.com/org/repo/blob/main/notebooks/{name}.json"
```

The `{name}` placeholder is replaced with the notebook name. This can also be set via `MICROMEGAS_NOTEBOOKS_SOURCE_URL_TEMPLATE` env var so CI pipelines set it once.

**When is it set?**
- `push` sets `source_url` on every notebook it creates or updates
- `pull` does NOT change it (pull is read-only from the server's perspective)
- Manual edits via the web UI do NOT clear it (the flag reflects the intended management mode, not the last editor)

**Web UI behavior when `source_url` is set:**
- Show a persistent banner at the top of the notebook: *"This notebook is managed by source control. Edits made here may be overwritten on the next deployment."* with a link: *"View source →"* pointing to `source_url`
- The notebook remains fully editable (users may need to make quick fixes)
- The banner uses a warning style (yellow/amber) — informational, not blocking

**API changes:**
- `GET /api/screens` and `GET /api/screens/{name}` include `source_url` in the response
- `PUT /api/screens/{name}` accepts an optional `source_url` field in the request body
- The CLI sets `source_url` when pushing; the web UI does not send it (so it stays unchanged)

**Multi-repo support:** Multiple git repos can each manage a different subset of notebooks. The `source_url` template naturally scopes ownership — each repo pushes a distinct URL pattern.

**Delete safety:** When `push --all` runs, it only deletes server notebooks whose `source_url` matches the current `--source-url-template` pattern AND that don't have a corresponding local file. Notebooks managed by a different repo (different URL pattern) or with `source_url = NULL` are never touched. Matching is done by checking if the notebook's `source_url` starts with the template's prefix (everything before `{name}`).

### CLI Commands

Add a new entry point `micromegas-notebooks` with subcommands:

```
micromegas-notebooks pull [--all | NAMES...] [--dir DIR] [--server URL]
micromegas-notebooks push [--all | NAMES...] [--dir DIR] [--server URL] [--create-missing]
micromegas-notebooks diff [NAMES...] [--dir DIR] [--server URL]
micromegas-notebooks list [--dir DIR] [--server URL]
```

#### `pull` — Download notebooks from server to disk

```
micromegas-notebooks pull my-notebook performance-dashboard --dir ./notebooks
micromegas-notebooks pull --all --dir ./notebooks
```

- Fetches from REST API, writes pretty-printed JSON to `DIR/NAME.json`
- `--all`: pull every notebook from the server
- Named args: pull only the specified notebooks
- If file already exists, overwrites it (this is intentional — you can use `git diff` to see what changed)
- Prints a summary: created/updated/unchanged counts

#### `push` — Sync notebooks from disk to server

```
micromegas-notebooks push my-notebook --dir ./notebooks
micromegas-notebooks push --all --dir ./notebooks
```

- Reads `DIR/NAME.json`, calls PUT (update) or POST (create) on the server
- `--all`: push every `.json` file in the directory, and **delete** server notebooks that are tracked but no longer have a local file (see Tracking below)
- Creates notebooks that exist locally but not on the server
- Validates the JSON structure before pushing
- Prints a summary: created/updated/deleted/unchanged/error counts

#### `diff` — Show differences between disk and server

```
micromegas-notebooks diff --dir ./notebooks
micromegas-notebooks diff my-notebook --dir ./notebooks
```

- Fetches current server state, compares with local files
- Shows: notebooks only on disk, only on server, or modified
- For modified notebooks, shows a unified diff of the JSON
- Useful before push to preview what would change

#### `list` — Show notebook inventory

```
micromegas-notebooks list --dir ./notebooks
```

- Lists notebooks from server and local directory side by side
- Shows sync status: `synced`, `local-only`, `server-only`, `modified`

#### Common Options

| Flag | Default | Description |
|------|---------|-------------|
| `--dir` | `./notebooks` | Local directory for notebook files |
| `--server` | `$MICROMEGAS_WEB_URL` or `http://localhost:8000` | analytics-web-srv URL |
| `--source-url-template` | `$MICROMEGAS_NOTEBOOKS_SOURCE_URL_TEMPLATE` | URL template with `{name}` placeholder (push only) |
| `--format` | `table` | Output format: `table`, `json` |

### HTTP Client for analytics-web-srv

The Python package needs a new HTTP client module since it currently only has FlightSQL/gRPC.

**New module:** `python/micromegas/micromegas/web_client.py`

```python
class WebClient:
    """HTTP client for analytics-web-srv REST API."""

    def __init__(self, base_url: str, auth_provider=None):
        self.base_url = base_url.rstrip("/")
        self.auth_provider = auth_provider

    def list_screens(self) -> list[dict]: ...
    def get_screen(self, name: str) -> dict: ...
    def create_screen(self, name: str, screen_type: str, config: dict, source_url: str | None = None) -> dict: ...
    def update_screen(self, name: str, config: dict, source_url: str | None = None) -> dict: ...
    def delete_screen(self, name: str) -> None: ...
```

**Auth:** Reuses the existing OIDC auth provider (`micromegas/auth/oidc.py`). The `get_token()` method returns an ID token that can be sent as `Authorization: Bearer <token>` header. This requires the bearer token auth prerequisite (see above).

### Conflict Handling: Keep It Simple

This tool does NOT attempt automatic conflict resolution. The workflow is:

1. `pull` always overwrites local files (server wins)
2. `push` always overwrites server state (local wins)
3. `diff` shows what would change before you push

**Why no automatic merge:** Notebook configs are structured JSON with ordered arrays (cells). Merging cell arrays automatically is error-prone. Git handles text-level conflicts well enough. The intended workflow is:

```
# Initial setup
micromegas-notebooks pull --all --dir ./notebooks
git add notebooks/
git commit -m "import notebooks"

# Edit cycle
# 1. Edit in web UI or in text editor
# 2. Pull latest from server
micromegas-notebooks pull my-notebook
# 3. Review changes
git diff notebooks/my-notebook.json
# 4. Commit
git commit -am "update my-notebook"

# Deploy / restore cycle
# 1. Push from git to server (e.g., in CI)
# Creates/updates all local notebooks, deletes tracked notebooks removed from git
micromegas-notebooks push --all
```

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `MICROMEGAS_WEB_URL` | analytics-web-srv base URL (e.g., `https://micromegas.example.com`) |

Reuses existing auth env vars: `MICROMEGAS_OIDC_ISSUER`, `MICROMEGAS_OIDC_CLIENT_ID`, etc.

## Implementation Steps

### Phase 1: Bearer Token Auth (Rust)
1. Modify `cookie_auth_middleware` in `rust/analytics-web-srv/src/auth.rs` to check `Authorization: Bearer` header before falling back to cookies
2. Test with `curl -H "Authorization: Bearer <token>"` against a local server

### Phase 2: `source_url` Column (Rust + DB + Frontend)
3. Add migration v3: `ALTER TABLE screens ADD COLUMN source_url VARCHAR(1024) DEFAULT NULL`; bump `LATEST_APP_SCHEMA_VERSION` to 3 and add the `current_version == 2` migration block in `rust/analytics-web-srv/src/app_db/migration.rs`
4. Add `source_url` field to the `Screen` struct in `rust/analytics-web-srv/src/app_db/models.rs`
5. Update `UpdateScreenRequest` and `CreateScreenRequest` to accept optional `source_url` field
6. Update screen handlers to read/write `source_url`
7. Add source-control banner with link to `NotebookRenderer.tsx` — shown when `source_url` is set
8. Update `Screen` type in `analytics-web-app/src/lib/screens-api.ts`

### Phase 3: Python HTTP Client
9. Create `python/micromegas/micromegas/web_client.py` with `WebClient` class
   - Uses `requests` library (add to pyproject.toml dependencies)
   - Auth via bearer token from existing OIDC provider
   - Methods: `list_screens`, `get_screen`, `create_screen`, `update_screen`, `delete_screen`

### Phase 4: File I/O Helpers
10. Create `python/micromegas/micromegas/cli/notebooks.py` with:
    - `read_notebook_file(path) -> dict` — read and validate a notebook JSON file
    - `write_notebook_file(path, screen_dict)` — write pretty-printed JSON with stable key order
    - `notebook_name_from_path(path) -> str` — extract name from filename
    - `list_local_notebooks(dir) -> dict[str, dict]` — scan directory

### Phase 5: CLI Commands
11. Implement subcommands in `python/micromegas/micromegas/cli/notebooks.py`:
    - `pull` subcommand
    - `push` subcommand (sets `source_url` from template, deletes tracked notebooks missing locally)
    - `diff` subcommand
    - `list` subcommand
12. Register entry point in `python/micromegas/pyproject.toml`:
    ```toml
    micromegas-notebooks = "micromegas.cli.notebooks:main"
    ```

### Phase 6: Testing
13. Unit tests for file I/O (read/write round-trip, key ordering, validation)
14. Unit tests for diff logic
15. Integration test against a running analytics-web-srv (manual / CI with services)

### Phase 7: Documentation
16. Add `mkdocs/docs/web-app/notebooks/notebooks-as-code.md` documenting the workflow
17. Update `mkdocs/docs/web-app/notebooks/index.md` to link to it

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics-web-srv/src/auth.rs` | Add Bearer token fallback in `cookie_auth_middleware` |
| `rust/analytics-web-srv/src/app_db/migration.rs` | Add v3 migration for `source_url` column; bump `LATEST_APP_SCHEMA_VERSION` to 3 |
| `rust/analytics-web-srv/src/app_db/models.rs` | Add `source_url` to `Screen`, `CreateScreenRequest`, and `UpdateScreenRequest` |
| `rust/analytics-web-srv/src/screens.rs` | Include `source_url` in queries and update handler |
| `analytics-web-app/src/lib/screens-api.ts` | Add `source_url` to `Screen` type |
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | Add source-control warning banner |
| `python/micromegas/micromegas/web_client.py` | **New** — HTTP client for REST API |
| `python/micromegas/micromegas/cli/notebooks.py` | **New** — CLI subcommands |
| `python/micromegas/pyproject.toml` | Add `requests` dependency, add `micromegas-notebooks` entry point |
| `python/micromegas/tests/test_notebook_files.py` | **New** — unit tests |
| `mkdocs/docs/web-app/notebooks/notebooks-as-code.md` | **New** — documentation |
| `mkdocs/docs/web-app/notebooks/index.md` | Add link to new doc page |

## Trade-offs

### One file per notebook vs. single export file
**Chosen: one file per notebook.** A single export file (like the web UI produces) doesn't diff well — changing one notebook changes the whole file. One file per notebook gives clean git history per notebook.

### JSON vs. YAML
**Chosen: JSON.** Zero round-trip risk, matches existing format, no new dependency. YAML would be slightly more readable for hand-editing but introduces parsing edge cases (anchors, multiline strings, type coercion).

### CLI tool vs. server-side git integration
**Chosen: CLI tool.** The server doesn't need to know about git. This keeps the architecture simple: PG is the runtime store, git is for version control and deployment. A server-side approach (e.g., server watches a git repo) would add complexity, require git credentials on the server, and couple the server to a specific git workflow.

### Manifest file vs. directory scan
**Chosen: directory scan.** A manifest would let you track notebooks that don't exist locally yet, but it's another file to keep in sync. The directory itself is the manifest — simpler mental model.

### `requests` vs. `httpx` vs. `urllib`
**Chosen: `requests`.** Already widely used, synchronous (matches the CLI usage pattern), well-understood. `httpx` would be fine too but adds less value for a CLI tool.

## Documentation

| Page | Action |
|------|--------|
| `mkdocs/docs/web-app/notebooks/notebooks-as-code.md` | **Create** — full guide with workflow examples |
| `mkdocs/docs/web-app/notebooks/index.md` | **Update** — add link to notebooks-as-code page |

## Testing Strategy

1. **Unit tests** (`test_notebook_files.py`):
   - Round-trip: write then read produces identical dict
   - Key ordering in output JSON is stable
   - Validation rejects malformed files (missing name, wrong structure)
   - Diff logic correctly identifies added/removed/modified notebooks

2. **Manual integration test**:
   - Start local services (`python3 local_test_env/ai_scripts/start_services.py`)
   - Create a notebook in the web UI
   - `micromegas-notebooks pull --all` → verify file on disk
   - Edit the file
   - `micromegas-notebooks diff` → verify diff shown
   - `micromegas-notebooks push` → verify update in web UI

## Resolved Questions

1. **Auth for the web server REST API**: The web server does NOT currently accept Bearer tokens — only session cookies. This is a prerequisite (Phase 1). The auth infrastructure (`micromegas_auth::types::HttpRequestParts`) already supports it; we just need to add the header check to `cookie_auth_middleware`.

2. **Should `push` delete notebooks from the server?** Yes — when `push --all` runs, notebooks that have a `source_url` set on the server but no corresponding local file are deleted. This ensures that deleting a notebook file from git and pushing results in it being removed from the server. Notebooks without `source_url` are never touched.

3. **CI/CD usage**: OIDC client credentials flow (`MICROMEGAS_OIDC_CLIENT_SECRET`) already works for service accounts — document this as the recommended approach for CI pipelines.
