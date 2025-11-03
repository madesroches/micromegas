# Authentication

Micromegas supports unified authentication across all services using both API keys and OpenID Connect (OIDC).

## Overview

Both the analytics server (flight-sql-srv) and ingestion server (telemetry-ingestion-srv) support two authentication methods:

- **OIDC (OpenID Connect)** - For human users and service accounts via federated identity providers (Google, Azure AD, Okta, Auth0, etc.)
- **API Keys** - Legacy support for simple bearer token authentication

Both methods can be enabled simultaneously. When multiple providers are configured, they are tried in order until one succeeds (API key first for performance, then OIDC).

## Authentication Methods

### OIDC Authentication (Recommended)

OIDC provides secure federated authentication with automatic token refresh and support for multiple identity providers.

**Benefits:**

- Standards-based authentication (OAuth 2.0 / OpenID Connect)
- Centralized user management via identity provider
- Automatic token refresh with no manual intervention
- Support for multiple identity providers simultaneously
- Full audit trail with user identity (email, subject)
- Token revocation support via identity provider

**Supported Identity Providers:**

- Google OAuth
- Microsoft Azure AD
- Okta
- Auth0
- Any standards-compliant OIDC provider

### API Keys (Legacy)

Simple bearer token authentication using pre-shared keys.

**Benefits:**

- Simple to configure
- Fast validation (HashMap lookup)
- No external dependencies

**Limitations:**

- Manual key distribution and rotation
- No automatic expiration
- No user identity for audit logging

## Server Configuration

### OIDC Configuration

Configure OIDC authentication using environment variables:

```bash
# OIDC Configuration
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "your-app-id.apps.googleusercontent.com"
    },
    {
      "issuer": "https://login.microsoftonline.com/{tenant}/v2.0",
      "audience": "api://your-api-id"
    }
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'

# Optional: Configure admin users
export MICROMEGAS_ADMINS='["alice@example.com", "bob@example.com"]'
```

**Configuration Fields:**

| Field | Description | Default |
|-------|-------------|---------|
| `issuers` | Array of OIDC issuer configurations | Required |
| `issuers[].issuer` | OIDC issuer URL | Required |
| `issuers[].audience` | Expected audience claim (client_id) | Required |
| `jwks_refresh_interval_secs` | JWKS cache TTL in seconds | 3600 |
| `token_cache_size` | Maximum validated tokens to cache | 1000 |
| `token_cache_ttl_secs` | Token cache TTL in seconds | 300 |

**Admin Configuration:**

The `MICROMEGAS_ADMINS` environment variable is a JSON array of user identifiers (email or subject) that have administrative privileges. Admin users can perform operations like partition management.

### API Key Configuration

Configure API keys using the `MICROMEGAS_API_KEYS` environment variable with a JSON array:

```bash
export MICROMEGAS_API_KEYS='[
  {"name": "service1", "key": "secret-key-123"},
  {"name": "service2", "key": "secret-key-456"}
]'
```

**Format:**
- JSON array of objects
- Each object has `name` (identifier for logging) and `key` (the actual API key)
- The `key` value is sent as the Bearer token by clients
- Generate keys with: `openssl rand -base64 512`

### Disable Authentication (Development Only)

For local development and testing, authentication can be disabled:

```bash
# Analytics server
flight-sql-srv --disable-auth

# Ingestion server
telemetry-ingestion-srv --disable-auth
```

!!! danger "Security Warning"
    Never disable authentication in production environments. This flag is intended only for local development and testing.

## Client Configuration

### Python Client with OIDC

The Python client supports automatic browser-based login with token persistence and refresh.

#### Interactive Use (Jupyter, Scripts)

