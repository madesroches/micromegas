# Analytics Server Authentication Plan

## Overview

Enhance the flight-sql-srv authentication to support both human users (via OIDC) and long-running services (via OAuth 2.0 client credentials), using the `openidconnect` crate.

## Current State (Updated 2025-01-24)

### Implementation Status

**✅ Phase 1 (Auth Crate): Complete**
- Separate `micromegas-auth` crate at `rust/auth/`
- AuthProvider trait, AuthContext struct, AuthType enum
- ApiKeyAuthProvider (current API key system)
- OidcAuthProvider (OIDC validation with JWKS caching)
- Unit tests in `tests/` directory
- All 10 tests + 2 doc tests passing

**⏳ Phase 1 (Integration): In Progress**
- Need to wire up AuthProvider in tonic_auth_interceptor.rs
- Need to add flight-sql-srv configuration and initialization

**🔜 Next:** Phase 2 (Service Accounts) and Phase 3 (Python client/CLI support)

### Existing Implementation (Legacy)
- Simple bearer token authentication via `check_auth` (tonic_auth_interceptor.rs:10)
- API keys stored in `KeyRing` HashMap (key_ring.rs:51)
- Keys loaded from `MICROMEGAS_API_KEYS` environment variable (JSON array format)
- Can be disabled with `--disable_auth` flag
- No identity information beyond key name mapping
- No token expiration or rotation

### Limitations (Being Addressed)
- ✅ No support for federated identity providers → OIDC provider implemented
- ⏳ Manual API key distribution and rotation → Service accounts planned (Phase 2)
- ✅ No fine-grained access control → Admin RBAC implemented (is_admin flag)
- ✅ No audit trail of user identity → AuthContext captures identity
- ⏳ Requires out-of-band key management → Service account SQL UDFs planned

## Requirements

### Human Users (OIDC)
1. Login via identity provider (Google, Azure AD, Okta, etc.)
2. Short-lived access tokens with automatic refresh
3. User identity propagation for audit logging
4. Support for multiple identity providers
5. Token validation including signature verification

### Long-Running Services
1. OAuth 2.0 client credentials flow for service accounts
2. Service accounts managed in OIDC provider (Google, Azure AD, Okta)
3. Standard client_id + client_secret authentication
4. Tokens fetched from OIDC provider (cached for token lifetime ~1 hour)
5. Service identity for audit logging
6. Backward compatibility with existing API key approach (migration path)

### General
1. Minimal performance overhead (gRPC interceptor must be fast)
2. Configuration via environment variables
3. Optional auth bypass for development/testing
4. Token caching to avoid repeated validation calls
5. Clear error messages for auth failures

## Design Approach

### Authentication Modes

The system will support two authentication modes (configurable):

1. **OIDC Mode**: For both human users and service accounts
   - **Human users**: Authorization code flow with PKCE
   - **Service accounts**: Client credentials flow (client_id + client_secret)
   - JWT token validation with remote JWKS from OIDC provider
   - Identity provider discovery
   - Support for multiple identity providers
   - Single authentication path for all users

2. **API Key Mode**: Legacy support (current implementation)
   - Simple bearer token validation
   - Backward compatibility
   - Migration support

### Architecture Components

#### 1. AuthProvider Trait
Abstract authentication interface to support multiple auth strategies:
```rust
trait AuthProvider {
    async fn validate_token(&self, token: &str) -> Result<AuthContext>;
}

struct AuthContext {
    subject: String,                 // user/service ID (primary identity)
    email: Option<String>,           // for OIDC users (optional)
    issuer: String,                  // token issuer (OIDC provider)
    expires_at: DateTime<Utc>,       // token expiration (chrono for better ergonomics)
    auth_type: AuthType,             // Oidc | ApiKey
    is_admin: bool,                  // Simple RBAC - admin flag for administrative operations
}

// Simple RBAC:
// - is_admin=true allows administrative operations (if needed in future)
// - Determined from MICROMEGAS_ADMINS config (list of subjects/emails)
//
// Using DateTime<Utc> instead of i64:
// - More type-safe and self-documenting
// - Better API for comparisons and formatting
// - Convert from JWT's i64 (NumericDate) when creating AuthContext
// - Example: DateTime::from_timestamp(jwt_exp, 0)
```

Implementations:
- `OidcAuthProvider` - OIDC/JWT validation with remote JWKS (both human users and service accounts)
- `ApiKeyAuthProvider` - Current key-ring approach (legacy)

#### 2. JWT Validation
Using `openidconnect` crate:

**For all OIDC tokens (human users and service accounts):**
- Fetch JWKS from identity provider's well-known endpoint
- `IdTokenVerifier` for JWT signature validation
- Nonce validation for replay prevention (human users)
- Claims extraction (sub, email, exp, aud, iss)
- JWKS cache with TTL refresh
- Same validation path for both authorization code and client credentials flows

