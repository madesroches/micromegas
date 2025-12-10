# Plan: Add BASE_PATH Support (GitHub Issue #650)

## Status: IMPLEMENTED

## Goal
Enable deploying analytics-web-srv behind a reverse proxy with custom URL prefix (e.g., `/analytics/*`) using a single container image that works for any base path.

## Configuration
- **Environment variable only**: `MICROMEGAS_BASE_PATH` (optional, defaults to empty)
- **Runtime injection**: Backend injects config into `index.html` at serve time
- **No rebuild needed**: Same container works for any base path

## Route Structure

Routes are simplified - the base path IS the prefix:
- `{base_path}/health`
- `{base_path}/query`
- `{base_path}/perfetto/{process_id}/info`
- `{base_path}/perfetto/{process_id}/generate`
- `{base_path}/auth/login`
- `{base_path}/auth/callback`
- `{base_path}/auth/refresh`
- `{base_path}/auth/logout`
- `{base_path}/auth/me`

## Implementation Summary

### Backend (`rust/analytics-web-srv/`)

1. **main.rs**:
   - Reads `MICROMEGAS_BASE_PATH` env var
   - Prefixes all routes with base_path
   - Injects `window.__MICROMEGAS_CONFIG__` into index.html at serve time
   - Nests static file serving under base_path

2. **auth.rs**:
   - Added `base_path` field to `AuthState`
   - Cookie path set to base_path (or "/" if empty)

### Frontend (`analytics-web-app/`)

1. **src/lib/config.ts** (new):
   - Reads `window.__MICROMEGAS_CONFIG__` injected by backend
   - Falls back to dev defaults if not present

2. **src/lib/api.ts** & **src/lib/auth.tsx**:
   - Use `getConfig().basePath` instead of hardcoded paths

## Deployment Example

```bash
# Same container, different deployments:
MICROMEGAS_BASE_PATH="/analytics" docker run ...
MICROMEGAS_BASE_PATH="/micromegas" docker run ...
MICROMEGAS_BASE_PATH="" docker run ...  # default, routes at root
```

## Backwards Compatibility

- Empty `MICROMEGAS_BASE_PATH` (default) = routes at root (`/health`, `/query`, etc.)
- No changes needed to existing deployments that don't use a base path