```python
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient

# First use: Opens browser for authentication
auth = OidcAuthProvider.login(
    issuer="https://accounts.google.com",
    client_id="your-app-id.apps.googleusercontent.com",
    client_secret="your-client-secret",  # Optional for some providers
    token_file="~/.micromegas/tokens.json"  # Persists tokens
)

# Create authenticated client
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Run queries - tokens auto-refresh before expiration
df = client.query("SELECT * FROM processes LIMIT 10")
```

#### Subsequent Use (Token Reuse)

```python
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient

# Load existing tokens - no browser interaction needed
auth = OidcAuthProvider.from_file(
    "~/.micromegas/tokens.json",
    client_secret="your-client-secret"  # Optional
)

client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Tokens automatically refresh when needed
import datetime
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
df = client.query("SELECT * FROM log_entries LIMIT 1000", begin, now)
```

#### Token Management

```python
# Clear saved tokens (logout)
import os
from pathlib import Path

token_file = Path.home() / ".micromegas" / "tokens.json"
if token_file.exists():
    token_file.unlink()
    print("Logged out - tokens cleared")
```

### CLI Tools with OIDC

CLI tools automatically support OIDC when environment variables are set:

```bash
# Configure OIDC
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="your-app-id.apps.googleusercontent.com"
export MICROMEGAS_OIDC_CLIENT_SECRET="your-client-secret"  # Optional
export MICROMEGAS_ANALYTICS_URI="grpc+tls://analytics.example.com:50051"

# First use: Opens browser for authentication
python3 -m micromegas.cli.query_processes --since 1h

# Subsequent uses: No browser interaction, uses cached tokens
python3 -m micromegas.cli.query_process_log <process_id>

# Logout (clear saved tokens)
micromegas_logout
```

**Environment Variables:**

