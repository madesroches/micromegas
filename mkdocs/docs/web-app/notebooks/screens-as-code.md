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
  "managed_by": "https://github.com/org/repo/tree/main/screens",
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
  }
}
```

- Filename must match the `name` field (e.g., `my-notebook.json`)
- Pretty-printed with 2-space indent
- Server metadata (`created_by`, `updated_by`, timestamps) is excluded

## Commands

### `init`

```bash
micromegas-screens init SERVER_URL [--remote REMOTE]
```

Initialize the screens directory. Must be run inside a git repository. Reads the git remote to construct the `managed_by` URL.

### `import`

```bash
micromegas-screens import NAME [NAME...]
```

Import existing server screens. Downloads the screen and sets `managed_by` on the server. If the screen is already managed by another repo, prompts for confirmation.

### `pull`

```bash
micromegas-screens pull [NAME...]
```

Refresh local files from server. With no arguments, pulls all locally-tracked screens. Does not pull untracked screens — use `import` for that.

### `plan`

```bash
micromegas-screens plan [NAME...]
```

Preview what `apply` would change. Shows creates, updates, deletes, and untracked screens. Read-only — no server mutations.

### `apply`

```bash
micromegas-screens apply [NAME...] [--auto-approve]
```

Apply local state to server. Runs `plan` first, then prompts for confirmation. Use `--auto-approve` for CI pipelines.

Screens tracked by this repo that no longer have a local file are deleted from the server.

### `list`

```bash
micromegas-screens list [--format table|json]
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

## Authentication

The CLI reuses existing OIDC environment variables:

| Variable | Description |
|----------|-------------|
| `MICROMEGAS_OIDC_ISSUER` | OIDC provider issuer URL |
| `MICROMEGAS_OIDC_CLIENT_ID` | OAuth client ID |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | Client secret (for CI/service accounts) |

Without a client secret, the CLI uses browser-based login with cached tokens.