#### 3. Token Cache
In-memory cache for validated tokens using `moka`:
- Thread-safe concurrent cache with lock-free reads
- Built-in TTL (time-to-live) expiration support
- LRU eviction policy with configurable max capacity
- Reduces validation overhead for repeated requests
- No manual locking required (moka handles concurrency internally)

Example setup:
```rust
use moka::sync::Cache;
use std::time::Duration;

// Create cache with TTL and size limit
let cache: Cache<String, AuthContext> = Cache::builder()
    .max_capacity(1000)  // Max number of entries
    .time_to_live(Duration::from_secs(300))  // 5 min TTL
    .build();

// Usage in auth interceptor
fn validate_token(&self, token: &str) -> Result<AuthContext> {
    // Check cache first (fast, lock-free read)
    if let Some(auth_ctx) = self.cache.get(token) {
        return Ok(auth_ctx);
    }

    // Cache miss - validate token
    let auth_ctx = self.provider.validate_token(token)?;

    // Store in cache with TTL (automatic expiration)
    self.cache.insert(token.to_string(), auth_ctx.clone());

    Ok(auth_ctx)
}
```

**Benefits of moka:**
- Automatic TTL expiration (no manual DateTime checks needed)
- Thread-safe without Arc<RwLock<>> wrapper
- High performance with concurrent readers
- Production-proven (powers crates.io)

#### 4. Enhanced Auth Interceptor
Replace current `check_auth` with multi-mode interceptor:
- Extract bearer token from Authorization header
- Determine auth mode (OIDC vs API key)
- Validate token using appropriate AuthProvider
- Check if user is admin (MICROMEGAS_ADMINS list, if needed)
- Cache validation results
- Inject AuthContext into request extensions
- Emit audit logs with user/service identity

### Configuration

Environment variables:

```bash
# General
MICROMEGAS_AUTH_MODE=oidc|api_key|disabled
# oidc mode supports both human users (authorization code) and service accounts (client credentials)

# OIDC Configuration (for both human users and service accounts)
MICROMEGAS_OIDC_ISSUERS='[
  {
    "issuer": "https://accounts.google.com",
    "audience": "your-app-id.apps.googleusercontent.com"
  },
  {
    "issuer": "https://login.microsoftonline.com/{tenant}/v2.0",
    "audience": "api://your-api-id"
  }
]'
MICROMEGAS_OIDC_JWKS_REFRESH_INTERVAL=3600  # seconds

# Service Account Configuration (for services)
# Service accounts are created and managed in the OIDC provider (Google, Azure AD, Okta)
# Services use client_id + client_secret to authenticate (OAuth 2.0 client credentials flow)
# No micromegas-specific configuration needed

# Admin Configuration (Simple RBAC - optional)
MICROMEGAS_ADMINS='["alice@example.com", "bob@example.com"]'
# List of subjects/emails that have admin privileges (for future admin operations)
# Matches against AuthContext.subject or AuthContext.email

# API Key Configuration (legacy)
MICROMEGAS_API_KEYS=[{"name": "service1", "key": "..."}]

# Cache Configuration
MICROMEGAS_AUTH_CACHE_SIZE=1000
MICROMEGAS_AUTH_CACHE_TTL=300  # seconds
```

CLI flags:
```bash
--disable-auth              # Skip authentication (development)
--auth-mode <mode>          # Override MICROMEGAS_AUTH_MODE
```

### Integration with FlightSQL Server

Modifications to `flight_sql_srv.rs`:

1. Replace `KeyRing` with `AuthProvider` enum
2. Initialize appropriate provider based on `MICROMEGAS_AUTH_MODE`
3. Replace `check_auth` interceptor with new multi-mode validator
4. Store `AuthContext` in request extensions for downstream access
5. Update logging to include user/service identity

### Token Validation Flow

#### OIDC Flow (Human Users)
1. User authenticates with IdP (external to flight-sql-srv)
2. Client obtains ID token and access token
3. Client sends request with `Authorization: Bearer <id_token>`
4. flight-sql-srv validates token:
   - Decode JWT header to identify issuer
   - Check cache for previously validated token
   - If not cached, fetch JWKS from identity provider
   - Verify JWT signature using JWKS public keys
   - Validate issuer, audience, expiration
   - Extract claims (sub, email, exp, iss) into AuthContext
   - Store AuthContext in cache
5. Request proceeds with AuthContext (identity available for audit logging)

#### Service Account Flow (OAuth 2.0 Client Credentials)
1. Admin creates service account in OIDC provider (Google Cloud, Azure AD, Okta)
2. OIDC provider issues client_id + client_secret for the service account
3. Service authenticates using client credentials flow:
   - POST to token endpoint: `grant_type=client_credentials&client_id=...&client_secret=...`
   - OIDC provider returns access token (standard OAuth JWT)
   - Service caches token until expiration (~1 hour)
4. Service sends request with `Authorization: Bearer <access_token>`
5. flight-sql-srv validates token (same as human users):
   - Decode JWT header to identify issuer
   - Check cache for previously validated token
   - If not cached, fetch JWKS from identity provider
   - Verify JWT signature using JWKS public keys
   - Validate issuer, audience, expiration
   - Extract claims (sub, email, exp, iss) into AuthContext
   - Store AuthContext in cache