| Variable | Description | Required |
|----------|-------------|----------|
| `MICROMEGAS_OIDC_ISSUER` | OIDC issuer URL | Yes |
| `MICROMEGAS_OIDC_CLIENT_ID` | OAuth client ID | Yes |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | OAuth client secret | No* |
| `MICROMEGAS_TOKEN_FILE` | Token storage path | No (default: ~/.micromegas/tokens.json) |
| `MICROMEGAS_ANALYTICS_URI` | Analytics server URI | No (default: grpc://localhost:50051) |

*Required for some providers (e.g., Google) even with PKCE

### Python Client with API Keys (Legacy)

```python
from micromegas.flightsql.client import FlightSQLClient

client = FlightSQLClient(
    "grpc://localhost:50051",
    headers={"authorization": "Bearer your-api-key"}
)

df = client.query("SELECT * FROM processes LIMIT 10")
```

!!! warning "Deprecated API"
    The `headers` parameter is deprecated. Use `auth_provider` with `OidcAuthProvider` instead.

## Ingestion Service Authentication

The telemetry ingestion service (telemetry-ingestion-srv) uses the same authentication infrastructure as the analytics service.

### Server Configuration

The ingestion server uses the same environment variables as the analytics server:

```bash
# Start ingestion server with authentication
export MICROMEGAS_API_KEYS='[{"name": "service1", "key": "secret-key-123"}]'
export MICROMEGAS_OIDC_CONFIG='{"issuers": [...]}'
telemetry-ingestion-srv

# Or disable auth for development
telemetry-ingestion-srv --disable-auth
```

### Rust Client Authentication

Rust applications sending telemetry can use either API keys or OIDC client credentials.

#### Automatic Configuration (Recommended)

Applications using `#[micromegas_main]` automatically configure authentication from environment variables:

```rust
use micromegas::micromegas_main;
use micromegas::tracing::prelude::*;

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    info!("Application starting");
    // Telemetry automatically authenticated based on environment variables
    Ok(())
}
```

**Environment Variables:**

| Variable | Authentication Method | Required |
|----------|----------------------|----------|
| `MICROMEGAS_INGESTION_API_KEY` | API key (simple) | For API key auth |
| `MICROMEGAS_OIDC_TOKEN_ENDPOINT` | OIDC client credentials | For OIDC auth |
| `MICROMEGAS_OIDC_CLIENT_ID` | OIDC client credentials | For OIDC auth |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | OIDC client credentials | For OIDC auth |
| `MICROMEGAS_TELEMETRY_URL` | Ingestion server URL | Yes (e.g., http://localhost:9000) |

**Example (API Key):**
```bash
export MICROMEGAS_INGESTION_API_KEY=secret-key-123
export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
cargo run
```

**Example (OIDC Client Credentials):**
```bash
export MICROMEGAS_OIDC_TOKEN_ENDPOINT=https://accounts.google.com/o/oauth2/token
export MICROMEGAS_OIDC_CLIENT_ID=my-service@project.iam.gserviceaccount.com
export MICROMEGAS_OIDC_CLIENT_SECRET=secret-from-secret-manager
export MICROMEGAS_TELEMETRY_URL=http://localhost:9000
cargo run
```

#### Manual Configuration

For applications not using `#[micromegas_main]`, configure authentication manually:

##### API Key Authentication (Simple)

```rust
use micromegas_telemetry_sink::http_event_sink::HttpEventSink;
use micromegas_telemetry_sink::api_key_decorator::ApiKeyRequestDecorator;
use std::sync::Arc;

// From environment variable
std::env::set_var("MICROMEGAS_INGESTION_API_KEY", "secret-key-123");
let decorator = ApiKeyRequestDecorator::from_env().unwrap();

// Configure HttpEventSink with authentication
let sink = HttpEventSink::new(
    "http://localhost:9000",
    max_queue_size,
    metadata_retry,
    blocks_retry,
    Box::new(move || Arc::new(decorator.clone())),
);
```

##### OIDC Client Credentials (Production)

```rust
use micromegas_telemetry_sink::http_event_sink::HttpEventSink;
use micromegas_telemetry_sink::oidc_client_credentials_decorator::OidcClientCredentialsDecorator;
use std::sync::Arc;

// Configure OIDC client credentials
std::env::set_var("MICROMEGAS_OIDC_TOKEN_ENDPOINT",
    "https://accounts.google.com/o/oauth2/token");
std::env::set_var("MICROMEGAS_OIDC_CLIENT_ID",
    "my-service@project.iam.gserviceaccount.com");
std::env::set_var("MICROMEGAS_OIDC_CLIENT_SECRET",
    "secret-from-secret-manager");

let decorator = OidcClientCredentialsDecorator::from_env().unwrap();

let sink = HttpEventSink::new(
    "http://localhost:9000",
    max_queue_size,
    metadata_retry,
    blocks_retry,
    Box::new(move || Arc::new(decorator.clone())),
);
```

**Authentication Methods Comparison:**

| Method | Use Case | Token Lifetime | Complexity |
|--------|----------|----------------|------------|
| API Key | Development, testing | No expiration | Low |
| Client Credentials | Production services | ~1 hour (auto-refresh) | Medium |

### Health Endpoint

The `/health` endpoint remains public for monitoring and liveness checks, even when authentication is enabled.

```bash
# Health check always works without authentication
curl http://localhost:9000/health
```

## Setting Up OIDC Providers

### Google OAuth Setup

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project or select existing
3. Navigate to **APIs & Services → OAuth consent screen**
   - Select "External" user type
   - Fill in app name and contact emails
   - Add test users (yourself and team members)
4. Navigate to **APIs & Services → Credentials**
   - Click "+ CREATE CREDENTIALS" → "OAuth client ID"
   - Application type: **"Desktop app"** (for CLI/local use)
   - Click "Create"
5. Copy both credentials:
   - **Client ID** (ends with `.apps.googleusercontent.com`)
   - **Client Secret**
6. Add authorized redirect URIs:
   - `http://localhost:48080/callback`

**Server Configuration:**

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "123-abc.apps.googleusercontent.com"
    }
  ]
}'
```

**Client Configuration:**

```bash
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="123-abc.apps.googleusercontent.com"
export MICROMEGAS_OIDC_CLIENT_SECRET="GOCSPX-..."
```

### Azure AD Setup

1. Go to [Azure Portal](https://portal.azure.com/)
2. Navigate to **Azure Active Directory → App registrations**
3. Click **"New registration"**
   - Name: "Micromegas Analytics"
   - Supported account types: Choose based on your needs
   - Redirect URI: "Public client/native" - `http://localhost:48080/callback`
