# Screens as Code

Manage micromegas screens as JSON files in a git repository using the `micromegas-screens` CLI tool. This enables version-controlled screen definitions, code review for dashboard changes, and CI/CD-driven deployments.

## Overview

PostgreSQL remains the runtime storage — `micromegas-screens` is a client-side sync tool. You opt in per-screen by importing it to disk, editing it, and applying it back.

The workflow is inspired by Terraform:

- **`init`** — set up the screens directory
- **`import`** — adopt existing server screens
- **`pull`** — refresh local files from server
- **`plan`** — preview what would change
- **`apply`** — push local state to server

## Getting Started

### Installation

```bash
pip install micromegas
# or in development:
cd python/micromegas && poetry install
```

The `micromegas-screens` command is installed as an entry point.

### Initialize

Create a directory for your screens and initialize it:

```bash
mkdir screens && cd screens
micromegas-screens init https://micromegas.example.com
```

This creates `micromegas-screens.json` with the server URL and a `managed_by` link derived from your git remote:

```json
{
  "managed_by": "https://github.com/org/repo",
  "server": "https://micromegas.example.com"
}
```

### Import Screens

Adopt existing screens from the server:

```bash
micromegas-screens import my-notebook performance-dashboard
```

Each screen is saved as a JSON file (e.g., `my-notebook.json`) and ownership is set on the server.

### Edit and Deploy

```bash
# Edit a screen file in your editor or via the web UI
# Pull latest from server
micromegas-screens pull

# Review changes
git diff

# Preview what apply would do
micromegas-screens plan

# Apply changes
micromegas-screens apply
```

## File Format

Each screen is a single `.json` file:

```json
{
  "name": "my-notebook",
  "screen_type": "notebook",
  "config": {
    "timeRangeFrom": "now-5m",
    "timeRangeTo": "now",
    "cells": []
  },
  "folder_path": "dashboards/team-a"
}
```

- Filename must match the `name` field (e.g., `my-notebook.json`)
- Pretty-printed with 2-space indent
- Server metadata (`created_by`, `updated_by`, timestamps) is excluded
- `folder_path` is optional: key omitted means "no folder / don't move it"; `""` explicitly means root. To move a screen back to root, set `"folder_path": ""` rather than removing the key.
- Files are read and written as UTF-8 (a leading BOM is tolerated on read). Non-ASCII content (e.g. em dashes, accents, CJK) is written as literal UTF-8 characters rather than `\uXXXX`-escaped.

## Commands

Pass `--version` to print the installed micromegas package and interpreter version and exit.

### `init`

```bash
micromegas-screens init SERVER_URL [--remote REMOTE]
```

Initialize the screens directory. Must be run inside a git repository. Reads the git remote to construct the `managed_by` URL.

### `import`

```bash
micromegas-screens import NAME [NAME...] [--profile NAME] [--no-auth]
```

Import existing server screens. Downloads the screen and sets `managed_by` on the server. If the screen is already managed by another repo, prompts for confirmation.

### `pull`

```bash
micromegas-screens pull [NAME...] [--profile NAME] [--no-auth]
```

Refresh local files from server. With no arguments, pulls all locally-tracked screens. Does not pull untracked screens — use `import` for that.

### `plan`

```bash
micromegas-screens plan [NAME...] [--profile NAME] [--no-auth]
```

Preview what `apply` would change. Shows creates, updates, deletes, and untracked screens, and prints a **unified diff** for each modified screen. Colored diff output is on by default in a terminal; pass `--no-color` to disable it. Read-only — no server mutations.

### `apply`

```bash
micromegas-screens apply [NAME...] [--auto-approve] [--profile NAME] [--no-auth]
```

Apply local state to server. Runs `plan` first, then prompts for confirmation. Use `--auto-approve` for CI pipelines.

Screens tracked by this repo that no longer have a local file are deleted from the server.

If a local `.json` file can't even be decoded/parsed, its identity is unknown, so deletes are skipped entirely for that run (a warning is printed, but the command still exits 0) — worth knowing when running `apply --auto-approve` in CI. A file that parses as JSON but fails schema validation only protects its own `name` (if present) from deletion; it doesn't affect deletes for other screens.

### `list`

```bash
micromegas-screens list [--format table|json] [--profile NAME] [--no-auth]
```

Show screen inventory with sync status: `synced`, `local-only`, `server-only`, `modified`.

## Source Control Tracking

When a screen is managed via git, the web UI shows a warning banner:

> This screen is managed by source control. Edits made here may be overwritten on the next deployment.

The banner includes a "View source" link to the screen's JSON file in the repository.

Screens remain fully editable in the web UI — the banner is informational only.

## CI/CD Example

```yaml
# GitHub Actions example
deploy-screens:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - run: pip install micromegas
    - run: |
        cd screens
        micromegas-screens apply --auto-approve
      env:
        MICROMEGAS_OIDC_ISSUER: ${{ secrets.OIDC_ISSUER }}
        MICROMEGAS_OIDC_CLIENT_ID: ${{ secrets.OIDC_CLIENT_ID }}
        MICROMEGAS_OIDC_CLIENT_SECRET: ${{ secrets.OIDC_CLIENT_SECRET }}
```

This example needs no `--profile`: the full `MICROMEGAS_OIDC_ISSUER`/`_CLIENT_ID`/`_CLIENT_SECRET`
triple is checked first, ahead of any profile resolution, so a non-interactive service-account
login works the same way it always has.

## Authentication

`micromegas-screens` resolves auth the same way as every other `WebClient`-based CLI
(`micromegas-grants`, `micromegas-groups`, `micromegas-setup-telemetry`), in this order:

1. All three of `MICROMEGAS_OIDC_ISSUER`/`MICROMEGAS_OIDC_CLIENT_ID`/`MICROMEGAS_OIDC_CLIENT_SECRET`
   set in the environment — non-interactive client-credentials login, for CI/service accounts (see
   the CI/CD example above).
2. Otherwise, a named connection profile from `~/.micromegas/config.json`, selected by `--profile`
   (typed after the subcommand, e.g. `micromegas-screens apply --profile prod`), `MICROMEGAS_PROFILE`,
   or `default_profile`. If the profile resolves an OIDC issuer and client ID, the CLI opens a
   browser for login on first use and caches the result in a per-profile
   `~/.micromegas/tokens-<profile>.json`, so switching `--profile` never reuses another profile's
   cached token.
3. If neither resolves, the command fails with an error naming what's missing, rather than
   silently connecting unauthenticated. Pass `--no-auth` (typed after the subcommand, e.g.
   `micromegas-screens list --no-auth`) to explicitly target a server started with
   `--disable-auth`.

`api_key_file` is not an option here: the analytics web API validates OIDC tokens only, so a
static analytics API key isn't a credential this tool can present. A profile whose only auth is
`api_key_file` fails with a diagnostic saying so — see the
[Python API guide](../../query-guide/python-api.md) for `micromegas-query`'s static-key workflow,
which that server does support.