6. Request proceeds with AuthContext (service identity available for audit logging)

**Key advantages:**
- Standard OAuth 2.0 flow (well-understood, broadly supported)
- Leverages mature OIDC provider infrastructure (key management, rotation, revocation)
- Single authentication path for all users (simpler codebase)
- No custom key management or database schema needed
- Service accounts managed in OIDC provider (not in micromegas)

#### API Key Flow (Legacy)
1. Service sends request with `Authorization: Bearer <api_key>`
2. flight-sql-srv checks KeyRing
3. If found, create minimal AuthContext with service name
4. Request proceeds

### Dependencies

Add to `rust/Cargo.toml` workspace dependencies:
```toml
openidconnect = "4.0"  # For OIDC discovery, metadata, and JWT validation
moka = "0.12"          # For token cache with TTL and thread-safe concurrent access
chrono = "0.4"         # For DateTime types (likely already a dependency)
```

**Why moka over basic lru?**
- Built-in TTL (time-to-live) expiration support
- Thread-safe concurrent access without explicit locking
- High performance with lock-free reads
- Production-proven (powers crates.io)
- Combines LRU eviction with LFU admission policy for better hit rates

**Why openidconnect for JWT validation?**
- Standards-compliant OIDC implementation
- Built-in JWT verification with proper security checks
- Handles JWKS conversion automatically
- Well-maintained and actively developed
- Reduces custom crypto code (smaller attack surface)

Note: `chrono` is likely already in use for timestamp handling throughout the codebase.

## Implementation Phases

### Phase 1: Refactor Current Auth ✅ AUTH CRATE COMPLETE, ⏳ INTEGRATION IN PROGRESS
- ✅ Extract AuthProvider trait
- ✅ Create ApiKeyAuthProvider wrapping current KeyRing
- ✅ Add AuthContext struct
- ✅ Create separate `micromegas-auth` crate (`rust/auth/`)
- ✅ Add OIDC provider with JWT validation
- ✅ Add unit tests for API key mode and OIDC mode
- ✅ Code style improvements (module-level imports, documented structs)
- ✅ Tests moved to `tests/` directory (separate from source)
- ⏳ Wire up AuthProvider in tonic_auth_interceptor.rs
- ⏳ Add flight-sql-srv configuration and initialization
- **Status**: Auth crate complete, integration with flight-sql-srv in progress

### Phase 2: Add Service Account Support (OAuth 2.0 Client Credentials)
- Document how to create service accounts in OIDC providers:
  - Google Cloud: Service accounts with OAuth 2.0 client credentials
  - Azure AD: App registrations with client credentials
  - Okta: OAuth 2.0 service applications
- Python client: Add `OidcClientCredentialsProvider` class
  - Takes issuer, client_id, client_secret
  - Implements token fetch from OIDC provider token endpoint
  - Caches token until expiration
  - Automatic token refresh when expired
- Rust client: Add equivalent client credentials support
- Add integration tests with mock OIDC provider
- Create example service using OAuth 2.0 client credentials
- Admin RBAC (simple is_admin flag):
  - Parse MICROMEGAS_ADMINS config (list of subjects/emails)
  - Set is_admin flag in AuthContext during token validation
  - Used to gate lakehouse partition management UDFs
- **Goal**: Support service authentication via standard OAuth 2.0 flow

### Phase 3: Add OIDC Support
**Server-side:** ✅ COMPLETED
- ✅ Implement OidcAuthProvider using openidconnect crate
- ✅ Add JWKS fetching and caching from remote endpoints (with SSRF protection)
- ✅ Add multi-issuer support (Google, Azure AD, Okta)
- ✅ Add OIDC configuration parsing (from MICROMEGAS_OIDC_CONFIG)
- ✅ Add admin users support (MICROMEGAS_ADMINS)
- ⏳ Add wiremock tests with mock OIDC provider (future improvement)
- ⏳ Wire up with flight-sql-srv

**Python client:**
- Implement OidcCredentials class
- Browser-based login flow (authorization code + PKCE)
- Token storage (access + refresh tokens)
- Automatic token refresh with 5-minute buffer
- Retry logic for 401 responses
- Thread-safe token refresh for concurrent queries
- Token persistence across sessions

**CLI (simplified):**
- Full OIDC flow on each invocation (browser popup)
- No token storage/caching needed
- Recommend Python client or service accounts for frequent use

**Goal**: Support human user authentication with transparent token refresh in Python client

### Phase 4: Add Token Caching
- Implement LRU cache for validated tokens
- Add cache configuration
- Add cache metrics/monitoring
- Add cache invalidation on config changes
- **Goal**: Reduce validation overhead to <1ms