4. Note the **Application (client) ID**
5. Navigate to **Authentication**
   - Under "Advanced settings", set "Allow public client flows" to **Yes**
   - This enables PKCE without requiring a client secret
6. Navigate to **API permissions** (optional)
   - Add permissions if needed for your organization

**Server Configuration:**

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://login.microsoftonline.com/{tenant-id}/v2.0",
      "audience": "{application-id}"
    }
  ]
}'
```

**Client Configuration:**

```bash
export MICROMEGAS_OIDC_ISSUER="https://login.microsoftonline.com/{tenant-id}/v2.0"
export MICROMEGAS_OIDC_CLIENT_ID="{application-id}"
# No MICROMEGAS_OIDC_CLIENT_SECRET needed - Azure AD supports public clients with PKCE
```

### Auth0 Setup

1. Go to [Auth0 Dashboard](https://manage.auth0.com/)
2. Create application:
   - Applications → Create Application
   - Name: "Micromegas Analytics"
   - Application type: **"Native"** (for CLI/desktop)
3. Configure application:
   - Allowed Callback URLs: `http://localhost:48080/callback`
   - Allowed Web Origins: `http://localhost:48080`
4. Note the **Domain** and **Client ID**
5. For Native apps, client secret is optional (true public client)

**Server Configuration:**

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://your-tenant.auth0.com/",
      "audience": "your-client-id"
    }
  ]
}'
```

**Client Configuration:**

```bash
export MICROMEGAS_OIDC_ISSUER="https://your-tenant.auth0.com/"
export MICROMEGAS_OIDC_CLIENT_ID="your-client-id"
# No client_secret needed for Native apps
```

## Security Considerations

### Token Storage

Tokens are stored at `~/.micromegas/tokens.json` with secure file permissions (0600 - owner read/write only).

**Token File Contents:**

- Access token (JWT)
- Refresh token
- ID token
- Expiration time
- Issuer and client ID

!!! warning "Token File Security"
    Never commit token files to version control or share them. Tokens provide full access to your analytics data.

### Token Refresh

The Python client automatically refreshes tokens when they approach expiration (5-minute buffer). This ensures:
- No mid-query authentication failures
- Transparent token management
- Thread-safe concurrent query support

### Token Revocation

To revoke access:

1. **User accounts:** Disable the user in your identity provider (Google, Azure AD, etc.)
2. **Service accounts:** Disable or delete the service account in your identity provider
3. **Immediate revocation:** Restart the analytics server to clear the token validation cache

**Revocation Timing:**

- New tokens will be rejected immediately after disabling the account
- Existing cached tokens remain valid for up to 5 minutes (configurable via `token_cache_ttl_secs`)
- Total revocation time: Cache TTL (5 min) + Token lifetime (typically 60 min) = ~65 minutes worst case

For faster revocation, use shorter token cache TTL or restart the analytics server.

### Admin Privileges

Admin users (configured via `MICROMEGAS_ADMINS`) have elevated privileges for administrative operations. Only grant admin access to trusted users.

**Admin Capabilities:**

- Partition management functions
- Schema migration operations
- Administrative SQL functions

### HTTPS/TLS

Always use TLS for production deployments:

```python
# Production: Use grpc+tls
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Development only: Plain grpc
client = FlightSQLClient(
    "grpc://localhost:50051",
    auth_provider=auth
)
```

Configure your load balancer or reverse proxy to handle TLS termination.

### PKCE (Proof Key for Code Exchange)

The Python client uses PKCE for all OIDC flows, providing security for public clients (desktop apps, CLIs) that cannot securely store client secrets.

**How PKCE Works:**

1. Client generates random `code_verifier`
2. Client creates `code_challenge` (SHA256 hash of verifier)
3. Authorization request includes `code_challenge`
4. Token exchange includes original `code_verifier`
5. Identity provider validates the verifier matches the challenge

This prevents authorization code interception attacks even if the client secret is compromised or unavailable.

## Troubleshooting

### Authentication Failures

**Symptom:** "Invalid token" or "Authentication failed" errors

**Solutions:**

1. Check server logs: `tail -f /tmp/analytics.log | grep -i auth`
2. Verify OIDC configuration matches between server and client
3. Ensure Client ID and Issuer URL are correct
4. Check token expiration: `cat ~/.micromegas/tokens.json | jq .expires_at`
5. Clear tokens and re-authenticate: `micromegas_logout`

### Token Refresh Failures

**Symptom:** Browser opens on every CLI invocation

**Solutions:**

1. Check if refresh token is present: `cat ~/.micromegas/tokens.json | jq .refresh_token`
2. Verify client secret matches (if required by provider)
3. Check token file permissions: `ls -la ~/.micromegas/tokens.json` (should be 600)
4. Re-authenticate: `micromegas_logout` then retry

### Server Configuration Issues

**Symptom:** Server fails to start or rejects all authentication

**Solutions:**

1. Validate OIDC config JSON syntax: `echo $MICROMEGAS_OIDC_CONFIG | jq .`
2. Check server can reach identity provider: `curl https://accounts.google.com/.well-known/openid-configuration`
3. Verify audience matches client ID exactly
4. Check server logs for OIDC discovery errors

