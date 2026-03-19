# Screens as Code Plan

## Overview

Add CLI tooling to manage micromegas web screens as files in git repositories. PostgreSQL remains the default and only runtime storage — this is a purely client-side sync tool. Users opt in per-screen by pulling it to disk, editing it, and pushing it back.

## Current State

### Storage
- Screens live in the `screens` PG table: `name` (PK), `screen_type`, `config` (JSONB), `created_by`, `updated_by`, `created_at`, `updated_at`
- `config` is an opaque JSONB blob containing type-specific fields (e.g., cells and time range for notebooks)
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

### File Format: JSON, One File Per Screen

Each screen is a single `.json` file containing:

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
- No lossy round-trip — what you pull is what you apply
- The web app's "Source View" (`NotebookSourceView.tsx`) already shows raw JSON
- JSON diffs well enough when pretty-printed with sorted keys
- No new dependency (YAML parser) needed

**Formatting for git-friendliness:**
- Pretty-printed with 2-space indent (`json.dumps(obj, indent=2, sort_keys=False)`)
- Keys within `config` are NOT sorted (cell order matters), but top-level keys are in a fixed order: `name`, `screen_type`, `config`
- Trailing newline

**What's excluded from the file:**
- `created_by`, `updated_by`, `created_at`, `updated_at` — these are server-side metadata, not part of the screen content. They'd create noisy diffs on every pull.

### Directory Structure

```
screens/                          # any directory in the repo
  micromegas-screens.json         # config file — identifies the managing repo
  my-notebook.json
  performance-dashboard.json
  ...
```

- Filename = screen name + `.json` (the name field inside must match)
- Flat structure — no subdirectories (screen names are already unique, flat identifiers)
- The directory can live anywhere in the repo; it's just a convention
- The set of git-managed screens is simply "whatever `.json` files are in the directory" (excluding `micromegas-screens.json`)
- All commands operate on the current working directory — `cd` into the screens directory before running them

**Config file: `micromegas-screens.json`**

Created by `init`, checked into git alongside the screen files. All commands read it from the current directory.

```json
{
  "managed_by": "https://github.com/org/repo/tree/main/screens",
  "server": "https://micromegas.example.com"
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `managed_by` | Yes | Repo+folder URL identifying the managing repo. Stored on each screen on the server. |
| `server` | Yes | analytics-web-srv URL. |

### Source-Control Tracking

When a screen is managed via git, users editing it in the web UI need to know their changes may be overwritten on the next CI apply. This requires the server to know which screens are tracked.

**Approach:** Add a `managed_by` column to the `screens` table:

```sql
ALTER TABLE screens ADD COLUMN managed_by VARCHAR(1024) DEFAULT NULL;
```

- `NULL` = not tracked, normal web-editable screen (default — preserves current behavior)
- A URL = managed by source control, pointing to the repo folder (e.g., `https://github.com/org/repo/tree/main/screens`)

The same value is stored for every screen managed by a given repo+folder. The file name always matches the screen name, so a per-file link can be derived: for GitHub, replace `/tree/` with `/blob/` and append `/{name}.json`.

Both `managed_by` and `server` are read from `micromegas-screens.json` in the current directory. This is the single source of truth — no CLI flags or env vars needed for normal operation.