### Phase 5: Audit and Security Hardening
- Add comprehensive audit logging with user/service identity
- Add rate limiting per user/service
- Add metrics for auth failures
- Document credential management in OIDC providers
- Security review and penetration testing
- Documentation and deployment guide
- **Goal**: Production-ready authentication

## Security Considerations

1. **Token Storage**: Never log or store tokens, only validation results
2. **Client Secret Protection** (Service Accounts):
   - Store client secrets in secret managers (AWS Secrets Manager, GCP Secret Manager, Azure Key Vault)
   - Never commit client secrets to git or config files
   - Use environment variables or secret mounting for runtime access
   - Rotate client secrets periodically in OIDC provider
3. **JWKS Caching**: Refresh JWKS periodically to detect identity provider key rotation
4. **Clock Skew**: Allow configurable clock skew (default 60s) for exp/nbf validation
5. **TLS/HTTPS**: Handled by load balancer - all traffic encrypted in transit
6. **Token Revocation**:
   - Token validation cache (5 min TTL): Recently validated tokens cached in memory
   - When service account disabled in OIDC provider:
     - OIDC provider stops issuing NEW tokens immediately
     - Server continues to accept EXISTING valid tokens until cache expiry (5 min) + token expiration (~1 hour)
   - **Worst case revocation delay**: ~65 minutes
     - 5 min: validation cache expiry
     - 60 min: token expiration
   - **For immediate revocation**: Restart analytics server to clear validation cache
   - **Better approach**: Use short-lived tokens (15-30 min) to reduce revocation window
7. **Audience Validation**: Strictly validate token audience to prevent token substitution
8. **Audit Logging**: Log all authentication attempts (success and failure) with identity information
9. **OIDC Provider Security**: Rely on OIDC provider's security (Google, Azure AD, Okta have mature security practices)

## Testing Strategy

1. **Unit Tests**:
   - Each AuthProvider implementation
   - Token validation logic
   - Cache behavior
   - Claims extraction

2. **Integration Tests**:
   - Mock OIDC provider (using openidconnect test utilities)
   - Mock OAuth introspection endpoint
   - End-to-end FlightSQL requests with auth
   - Multi-mode configuration switching

3. **Performance Tests**:
   - Auth interceptor latency
   - Cache hit/miss ratios
   - Concurrent auth request handling
   - JWKS fetch impact

4. **Security Tests**:
   - Invalid token rejection
   - Expired token rejection
   - Wrong audience/issuer rejection
   - Signature verification
   - Token replay prevention (nonce validation)

## Migration Path

### For Existing API Key Users
**Backward-compatible transition:**

1. **Initial release**: Service account support
   - API keys still work (no breaking changes)
   - Early adopters can migrate immediately
   - Server supports both auth methods

2. **Migration process:**

   Admin creates service account in OIDC provider:

   **Google Cloud example:**
   ```bash
   # Create service account in Google Cloud
   gcloud iam service-accounts create my-service \
     --display-name="Data pipeline production service"

   # Create OAuth 2.0 client credentials
   gcloud iam service-accounts keys create credentials.json \
     --iam-account=my-service@project.iam.gserviceaccount.com

   # Note the client_id and download client_secret
   ```

   **Azure AD example:**
   ```bash
   # Create app registration
   az ad app create --display-name "my-service"

   # Create client secret
   az ad app credential reset --id <app-id>

   # Note the client_id (application ID) and client_secret
   ```

   Service uses OAuth 2.0 client credentials:
   ```python
   from micromegas.auth import OidcClientCredentialsProvider

   auth = OidcClientCredentialsProvider(
       issuer="https://accounts.google.com",
       client_id="my-service@project.iam.gserviceaccount.com",
       client_secret=os.environ["CLIENT_SECRET"],
   )
   client = FlightSQLClient(uri, auth_provider=auth)

   # Test in parallel - both API key and OAuth work
   # Remove API key when confident
   ```

3. **Deprecation warnings:**
   - API key usage logs deprecation warnings
   - Clear migration documentation
   - Communication to users

4. **Enforcement phase:**
   - API keys disabled by default
   - Must opt-in: `--enable-legacy-api-keys` flag
   - Strong pressure to migrate

5. **Final removal:**
   - API key code removed entirely
   - Clean, maintainable codebase
   - Major version bump

### For New Deployments
1. Choose auth mode based on use case:
   - Human users → OIDC (set up identity provider)
   - Services → OAuth 2.0 client credentials (create service accounts in OIDC provider)
   - Development → Disabled (`--disable-auth`)

2. OIDC setup:
   - Register app with identity provider (Google/Azure/Okta)
   - Configure `MICROMEGAS_OIDC_ISSUERS`
   - Users login via standard OIDC flow