### Multi-Provider Issues

**Symptom:** Only one identity provider works

**Solutions:**

1. Verify all issuers are in the configuration array
2. Check each issuer URL is correct and accessible
3. Ensure audience (client_id) matches for each provider
4. Review server logs for OIDC discovery failures per issuer

## Migration from API Keys to OIDC

To migrate from API keys to OIDC authentication:

1. **Set up OIDC provider** (Google, Azure AD, etc.)
2. **Configure server** with both API keys and OIDC
3. **Update clients** to use OIDC authentication
4. **Test** with OIDC while API keys still work (parallel operation)
5. **Remove API keys** from configuration when migration complete

**Example Migration:**

```bash
# Step 1: Add OIDC configuration (API keys still work)
export MICROMEGAS_API_KEYS='[{"name": "service1", "key": "old-key"}]'
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://accounts.google.com",
    "audience": "new-client-id.apps.googleusercontent.com"
  }]
}'

# Step 2: Update clients to use OIDC
# Test both authentication methods work

# Step 3: Remove API keys when all clients migrated
unset MICROMEGAS_API_KEYS
# Only OIDC remains
```

## Best Practices

1. **Use OIDC for all new deployments** - Better security and user management
2. **Enable admin privileges sparingly** - Only for users who need administrative access
3. **Use short token cache TTL** in high-security environments (60-300 seconds)
4. **Monitor authentication logs** - Track failed auth attempts and unusual patterns
5. **Rotate client secrets regularly** - Update in identity provider and redistribute
6. **Use separate OAuth clients** for different environments (dev, staging, prod)
7. **Document your identity provider setup** - Makes onboarding new team members easier

## Reference

- [OpenID Connect Core Specification](https://openid.net/specs/openid-connect-core-1_0.html)
- [OAuth 2.0 RFC 6749](https://datatracker.ietf.org/doc/html/rfc6749)
- [PKCE RFC 7636](https://datatracker.ietf.org/doc/html/rfc7636)
- [OAuth 2.0 Security Best Practices](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-security-topics)
