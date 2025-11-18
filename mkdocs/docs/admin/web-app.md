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

# FlightSQL connection
export MICROMEGAS_FLIGHTSQL_URL="grpc://127.0.0.1:50051"
```

### Optional

```bash
# Cookie settings (production)
export MICROMEGAS_COOKIE_DOMAIN=".example.com"
export MICROMEGAS_SECURE_COOKIES="true"  # HTTPS only

# Disable auth (dev only)
analytics-web-srv --disable-auth
```

## Production Notes

**CORS Origin must match OAuth redirect URI origin:**
```bash
MICROMEGAS_WEB_CORS_ORIGIN="https://analytics.example.com"
MICROMEGAS_AUTH_REDIRECT_URI="https://analytics.example.com/auth/callback"
```

**Configure OAuth redirect in your identity provider:**
- Add the redirect URI to allowed callbacks
- For Google: Cloud Console → APIs & Services → Credentials

## Architecture

- **Frontend**: Next.js on port 3000
- **Backend**: Rust (`analytics-web-srv`) on port 8000
- **Auth**: OIDC (ID tokens via httpOnly cookies)
- **Data**: FlightSQL queries to analytics service

Backend proxies FlightSQL with user's ID token. No direct data access.

## Disable Auth (Dev Only)

```bash
analytics-web-srv --disable-auth --port 8000 --frontend-dir ./out
```

Removes authentication middleware. **Do not use in production.**