3. Service account setup in OIDC provider:
   **Google Cloud:**
   ```bash
   # Create service account
   gcloud iam service-accounts create data-pipeline-prod \
     --display-name="Production data pipeline"

   # Get credentials
   gcloud iam service-accounts keys create credentials.json \
     --iam-account=data-pipeline-prod@project.iam.gserviceaccount.com
   ```

   **Azure AD:**
   ```bash
   # Create app registration
   az ad app create --display-name "data-pipeline-prod"

   # Create client secret
   az ad app credential reset --id <app-id>
   ```

   **Usage in Python:**
   ```python
   from micromegas.auth import OidcClientCredentialsProvider
   auth = OidcClientCredentialsProvider(
       issuer="https://accounts.google.com",
       client_id="data-pipeline-prod@project.iam.gserviceaccount.com",
       client_secret=os.environ["CLIENT_SECRET"],
   )
   client = FlightSQLClient(uri, auth_provider=auth)
   ```

### For Corporate Auth Wrapper Users (MICROMEGAS_PYTHON_MODULE_WRAPPER)

**Current State:**
- Python CLI uses `MICROMEGAS_PYTHON_MODULE_WRAPPER` environment variable
- Points to custom Python module that wraps authentication
- Each organization implements their own auth wrapper
- Wrapper provides a `connect()` function that returns authenticated FlightSQL client

**Migration Path:**
Once OIDC support is implemented, corporate environments can migrate to standard OIDC:

1. **Short term (backward compatible):**
   - OIDC support added to `cli/connection.py`
   - `MICROMEGAS_PYTHON_MODULE_WRAPPER` continues to work (takes precedence)
   - Organizations can choose when to migrate

2. **Migration process:**
   ```python
   # Old: Custom wrapper module
   # my_company_auth.py
   def connect():
       # Custom auth logic...
       return FlightSQLClient(uri, headers={"authorization": f"Bearer {token}"})

   # New: Use standard OIDC
   # No custom module needed - just set env vars:
   export MICROMEGAS_OIDC_ISSUER="https://login.company.com/oauth2"
   export MICROMEGAS_OIDC_CLIENT_ID="micromegas-analytics"
   # CLI automatically uses OIDC with token persistence
   ```

3. **Benefits of migrating to OIDC:**
   - No custom Python module to maintain
   - Standard OIDC flow (works with any identity provider)
   - Automatic token refresh built-in
   - Token persistence across CLI invocations
   - Same auth mechanism for CLI, Python client, and services

4. **Deprecation timeline:**
   - Phase 1: OIDC support added (wrapper still works)
   - Phase 2: Deprecation warning when wrapper is used
   - Phase 3: Documentation guides organizations to migrate
   - Phase 4 (eventual): Remove wrapper support after sufficient migration period

**Corporate Auth Wrapper → OIDC Migration:**
Organizations should plan to migrate custom auth wrappers to standard OIDC flows, reducing maintenance burden and leveraging standard OAuth2/OIDC infrastructure.

## Impacted Components

### Grafana Plugin
**Current**: Uses API keys for authentication
**Required Changes** (Single bundled update):

1. **Major version update bundling:**
   - Auth migration: API keys → OAuth 2.0 client credentials
   - Other planned improvements (TBD - what other features needed?)
   - Single release, clean break

2. **Service account authentication (OAuth 2.0 client credentials):**
   - Configure OIDC provider, client_id, and client_secret in datasource settings
   - Fetch token from OIDC provider token endpoint before first query
   - Cache token until expiration
   - Automatic token refresh when expired
   - TypeScript/JavaScript OAuth 2.0 client library (e.g., `openid-client`)

3. **Updated datasource configuration UI:**
   ```
   Authentication Method:
   ☑ OAuth 2.0 Client Credentials

   OIDC Issuer: https://accounts.google.com
   Client ID: my-service@project.iam.gserviceaccount.com
   Client Secret: [••••••••••] (secured field)

   [ Test Connection ]
   ```

4. **Migration strategy:**
   - Major version bump
   - Clear migration guide in release notes
   - Example: "Grafana plugin v2.0: Migrate from API keys to OAuth 2.0"
   - Breaking change, but one-time clean migration
   - Document how to create service accounts in different OIDC providers

### Python Client Library
**Current**:
- `FlightSQLClient(uri, headers=None)` - auth via optional headers dict
- Headers passed as `{"authorization": "Bearer token"}`
- Middleware adds headers to every gRPC request
- Simple `connect()` helper has no auth (localhost development)

**Required Changes**:

**Design: Auth Provider Pattern**
- Client takes an `auth_provider` parameter instead of static headers
- Auth provider handles token generation and automatic refresh
- Token refresh happens transparently before each query
- Client is allocated once and reused

1. **Auth Provider Interface**
   ```python
   # Simple interface - any object with get_token() method works
   # Using Protocol for type hints (available in Python 3.8+, project uses 3.10+)

   from typing import Protocol

   class AuthProvider(Protocol):
       """Protocol for authentication providers.

       Any class implementing get_token() can be used as an auth provider.
       This uses structural typing - no inheritance required.
       """
       def get_token(self) -> str:
           """Get current valid token, refreshing if necessary.

           Returns:
               str: Bearer token to use in Authorization header
           """
           ...
   ```