**When is it set?**
- `apply` sets `managed_by` on every screen it creates or updates
- `pull` does NOT change it (pull is read-only from the server's perspective)
- Manual edits via the web UI do NOT clear it (the flag reflects the intended management mode, not the last editor)

**Web UI behavior when `managed_by` is set:**
- Show a warning banner only when the user is editing the screen (not in read-only view): *"This screen is managed by source control. Edits made here may be overwritten on the next deployment."* with a link: *"View source →"* pointing to the derived file URL
- The screen remains fully editable (users may need to make quick fixes)
- The banner uses a warning style (yellow/amber) — informational, not blocking

**API changes:**
- `GET /api/screens` and `GET /api/screens/{name}` include `managed_by` in the response
- `PUT /api/screens/{name}` accepts an optional `managed_by` field in the request body
- The CLI sets `managed_by` when applying; the web UI does not send it (so it stays unchanged)

**Multi-repo support:** Multiple git repos can each manage a different subset of screens. Each repo has a different `managed_by` value in its config file — ownership is scoped by exact string match.

**Delete safety:** When `apply` runs, it only deletes server screens whose `managed_by` exactly matches the value from the config file AND that don't have a corresponding local file. Screens managed by a different repo (different `managed_by` value) or with `managed_by = NULL` are never touched.

### CLI Commands

Add a new entry point `micromegas-screens` with subcommands:

```
micromegas-screens init SERVER_URL [--remote REMOTE] [--branch BRANCH]
micromegas-screens import NAMES...
micromegas-screens pull [NAMES...]
micromegas-screens plan [NAMES...]
micromegas-screens apply [NAMES...] [--auto-approve]
micromegas-screens list
```

All commands run in the current directory, which must contain `micromegas-screens.json` (except `init`, which creates it).

With no names specified, `pull`, `plan`, and `apply` operate on all `.json` files in the current directory — the directory is the scope, like `.tf` files in a Terraform directory. Named args narrow the scope to specific screens.

#### `init` — Initialize the screens directory and config file

```
cd screens
micromegas-screens init https://micromegas.example.com
```

- Must be run inside a git repository
- Reads the git remote URL (`origin` by default, override with `--remote`)
- Reads the current branch (`main`/`HEAD` by default, override with `--branch`)
- Computes the current directory's path relative to the repo root
- Constructs `managed_by` from these: e.g., `https://github.com/org/repo/tree/main/screens`
- Uses `giturlparse` to parse and convert remote URLs (SSH, HTTPS, git protocol) to browsable HTTPS URLs — supports GitHub, GitLab, Bitbucket, and self-hosted instances
- Writes `micromegas-screens.json` in the current directory
- Refuses if `micromegas-screens.json` already exists
- Prints the generated config for review

Example output:
```
Created micromegas-screens.json:
{
  "managed_by": "https://github.com/org/repo/tree/main/screens",
  "server": "https://micromegas.example.com"
}
```

#### `import` — Start tracking an existing server screen (like `terraform import`)

```
micromegas-screens import my-notebook
micromegas-screens import my-notebook other-notebook
```

- Fetches the screen from the server, writes it to `NAME.json`, and sets `managed_by` on the server immediately — this repo takes ownership on import
- If the screen is **untracked** (`managed_by = NULL`): imports silently, no confirmation needed
- If the screen is **tracked by another repo** (`managed_by` is set to a different value): warns and prompts for confirmation before taking ownership
  ```
  Warning: "my-notebook" is currently managed by:
    https://github.com/other-org/other-repo/tree/main/screens
  Transfer ownership to this repo? [y/N]:
  ```
- Refuses if a local file already exists for that name (already imported)
- This is the only way to adopt an existing server screen into a repo

#### `pull` — Refresh tracked screens from server to disk

```
micromegas-screens pull
micromegas-screens pull my-notebook performance-dashboard
```

- Fetches from REST API, writes pretty-printed JSON to `NAME.json` in the current directory
- No args: refreshes every locally-tracked screen (every `.json` file in the directory) — does not pull untracked server screens, use `import` for that
- Named args: pull only the specified screens (must already exist locally)
- If file already exists, overwrites it (this is intentional — you can use `git diff` to see what changed)
- Prints a summary: updated/unchanged counts

#### `plan` — Preview what apply would change (like `terraform plan`)

```
micromegas-screens plan
micromegas-screens plan my-notebook
```

- Fetches current server state, compares with local files
- Local files with no server counterpart are marked `+ create` (new screen)
- Server screens tracked by this repo (matching `managed_by` value) with no local file are marked `- delete`
- Shows a Terraform-style execution plan:
  ```
  micromegas-screens will perform the following actions:

    + create: new-notebook
    ~ update: performance-dashboard (3 cells changed)
    - delete: old-notebook (tracked, removed from local)
    = unchanged: 12 screens

  Plan: 1 to create, 1 to update, 1 to delete, 12 unchanged.

  Untracked screens on server (use 'import' to start tracking):
    ? recently-created-dashboard
  ```
- For modified screens, shows a unified diff of the JSON
- Untracked server screens (no `managed_by` or `managed_by` from a different repo) are listed as informational — they require `import` to adopt
- Pure read-only operation — no server mutations
- Useful before `apply` to preview what would change, or in CI to verify expected state

#### `apply` — Apply local screen state to server (like `terraform apply`)

```
micromegas-screens apply
micromegas-screens apply my-notebook
micromegas-screens apply --auto-approve
```

- Runs `plan` first, displays the execution plan, then prompts for confirmation:
  ```
  Do you want to apply these changes? [y/N]: y
  Applying...

  Apply complete! 1 created, 1 updated, 1 deleted.
  ```
- `--auto-approve`: skip the confirmation prompt (for CI pipelines)
- No args: applies all `.json` files in the directory and **deletes** server screens that are tracked by this repo but no longer have a local file (see Tracking below)
- Reads `NAME.json` from the current directory, calls PUT (update) or POST (create) on the server
- Sets `managed_by` from the config file on every screen it creates or updates
- Validates the JSON structure before applying
- Exits with non-zero status if the user declines or if any operation fails

#### `list` — Show screen inventory

```
micromegas-screens list
```

- Lists screens from server and local directory side by side
- Shows sync status: `synced`, `local-only`, `server-only`, `modified`

#### Common Options

| Flag | Description |
|------|-------------|
| `--auto-approve` | Skip confirmation prompt (apply only, for CI) |
| `--format` | Output format: `table` (default), `json` |

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
    def create_screen(self, name: str, screen_type: str, config: dict, managed_by: str | None = None) -> dict: ...
    def update_screen(self, name: str, config: dict, managed_by: str | None = None) -> dict: ...
    def delete_screen(self, name: str) -> None: ...
```

**Auth:** Reuses the existing OIDC auth provider (`micromegas/auth/oidc.py`). `OidcAuthProvider.get_token()` returns an ID token suitable for `Authorization: Bearer <token>`. Note: `OidcClientCredentialsProvider.get_token()` returns an *access token* (not an ID token) — whether this passes the server's JWT validation depends on the OIDC provider issuing JWT access tokens. For CI, prefer `OidcAuthProvider` with client credentials configured to include `id_token` in the response, or verify your provider issues JWT access tokens. This requires the bearer token auth prerequisite (see above).

### Conflict Handling: Keep It Simple

This tool does NOT attempt automatic conflict resolution. The workflow is:

1. `pull` always overwrites local files (server wins)
2. `apply` always overwrites server state (local wins)
3. `plan` shows what would change before you apply

**Why no automatic merge:** Screen configs are structured JSON with ordered arrays (e.g., cells). Merging these automatically is error-prone. Git handles text-level conflicts well enough. The intended workflow is:

```
# Initial setup — init creates the config file from git metadata
mkdir screens && cd screens
micromegas-screens init https://micromegas.example.com
micromegas-screens import my-notebook performance-dashboard
cd ..
git add screens/
git commit -m "import screens"

# Take ownership — first apply sets managed_by on the server
micromegas-screens apply

# Adopt a screen created in the web UI
micromegas-screens import new-dashboard
git add screens/new-dashboard.json
git commit -m "track new-dashboard"

# Create a brand new screen from scratch
# Just create the .json file locally — plan/apply will create it on the server
echo '{"name": "my-new-notebook", "screen_type": "notebook", "config": {...}}' > screens/my-new-notebook.json

# Edit cycle
# 1. Edit in web UI or in text editor
# 2. Pull latest from server
micromegas-screens pull my-notebook
# 3. Review changes
git diff screens/my-notebook.json
# 4. Commit
git commit -am "update my-notebook"

# Deploy / restore cycle (e.g., in CI)
# 1. Preview changes
micromegas-screens plan
# 2. Apply — creates/updates local screens, deletes tracked screens removed from git
micromegas-screens apply --auto-approve
```

### Environment Variables

Reuses existing auth env vars: `MICROMEGAS_OIDC_ISSUER`, `MICROMEGAS_OIDC_CLIENT_ID`, etc. The `server` and `managed_by` values come from `micromegas-screens.json`.

## Implementation Steps

### Phase 1: Bearer Token Auth (Rust)
1. Modify `cookie_auth_middleware` in `rust/analytics-web-srv/src/auth.rs` to check `Authorization: Bearer` header before falling back to cookies
2. Test with `curl -H "Authorization: Bearer <token>"` against a local server

### Phase 2: `managed_by` Column (Rust + DB + Frontend)
3. Add migration v3: `ALTER TABLE screens ADD COLUMN managed_by VARCHAR(1024) DEFAULT NULL`; bump `LATEST_APP_SCHEMA_VERSION` to 3 and add the `current_version == 2` migration block in `rust/analytics-web-srv/src/app_db/migration.rs`
4. Add `managed_by` field to the `Screen` struct in `rust/analytics-web-srv/src/app_db/models.rs`
5. Update `UpdateScreenRequest` and `CreateScreenRequest` to accept optional `managed_by` field
6. Update screen handlers to read/write `managed_by`
7. Update `Screen` type in `analytics-web-app/src/lib/screens-api.ts` to include optional `managed_by` field
8. Add source-control banner to `analytics-web-app/src/routes/ScreenPage.tsx` — shown only when editing a screen with `managed_by` set; derive per-file link by appending `/{name}.json` to `managed_by` URL. This goes in `ScreenPage` (not a renderer) because the `screen` object with `managed_by` is available there, and the banner applies to all screen types.

### Phase 3: Python HTTP Client
9. Create `python/micromegas/micromegas/web_client.py` with `WebClient` class
   - Uses `requests` library (already in pyproject.toml)
   - Auth via bearer token from existing OIDC provider
   - Methods: `list_screens`, `get_screen`, `create_screen`, `update_screen`, `delete_screen`

### Phase 4: File I/O Helpers
10. Create `python/micromegas/micromegas/cli/screens.py` with:
    - `read_config() -> dict` — read and validate `micromegas-screens.json` from the current directory
    - `read_screen_file(path) -> dict` — read and validate a screen JSON file
    - `write_screen_file(path, screen_dict)` — write pretty-printed JSON with stable key order
    - `screen_name_from_path(path) -> str` — extract name from filename
    - `list_local_screens() -> dict[str, dict]` — scan current directory (excluding `micromegas-screens.json`)

### Phase 5: CLI Commands
11. Implement subcommands in `python/micromegas/micromegas/cli/screens.py`:
    - `init` subcommand (uses `giturlparse` to parse remote URL, reads branch via `git branch --show-current`, computes relative path via `git rev-parse --show-toplevel`, constructs `managed_by`, writes config file)
    - `import` subcommand (fetches from server, refuses if `managed_by` already set by another repo)
    - `pull` subcommand (refreshes locally-tracked screens from server)
    - `plan` subcommand (computes execution plan: creates/updates/deletes; shows untracked server screens as informational)
    - `apply` subcommand (runs plan, prompts for confirmation, executes; sets `managed_by`, deletes tracked screens missing locally; `--auto-approve` for CI)
    - `list` subcommand
12. Register entry point in `python/micromegas/pyproject.toml`:
    ```toml
    micromegas-screens = "micromegas.cli.screens:main"
    ```

### Phase 6: Testing
13. Unit tests for file I/O (read/write round-trip, key ordering, validation)
14. Unit tests for diff logic
15. Integration test against a running analytics-web-srv (manual / CI with services)

### Phase 7: Documentation
16. Add `mkdocs/docs/web-app/notebooks/screens-as-code.md` documenting the workflow
17. Update `mkdocs/docs/web-app/notebooks/index.md` to link to it
18. Add `- Screens as Code: web-app/notebooks/screens-as-code.md` to the Notebooks nav section in `mkdocs/mkdocs.yml`

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics-web-srv/src/auth.rs` | Add Bearer token fallback in `cookie_auth_middleware` |
| `rust/analytics-web-srv/src/app_db/migration.rs` | Add v3 migration for `managed_by` column; bump `LATEST_APP_SCHEMA_VERSION` to 3 |
| `rust/analytics-web-srv/src/app_db/models.rs` | Add `managed_by` to `Screen`, `CreateScreenRequest`, and `UpdateScreenRequest` |
| `rust/analytics-web-srv/src/screens.rs` | Include `managed_by` in queries and update handler |
| `analytics-web-app/src/lib/screens-api.ts` | Add `managed_by` to `Screen` type |
| `analytics-web-app/src/routes/ScreenPage.tsx` | Add source-control warning banner |
| `mkdocs/mkdocs.yml` | Add screens-as-code page to Notebooks nav section |
| `python/micromegas/micromegas/web_client.py` | **New** — HTTP client for REST API |
| `python/micromegas/micromegas/cli/screens.py` | **New** — CLI subcommands |
| `python/micromegas/pyproject.toml` | Add `micromegas-screens` entry point and `giturlparse` dependency (`requests` is already a dependency) |
| `python/micromegas/tests/test_screen_files.py` | **New** — unit tests |
| `mkdocs/docs/web-app/notebooks/screens-as-code.md` | **New** — documentation |
| `mkdocs/docs/web-app/notebooks/index.md` | Add link to new doc page |

## Trade-offs

### One file per screen vs. single export file
**Chosen: one file per screen.** A single export file (like the web UI produces) doesn't diff well — changing one screen changes the whole file. One file per screen gives clean git history per screen.

### JSON vs. YAML
**Chosen: JSON.** Zero round-trip risk, matches existing format, no new dependency. YAML would be slightly more readable for hand-editing but introduces parsing edge cases (anchors, multiline strings, type coercion).

### CLI tool vs. server-side git integration
**Chosen: CLI tool.** The server doesn't need to know about git. This keeps the architecture simple: PG is the runtime store, git is for version control and deployment. A server-side approach (e.g., server watches a git repo) would add complexity, require git credentials on the server, and couple the server to a specific git workflow.

### Manifest file vs. directory scan
**Chosen: directory scan.** A manifest would let you track screens that don't exist locally yet, but it's another file to keep in sync. The directory itself is the manifest — simpler mental model.

### `requests` vs. `httpx` vs. `urllib`
**Chosen: `requests`.** Already widely used, synchronous (matches the CLI usage pattern), well-understood. `httpx` would be fine too but adds less value for a CLI tool.

## Documentation

| Page | Action |
|------|--------|
| `mkdocs/docs/web-app/notebooks/screens-as-code.md` | **Create** — full guide with workflow examples |
| `mkdocs/docs/web-app/notebooks/index.md` | **Update** — add link to screens-as-code page |
| `mkdocs/mkdocs.yml` | **Update** — add nav entry under Notebooks section |

## Testing Strategy

1. **Unit tests** (`test_screen_files.py`):
   - Round-trip: write then read produces identical dict
   - Key ordering in output JSON is stable
   - Validation rejects malformed files (missing name, wrong structure)
   - Plan logic correctly identifies added/removed/modified screens

2. **Manual integration test**:
   - Start local services (`python3 local_test_env/ai_scripts/start_services.py`)
   - Create a screen in the web UI
   - `micromegas-screens import my-notebook` → verify file on disk
   - Edit the file
   - `micromegas-screens plan` → verify execution plan shown
   - `micromegas-screens apply --auto-approve` → verify update in web UI
   - `micromegas-screens import` on a screen with `managed_by` set → verify refusal

## Resolved Questions

1. **Auth for the web server REST API**: The web server does NOT currently accept Bearer tokens — only session cookies. This is a prerequisite (Phase 1). The auth infrastructure (`micromegas_auth::types::HttpRequestParts`) already supports it; we just need to add the header check to `cookie_auth_middleware`.

2. **Should `apply` delete screens from the server?** Yes — when `apply` runs, screens whose `managed_by` matches the config file value but have no corresponding local file are deleted. This ensures that deleting a screen file from git and applying results in it being removed from the server. Screens with a different `managed_by` or `managed_by = NULL` are never touched.

3. **CI/CD usage**: `OidcClientCredentialsProvider` returns an *access token*, not an ID token — the analytics-web-srv validates JWT ID tokens, so this path only works if the OIDC provider issues JWT access tokens (e.g., Auth0 with a custom API audience). For CI, the safest approach is to use `OidcAuthProvider` with client credentials and confirm `id_token` is included in the token response. Document provider requirements in the setup guide.
