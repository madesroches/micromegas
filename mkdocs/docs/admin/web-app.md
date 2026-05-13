# Web App Deployment

> **TLDR:** The web app is a dev/demo tool. For production use, query via FlightSQL directly.

## Quick Start (Dev)

```bash
cd analytics-web-app
python start_analytics_web.py
```

Opens on `http://localhost:3000` with backend on `:8000`. Automatically sets localhost defaults.

## Environment Variables

### Required

```bash
# OIDC provider configuration (same format as FlightSQL server)
# Note: Web app only supports a single issuer
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "your-client-id.apps.googleusercontent.com"
    }
  ]
}'

# CORS and OAuth callback
export MICROMEGAS_WEB_CORS_ORIGIN="http://localhost:3000"
export MICROMEGAS_AUTH_REDIRECT_URI="http://localhost:3000/auth/callback"

# OAuth state signing secret (IMPORTANT: must be same across all instances)
# Generate with: openssl rand -base64 32
export MICROMEGAS_STATE_SECRET="your-random-secret-here"

# FlightSQL connection
export MICROMEGAS_FLIGHTSQL_URL="grpc://127.0.0.1:50051"
```

### Optional

```bash
# Base path for reverse proxy deployments (e.g., behind ALB)
# All routes will be prefixed with this path
export MICROMEGAS_BASE_PATH="/analytics"

# Cookie settings (production)
export MICROMEGAS_COOKIE_DOMAIN=".example.com"
export MICROMEGAS_SECURE_COOKIES="true"  # HTTPS only

# Map assets (object store URI; see "Maps" below)
export MICROMEGAS_MAPS_OBJECT_STORE_URI="s3://my-bucket/maps/"
export MICROMEGAS_MAPS_MAX_UPLOAD_BYTES="268435456"  # 256 MiB default

# Disable auth (dev only)
analytics-web-srv --disable-auth
```

## Maps

Map cells render GLB assets fetched from a server-side object store. Set `MICROMEGAS_MAPS_OBJECT_STORE_URI` to a prefix the web-app process can read **and write** — admins upload and delete GLBs through **Admin → Maps**, which calls `PUT`/`DELETE` on `/api/maps/blob/{filename}`. If the variable is unset, the maps endpoints return 503 and the dropdown is empty.

**IAM / credentials.** The process credentials need the equivalent of `s3:GetObject`, `s3:PutObject`, `s3:DeleteObject`, and `s3:ListBucket` (or GCS / local-fs equivalents) scoped to the configured prefix. Read-only credentials are sufficient only if you keep populating maps out-of-band (`aws s3 cp ...`) and don't expose the admin page.

**Upload cap.** `MICROMEGAS_MAPS_MAX_UPLOAD_BYTES` bounds the per-request body for uploads (default 256 MiB). The cap is enforced before the body is buffered. The handler gzips on upload and the read path serves bytes verbatim with `Content-Encoding: gzip`.

**URI grammar.** Same shape as `MICROMEGAS_OBJECT_STORE_URI` (passed through `object_store::parse_url_opts`):

| Backend | Example |
|---|---|
| Local dev | `file:///home/you/lake/maps/` |
| AWS prod | `s3://my-bucket/maps/` |
| GCS | `gs://my-bucket/maps/` |

`start_analytics_web.py` defaults this to `<MICROMEGAS_OBJECT_STORE_URI>/maps/` — i.e. a `maps/` sibling of the telemetry lake — so a single lake root supplies both telemetry blobs and map assets for local dev.

## Production Notes

**CORS Origin must match OAuth redirect URI origin:**
```bash
MICROMEGAS_WEB_CORS_ORIGIN="https://analytics.example.com"
MICROMEGAS_AUTH_REDIRECT_URI="https://analytics.example.com/auth/callback"
```

**Deploying behind a reverse proxy with path prefix:**
```bash
# Example: ALB routes /analytics/* to the web app
MICROMEGAS_BASE_PATH="/analytics"
MICROMEGAS_WEB_CORS_ORIGIN="https://example.com"
MICROMEGAS_AUTH_REDIRECT_URI="https://example.com/analytics/auth/callback"
```

Routes become: `/analytics/health`, `/analytics/query`, `/analytics/auth/*`, etc.
The same container image works for any base path - no rebuild needed.

## API Routes

Without `MICROMEGAS_BASE_PATH`:
- `GET /health` - Health check
- `POST /query` - Execute SQL query
- `GET /perfetto/{process_id}/info` - Trace metadata
- `POST /perfetto/{process_id}/generate` - Generate Perfetto trace
- `GET /auth/login` - Initiate OAuth login
- `GET /auth/callback` - OAuth callback
- `POST /auth/refresh` - Refresh tokens
- `POST /auth/logout` - Logout
- `GET /auth/me` - Current user info

With `MICROMEGAS_BASE_PATH="/analytics"`, all routes are prefixed (e.g., `/analytics/health`).

**Configure OAuth redirect in your identity provider:**
- Add the redirect URI to allowed callbacks
- For Google: Cloud Console → APIs & Services → Credentials

## Architecture

- **Frontend**: Vite/React SPA on port 3000 (dev) or served by backend (prod)
- **Backend**: Rust (`analytics-web-srv`) on port 8000
- **Auth**: OIDC (ID tokens via httpOnly cookies)
- **Data**: FlightSQL queries to analytics service

Backend proxies FlightSQL with user's ID token. No direct data access.

## Command Line Options

```bash
analytics-web-srv [OPTIONS]

Options:
  -p, --port <PORT>              Server port [default: 3000]
      --frontend-dir <DIR>       Frontend build directory [default: ../analytics-web-app/dist]
      --disable-auth             Disable authentication (dev only)
  -h, --help                     Print help
```

Example:
```bash
analytics-web-srv --port 8000 --frontend-dir ./dist --disable-auth
```

**Warning:** `--disable-auth` removes authentication middleware. Do not use in production.