2. **Service Account Support (OAuth 2.0 Client Credentials)**
   ```python
   from micromegas.auth import OidcClientCredentialsProvider
   from micromegas.flightsql.client import FlightSQLClient

   # Create auth provider with service account credentials
   auth = OidcClientCredentialsProvider(
       issuer="https://accounts.google.com",
       client_id="my-service@project.iam.gserviceaccount.com",
       client_secret=os.environ["CLIENT_SECRET"],
   )

   # Create client once with auth provider
   client = FlightSQLClient(
       "grpc+tls://analytics.example.com:50051",
       auth_provider=auth
   )

   # Use client multiple times - tokens auto-refreshed before each query
   df1 = client.query("SELECT * FROM logs WHERE time > now() - interval '1 hour'")
   df2 = client.query("SELECT * FROM metrics WHERE service = 'api'")
   # Each query automatically calls auth.get_token() which fetches/refreshes token
   ```

   **Implementation details**:
   - `OidcClientCredentialsProvider` uses OAuth 2.0 client credentials flow
   - `get_token()` fetches token from OIDC provider token endpoint on first call
   - Caches token until expiration (~1 hour)
   - Automatically refreshes when expired (transparent to caller)
   - Uses standard `requests` library for token endpoint calls

3. **OIDC Support with Automatic Refresh**
   ```python
   from micromegas.auth import OidcAuthProvider
   from micromegas.flightsql.client import FlightSQLClient

   # Initial login (opens browser) and save tokens
   auth = OidcAuthProvider.login(
       issuer="https://accounts.google.com",
       client_id="...",
       token_file="~/.micromegas/tokens.json"  # Persist tokens
   )

   # Or load existing saved credentials
   auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")

   # Create client once with auth provider
   client = FlightSQLClient(
       "grpc+tls://analytics.example.com:50051",
       auth_provider=auth
   )

   # Use client multiple times - tokens auto-refreshed before each query
   df1 = client.query("SELECT * FROM logs")
   time.sleep(3600)  # 1 hour later...
   df2 = client.query("SELECT * FROM metrics")  # Token automatically refreshed!
   ```

   **Implementation details**:
   - Store access token + refresh token + expiration in JSON file
   - `get_token()` checks if token expires soon (5 min buffer)
   - If expired/expiring, use refresh token to get new access token
   - Update stored tokens on disk after refresh
   - Re-authenticate (browser flow) only if refresh token expired
   - Thread-safe token refresh using locks (for concurrent queries)
   - Called automatically by FlightSQLClient before each query

4. **FlightSQLClient Implementation**
   ```python
   # Integration with existing middleware pattern
   class FlightSQLClient:
       def __init__(
           self,
           uri: str,
           auth_provider: Optional[AuthProvider] = None,
           headers: Optional[Dict[str, str]] = None  # Legacy support
       ):
           """Initialize FlightSQL client with auth provider.

           Args:
               uri: FlightSQL server URI
               auth_provider: Auth provider that implements get_token()
               headers: Static headers for backward compatibility (legacy API keys)
           """
           self.uri = uri
           self.auth_provider = auth_provider
           self.static_headers = headers or {}

           # Create middleware with auth provider support
           if auth_provider:
               middleware_factory = DynamicAuthMiddlewareFactory(auth_provider)
           else:
               # Existing pattern for static headers
               middleware_factory = MicromegasMiddlewareFactory(headers)

           # ... rest of initialization

       # All query methods remain unchanged - middleware handles auth automatically
   ```

   **Dynamic Auth Middleware** (new class):
   ```python
   class DynamicAuthMiddleware(flight.ClientMiddleware):
       """Middleware that calls auth provider before each request."""

       def __init__(self, auth_provider):
           self.auth_provider = auth_provider

       def sending_headers(self):
           """Called before each request - gets fresh token."""
           token = self.auth_provider.get_token()  # Auto-refresh if needed
           return {"authorization": f"Bearer {token}"}

       def call_completed(self, exception):
           if exception is not None:
               print(exception, file=sys.stderr)

       def received_headers(self, headers):
           pass


   class DynamicAuthMiddlewareFactory(flight.ClientMiddlewareFactory):
       """Factory for creating auth middleware with dynamic tokens."""

       def __init__(self, auth_provider):
           self.auth_provider = auth_provider

       def start_call(self, info):
           return DynamicAuthMiddleware(self.auth_provider)
   ```

5. **Backward Compatibility**
   ```python
   from micromegas.flightsql.client import FlightSQLClient

   # API keys still work via static headers (no auth provider)
   client = FlightSQLClient(
       "grpc://localhost:50051",
       headers={"authorization": "Bearer legacy-api-key"}
   )

   # Or use simple connect() for localhost (no auth)
   from micromegas import connect
   client = connect()  # localhost:50051, no auth
   ```

**Key Benefits**:
- **Clean separation**: Auth logic separate from client logic
- **Automatic refresh**: No manual token management by users
- **Reusable client**: Create once, use many times
- **Thread-safe**: Auth providers handle concurrent refresh properly
- **Testable**: Easy to mock auth providers for testing

### CLI Tools
**Current**: Hardcoded API key or env var
**Required Changes**:

1. **Service Account Support (OAuth 2.0 Client Credentials)**
   ```bash
   $ micromegas query "SELECT ..." \
     --client-id=my-service@project.iam.gserviceaccount.com \
     --client-secret=$CLIENT_SECRET \
     --issuer=https://accounts.google.com
   ```

2. **OIDC Support (Simple - Browser on Every Call)**
   ```bash
   # Opens browser for each command (no token caching)
   $ micromegas query "SELECT * FROM logs" --oidc
   Opening browser for authentication...
   # User authenticates, command executes, done

   # For frequent CLI use, prefer service accounts instead:
   $ micromegas query "SELECT ..." --client-id=... --client-secret=...
   ```

3. **Design Rationale**:
   - CLI usage is infrequent, so browser popup is acceptable
   - No token storage = simpler code, better security (no credentials on disk)
   - No refresh logic needed in CLI
   - For automation/frequent use → service accounts with client credentials
   - For interactive/long-running → Python client (with auto-refresh)

4. **Implementation**:
   - Full OIDC flow each invocation
   - Short-lived local callback server (e.g., http://localhost:8080/callback)
   - OAuth redirect handling
   - Use token immediately, then discard

### SDKs (Rust, others)
**Required Changes**:
1. OAuth 2.0 client credentials flow implementation
2. Token caching and automatic refresh
3. Examples and documentation

### API Key Deprecation Timeline
**Deprecation Plan - "Soon"**:

- **Phase 1**: Release OAuth 2.0 client credentials support
  - Server: OIDC auth validates tokens from both human users and service accounts
  - Python client: OidcClientCredentialsProvider class
  - CLI: OAuth 2.0 client credentials support
  - **API keys still work** (backward compatibility maintained)

- **Phase 2**: Grafana plugin major update
  - Bundle auth migration with other planned improvements
  - Single release: OAuth 2.0 client credentials + new features
  - Migration guide published
  - Deprecation warning in API key flow

- **Phase 3**: Python client OIDC support (for human users)
  - OIDC authorization code flow with auto-refresh
  - Deprecation warnings for API key usage
  - Migration documentation

- **Phase 4**: Deprecation enforcement
  - API keys disabled by default
  - Opt-in flag: `--enable-legacy-api-keys` (for stragglers)
  - Strong migration communication

- **Phase 5**: API key removal
  - Remove API key code entirely
  - Major version bump (v1.0 or v2.0)
  - Clean, maintainable auth codebase

**Key principle**: Move quickly, but maintain backward compatibility during transition

## Documentation Needs

1. **Admin Guide**:
   - How to configure OIDC auth mode
   - Identity provider setup (Google, Azure AD, Okta)
   - Admin configuration (MICROMEGAS_ADMINS list)
   - Service account management in OIDC providers:
     - Creating service accounts (Google Cloud, Azure AD, Okta)
     - Generating client credentials (client_id + client_secret)
     - Rotating client secrets in OIDC provider
     - Disabling/revoking service accounts
   - Migration from API keys to OAuth 2.0 client credentials

2. **User Guide**:
   - How to obtain OIDC tokens for CLI/SDK access
   - Using OAuth 2.0 client credentials for services
   - Storing client secrets securely (secret managers)
   - Troubleshooting auth failures

3. **Developer Guide**:
   - AuthProvider trait implementation
   - OAuth 2.0 client credentials integration examples (Rust, Python, etc.)
   - OIDC auto-refresh implementation patterns
   - Testing auth changes
   - Security best practices

4. **Integration Guide**:
   - Grafana plugin v2.0: Migration guide for bundled update (auth + features)
   - Python client: Auth provider pattern implementation
   - Python client: OidcAuthProvider with automatic token refresh (human users)
   - Python client: OidcClientCredentialsProvider (service accounts)
   - CLI: Simple OIDC flow (browser on each call)
   - Other client SDK updates

5. **Python Library Implementation Guide**:
   - Auth provider interface (Protocol)
   - OidcClientCredentialsProvider: OAuth 2.0 client credentials flow
   - OidcAuthProvider: Token storage format and refresh logic (authorization code flow)
   - Thread-safe token refresh for concurrent queries
   - Error handling (network failures, expired refresh tokens)
   - Browser-based auth flow integration
   - Testing with mock auth providers

## Open Questions

1. Should we support multiple OIDC providers simultaneously? (e.g., Google + Azure AD)
   - **Answer: Yes** - Configuration already supports multiple issuers
2. Do we need role-based access control (RBAC) or is identity sufficient?
   - **Decision: Simple RBAC** - Single `is_admin` flag for lakehouse partition management UDFs
   - Admin determined by MICROMEGAS_ADMINS config (list of subjects/emails)
   - Sufficient for administrative operations
3. What's the token refresh strategy for long-running queries?
   - **Services**: OAuth 2.0 client credentials - fetch token from OIDC provider, cache until expiration (~1 hour)
   - **OIDC users**: Python client auto-refreshes tokens transparently using refresh tokens
   - **Long queries**: Token refresh happens mid-query if needed (Python client handles it)
4. Do we need emergency token revocation support (JTI blacklist)?
   - **Decision: No** - Use short token lifetime (15-60 min) + service account disable in OIDC provider
5. Timeline for API key deprecation?
   - **Decision: Soon** - Phased approach with backward compatibility
   - Grafana plugin will be updated in single release (auth + other improvements)
   - Final removal when migration complete (major version bump)
6. Should Grafana plugin support both API keys and OAuth during transition?
   - **Decision: No** - Single bundled update with breaking change
   - Clean major version bump, bundle with other improvements
   - Clear migration guide, but no dual-mode complexity
7. Python client library: automatic token generation or manual?
   - **Decision: Automatic via auth provider pattern**
   - Client takes auth_provider parameter (OidcClientCredentialsProvider or OidcAuthProvider)
   - Auth provider handles token fetch/refresh transparently before each query
   - Client allocated once and reused, no manual token management by users

## Success Metrics

1. Zero breaking changes for existing API key users
2. OIDC login flow completes in <2s (Python client initial login)
3. Token validation adds <10ms latency per request (server-side)
4. Token refresh adds <1s latency (Python client auto-refresh)
5. Cache hit rate >95% for repeated requests (server-side token validation cache)
6. Support 3+ major identity providers (Google, Azure AD, Okta)
7. Python client auto-refresh works transparently (no user intervention for weeks)
8. CLI OIDC flow completes in <5s (browser + auth + command)
9. Zero security vulnerabilities in initial review
10. Complete documentation and examples

## Design Decisions

### OAuth 2.0 Client Credentials Instead of Self-Signed JWTs

**Decision**: Use OAuth 2.0 client credentials flow for service accounts instead of self-signed JWTs with local JWKS.

**Rationale:**

1. **Simpler Architecture**
   - Single authentication path for all users (human and service)
   - No custom key management or database schema
   - Leverages existing OIDC infrastructure

2. **Industry Standard**
   - OAuth 2.0 is the de facto standard for service authentication
   - Well-understood by developers and security teams
   - Supported by all major identity providers

3. **Better Security**
   - OIDC providers have mature key management, rotation, and revocation
   - Professional security teams manage the OIDC infrastructure
   - Reduced attack surface (no custom crypto code)

4. **Less Code**
   - ~40-50% less code than dual authentication paths
   - No service account database tables
   - No admin SQL UDFs for key management
   - Single OidcAuthProvider handles everything

5. **Easier Operations**
   - Service accounts managed in OIDC provider (not in micromegas)
   - Standard tools and workflows for credential management
   - Built-in audit trails in OIDC provider

**Trade-offs Accepted:**
- Services need network access to fetch initial token (vs. offline JWT generation)
  - Mitigated by: Token caching (~1 hour), services typically have network access
- External dependency on OIDC provider
  - Mitigated by: High availability of major providers, local caching

**Handling Compromise:**
```bash
# Disable service account in OIDC provider
gcloud iam service-accounts disable compromised-service@project.iam.gserviceaccount.com

# Create new service account
gcloud iam service-accounts create my-service-v2 \
  --display-name="Replacement for compromised service"

# Generate new credentials
gcloud iam service-accounts keys create credentials-v2.json \
  --iam-account=my-service-v2@project.iam.gserviceaccount.com

# Deploy new credentials, done
```

**Alternative considered**: Self-signed JWTs with local JWKS
- More complex (two authentication paths, custom key management)
- More code to write, test, and maintain
- Custom database schema for public keys
- Admin SQL UDFs needed for key management
- Benefits (offline operation) not significant in practice

### Why OIDC Provider for Service Account Management?

**Decision**: Service accounts managed in OIDC provider (Google, Azure AD, Okta) instead of custom database.

**Rationale:**

1. **Leverage Existing Infrastructure**
   - Organizations already have OIDC providers
   - No need to build custom service account management
   - Standard IAM workflows

2. **Professional Security**
   - OIDC providers have security teams and best practices
   - Regular security audits and compliance certifications
   - Automatic key rotation and secure storage

3. **Standard Tooling**
   - Use existing IAM tools (gcloud, az, okta CLI)
   - Integrate with secret managers automatically
   - Audit logs built-in

4. **Consistency**
   - Same place for human and service identities
   - Unified access management
   - Single source of truth for authentication

**Example Admin Workflow:**
```bash
# Admin creates service account in OIDC provider (Google Cloud example)
$ gcloud iam service-accounts create data-pipeline \
    --display-name="Production data pipeline"

# Generate credentials
$ gcloud iam service-accounts keys create credentials.json \
    --iam-account=data-pipeline@project.iam.gserviceaccount.com

# Securely distribute credentials (e.g., via secret manager)
$ gcloud secrets create data-pipeline-credentials \
    --data-file=credentials.json
```

