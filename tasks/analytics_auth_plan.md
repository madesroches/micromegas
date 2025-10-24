# Analytics Server Authentication Plan

## Overview

Enhance the flight-sql-srv authentication to support both human users (via OIDC) and long-running services (via self-signed JWTs), using the `openidconnect` and `jsonwebtoken` crates.

## Current State (Updated 2025-01-24)

### Implementation Status

**✅ Phase 1 Complete:** Auth crate created with clean architecture
- Separate `micromegas-auth` crate at `rust/auth/`
- AuthProvider trait, AuthContext struct, AuthType enum
- ApiKeyAuthProvider (current API key system)
- OidcAuthProvider (OIDC validation with JWKS caching)
- Unit tests in `tests/` directory
- All 10 tests + 2 doc tests passing

**⏳ In Progress:** Integration with flight-sql-srv
- Need to wire up AuthProvider in tonic_auth_interceptor.rs
- Need to add flight-sql-srv configuration and initialization

**🔜 Next:** Phase 2 (Service Accounts) and Python client/CLI support

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
1. Self-signed JWT generation from service account credentials
2. Short-lived tokens generated locally (no external calls)
3. Service identity for audit logging
4. Simple key rotation via credential updates
5. Backward compatibility with existing API key approach (migration path)

### General
1. Minimal performance overhead (gRPC interceptor must be fast)
2. Configuration via environment variables
3. Optional auth bypass for development/testing
4. Token caching to avoid repeated validation calls
5. Clear error messages for auth failures

## Design Approach

### Authentication Modes

The system will support three authentication modes (configurable):

1. **OIDC Mode**: For human users
   - Authorization code flow with PKCE
   - JWT token validation with remote JWKS
   - Identity provider discovery
   - Support for multiple identity providers

2. **Service Account Mode**: For long-running services
   - Self-signed JWT generation using private keys
   - Local token generation (no external dependencies)
   - Public key registry for validation
   - Short-lived tokens (1 hour) generated on-demand

3. **API Key Mode**: Legacy support (current implementation)
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
    issuer: String,                  // token issuer (OIDC provider or service account ID)
    expires_at: DateTime<Utc>,       // token expiration (chrono for better ergonomics)
    auth_type: AuthType,             // Oidc | ServiceAccount | ApiKey
    is_admin: bool,                  // Simple RBAC - admin flag for service account management
}

// Simple RBAC:
// - is_admin=true allows service account management operations
// - Determined from MICROMEGAS_ADMINS config (list of subjects/emails)
// - Used by admin SQL UDFs to check permissions
//
// Using DateTime<Utc> instead of i64:
// - More type-safe and self-documenting
// - Better API for comparisons and formatting
// - Convert from JWT's i64 (NumericDate) when creating AuthContext
// - Example: DateTime::from_timestamp(jwt_exp, 0)
```

Implementations:
- `OidcAuthProvider` - OIDC/JWT validation with remote JWKS
- `ServiceAccountAuthProvider` - Self-signed JWT validation with local public key registry
- `ApiKeyAuthProvider` - Current key-ring approach

#### 2. JWT Validation
Using `openidconnect` and `jsonwebtoken` crates:

**For OIDC tokens:**
- Fetch JWKS from identity provider's well-known endpoint
- `IdTokenVerifier` for JWT signature validation
- Nonce validation for replay prevention
- Claims extraction (sub, email, exp, aud, iss)
- JWKS cache with TTL refresh

**For service account tokens:**
- Public key registry loaded from database
- JWT signature validation using registered public keys
- Claims extraction (sub, aud, iss, exp)
- Support for multiple signing algorithms (RS256, etc.)

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

#### 4. Service Account Registry
Database-backed registry for service account public keys:
```sql
CREATE TABLE service_accounts (
    id TEXT PRIMARY KEY,              -- service account ID
    public_key TEXT NOT NULL,         -- PEM-encoded RSA public key
    description TEXT,                 -- human-readable description
    created_by TEXT NOT NULL,         -- admin user who created this service account
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    disabled BOOLEAN DEFAULT false
);
```

Features:
- SQL UDFs to create/disable/enable service accounts
- Single public key per service account
- Load into memory at startup, reload on SIGHUP
- Audit trail: track which admin created each service account
- Small table (typically dozens of entries), no indexes needed beyond PK
- **Key rotation**: Not supported - disable old service account and create new one

#### 5. Enhanced Auth Interceptor
Replace current `check_auth` with multi-mode interceptor:
- Extract bearer token from Authorization header
- Determine auth mode (OIDC vs service account vs API key)
- Validate token using appropriate AuthProvider
- Check if user is admin (MICROMEGAS_ADMINS list)
- Cache validation results
- Inject AuthContext into request extensions
- Emit audit logs with user/service identity

#### 6. Admin SQL UDFs (User Defined Functions)
Service account management via SQL functions in DataFusion:

**Create service account:**
```sql
SELECT create_service_account(
    'my-service',                              -- id
    'Data pipeline for production'            -- description
) AS credential_json;

-- Returns JSON credential file (contains private key)
-- Must be saved immediately - cannot be retrieved later
-- created_by is automatically set from authenticated user (AuthContext.subject or email)
-- Cannot be overridden - prevents admin impersonation
```

**List service accounts:**
```sql
SELECT * FROM list_service_accounts();

-- Returns table:
-- id | description | created_by | created_at | disabled
```

**Get service account details:**
```sql
SELECT * FROM get_service_account('my-service');
```

**Disable/enable service account:**
```sql
SELECT disable_service_account('my-service');
SELECT enable_service_account('my-service');
```

**Handling compromised keys (no rotation, create new):**
```sql
-- If a service account key is compromised:

-- 1. Immediately disable the compromised service account
SELECT disable_service_account('my-service');

-- 2. Create a new service account with a fresh keypair
SELECT create_service_account(
    'my-service-v2',
    'Replacement for compromised my-service'
) AS credential_json;

-- 3. Deploy new credential file to services
-- 4. Update services to use new service account
-- 5. After migration complete, old service account remains disabled

-- Simple, clean break - no grace period complexity
```

**Implementation:**
- Each UDF checks `auth_context.is_admin` before executing
- Returns `PermissionDenied` error if not admin
- UDFs are registered with DataFusion SessionContext
- Access auth context from session state (injected by interceptor)
- `created_by` always set from AuthContext (subject or email) - no override allowed
- Prevents admin impersonation and ensures audit trail integrity

### Configuration

Environment variables:

```bash
# General
MICROMEGAS_AUTH_MODE=jwt|api_key|disabled
# jwt mode supports both OIDC and service accounts automatically

# OIDC Configuration (for human users)
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
# Public keys loaded from database via MICROMEGAS_SQL_CONNECTION_STRING
# No additional config needed - service accounts are managed via SQL UDFs

# Admin Configuration (Simple RBAC)
MICROMEGAS_ADMINS='["alice@example.com", "bob@example.com", "admin-service-account"]'
# List of subjects/emails that have admin privileges (can manage service accounts)
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

#### Service Account Flow (Services)
1. Service loads credential file with private key (one-time setup)
2. Service generates JWT locally:
   - Claims: iss=service_account_id, sub=service_account_id, aud=micromegas-analytics, exp=now+1h
   - Signs with private key using RS256
3. Service sends request with `Authorization: Bearer <self_signed_jwt>`
4. flight-sql-srv validates token:
   - Decode JWT header to identify issuer (service_account_id)
   - Check cache for previously validated token
   - If not cached, lookup public key in service account registry
   - Verify JWT signature using registered public key
   - Validate audience, expiration, service account not disabled
   - Extract claims (sub, aud, exp, iss) into AuthContext
   - Convert JWT exp (i64) to DateTime:
     ```rust
     let expires_at = DateTime::from_timestamp(jwt_claims.exp, 0)
         .ok_or("invalid expiration timestamp")?;
     ```
   - Store AuthContext in cache
5. Request proceeds with AuthContext (service identity available for audit logging)

**Key advantages:**
- No external calls for token generation or validation
- Service can generate tokens offline
- Short-lived tokens (1 hour) generated on-demand
- Private key compromise limited to token duration

#### API Key Flow (Legacy)
1. Service sends request with `Authorization: Bearer <api_key>`
2. flight-sql-srv checks KeyRing
3. If found, create minimal AuthContext with service name
4. Request proceeds

### Dependencies

Add to `rust/Cargo.toml` workspace dependencies:
```toml
openidconnect = "4.0"
jsonwebtoken = "9.3"  # For JWT parsing and validation
moka = "0.12"          # For token cache with TTL and thread-safe concurrent access
chrono = "0.4"         # For DateTime types (likely already a dependency)
```

**Why moka over basic lru?**
- Built-in TTL (time-to-live) expiration support
- Thread-safe concurrent access without explicit locking
- High performance with lock-free reads
- Production-proven (powers crates.io)
- Combines LRU eviction with LFU admission policy for better hit rates

Note: `chrono` is likely already in use for timestamp handling throughout the codebase.

## Implementation Phases

### Phase 1: Refactor Current Auth ✅ COMPLETED
- ✅ Extract AuthProvider trait
- ✅ Create ApiKeyAuthProvider wrapping current KeyRing
- ✅ Add AuthContext struct
- ✅ Create separate `micromegas-auth` crate (`rust/auth/`)
- ✅ Add unified JWT validation utilities
- ✅ Add unit tests for API key mode
- ✅ Code style improvements (module-level imports, documented structs)
- ✅ Tests moved to `tests/` directory (separate from source)
- **Status**: Auth crate complete, needs integration with flight-sql-srv

### Phase 2: Add Service Account Support
- Create service_accounts database table (with created_by field)
- Implement ServiceAccountRegistry (load public keys from DB)
- Implement ServiceAccountAuthProvider (JWT validation with local keys)
- Add admin RBAC:
  - Add is_admin field to AuthContext
  - Parse MICROMEGAS_ADMINS config
  - Check admin status during auth
- Implement admin SQL UDFs in DataFusion:
  - `create_service_account(id, description)` → credential JSON
  - `list_service_accounts()` → table function
  - `get_service_account(id)` → table function
  - `disable_service_account(id)` → boolean
  - `enable_service_account(id)` → boolean
  - Each UDF checks auth_context.is_admin
  - created_by automatically extracted from AuthContext (no parameter)
- Add credential file format and generation (RSA keypair)
- Python client: ServiceAccount class with JWT generation
- Add integration tests with test keypairs
- Create example service using self-signed JWTs
- **Goal**: Support service authentication + admin management via SQL

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

### Phase 4: Unified JWT Mode
- Combine OIDC and ServiceAccount into unified JWT mode
- Auto-detect token type based on issuer
- Single configuration: MICROMEGAS_AUTH_MODE=jwt
- Support both OIDC and service accounts simultaneously
- **Goal**: Seamless support for both auth types

### Phase 5: Add Token Caching
- Implement LRU cache for validated tokens
- Add cache configuration
- Add cache metrics/monitoring
- Add cache invalidation on config changes
- **Goal**: Reduce validation overhead to <1ms

### Phase 6: Audit and Security Hardening
- Add comprehensive audit logging with user/service identity
- Add rate limiting per user/service
- Add metrics for auth failures
- Key rotation procedures and documentation
- Security review and penetration testing
- Documentation and deployment guide
- **Goal**: Production-ready authentication

## Security Considerations

1. **Token Storage**: Never log or store tokens, only validation results
2. **Private Key Protection** (Service Accounts):
   - Store credential files with restrictive permissions (0600)
   - Use secret managers in production (not env vars or config files)
   - Never commit credential files to git
   - **Key rotation**: If compromised, disable service account and create a new one
3. **Public Key Registry**: Load from database, cache in memory, single key per service account
4. **JWKS Caching**: Refresh JWKS periodically to detect identity provider key rotation
5. **Clock Skew**: Allow configurable clock skew (default 60s) for exp/nbf validation
6. **TLS/HTTPS**: Handled by load balancer - all traffic encrypted in transit
7. **Token Revocation**:
   - Two caches affect revocation timing:
     - **Service account registry cache** (5 min TTL): Public keys loaded from DB
     - **Token validation cache** (5 min TTL): Recently validated tokens
   - When service account disabled in database:
     - Server reloads registry after 5 minutes (sees disabled flag)
     - Server stops accepting NEW tokens from that service account
     - However, EXISTING valid tokens remain in validation cache (5 min)
     - AND existing tokens are valid until expiration (1 hour from creation)
   - **Worst case revocation delay**: ~65 minutes
     - 5 min: cache refresh to see disabled service account
     - 60 min: existing token valid until expiration
   - **For immediate revocation**: Restart analytics server to clear all caches
8. **Audience Validation**: Strictly validate token audience to prevent token substitution
9. **Audit Logging**: Log all authentication attempts (success and failure) with identity information
10. **Admin Impersonation Prevention**: `created_by` field always set from AuthContext, cannot be overridden by admin parameter - ensures audit trail integrity and prevents admins from impersonating other admins in service account creation records

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

   Admin (alice@example.com) authenticates with OIDC and runs SQL commands via FlightSQL:

   ```sql
   -- Admin creates service account via SQL UDF
   -- created_by is automatically set to alice@example.com from AuthContext
   SELECT create_service_account(
       'my-service',
       'Data pipeline production service'
   ) AS credential_json;

   -- Returns:
   -- {
   --   "type": "service_account",
   --   "service_account_id": "my-service",
   --   "private_key": "-----BEGIN PRIVATE KEY-----\n...",
   --   "token_uri": "https://analytics.example.com",
   --   "audience": "micromegas-analytics"
   -- }

   ⚠️  Save this JSON to a file - it cannot be recovered!

   -- List service accounts (audit - see who created what)
   SELECT * FROM list_service_accounts();

   -- Returns table:
   -- id           | description                    | created_by          | created_at          | disabled
   -- my-service   | Data pipeline production...    | alice@example.com   | 2024-01-15 10:30    | false
   -- data-pipeline| Legacy pipeline                | bob@example.com     | 2024-01-10 14:22    | false
   ```

   Service uses credential file:
   ```bash
   # Update service to use credential file
   # Service loads my-service.json and generates JWTs

   # Test in parallel - both API key and service account work

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
   - Services → Service accounts (generate credential files)
   - Development → Disabled (`--disable-auth`)

2. Service account setup via SQL UDFs:
   ```sql
   -- Admin connects to flight-sql-srv with admin credentials (OIDC or admin service account)

   -- Create service account
   -- created_by automatically set from authenticated user
   SELECT create_service_account(
       'data-pipeline-prod',
       'Production data pipeline'
   ) AS credential_json;

   -- Save the returned JSON to data-pipeline-prod.json
   -- Distribute credential file to service
   -- Service generates tokens locally, no OAuth server needed

   -- Audit: See who created what
   SELECT * FROM list_service_accounts();

   SELECT * FROM get_service_account('data-pipeline-prod');
   ```

3. OIDC setup:
   - Register app with identity provider (Google/Azure/Okta)
   - Configure `MICROMEGAS_OIDC_ISSUERS`
   - Users login via standard OIDC flow

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
   - Auth migration: API keys → Service accounts
   - Other planned improvements (TBD - what other features needed?)
   - Single release, clean break

2. **Service account authentication:**
   - Load credential file from Grafana configuration
   - Generate self-signed JWTs before each query
   - Token generation helper library (TypeScript/JavaScript)
   - Credential file path in datasource config

3. **Updated datasource configuration UI:**
   ```
   Authentication Method:
   ☑ Service Account Credential File
   Path: /path/to/service-account.json
   [Browse...]

   [ Test Connection ]
   ```

4. **Migration strategy:**
   - Major version bump
   - Clear migration guide in release notes
   - Example: "Grafana plugin v2.0: Migrate from API keys to service accounts"
   - Breaking change, but one-time clean migration

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

2. **Service Account Support**
   ```python
   from micromegas.auth import ServiceAccountAuthProvider
   from micromegas.flightsql.client import FlightSQLClient

   # Create auth provider from credential file
   auth = ServiceAccountAuthProvider.from_file("my-service.json")

   # Create client once with auth provider
   client = FlightSQLClient(
       "grpc+tls://analytics.example.com:50051",
       auth_provider=auth
   )

   # Use client multiple times - tokens auto-generated before each query
   df1 = client.query("SELECT * FROM logs WHERE time > now() - interval '1 hour'")
   df2 = client.query("SELECT * FROM metrics WHERE service = 'api'")
   # Each query automatically calls auth.get_token() which generates fresh JWT if needed
   ```

   **Implementation details**:
   - `ServiceAccountAuthProvider` loads private key from credential file
   - `get_token()` generates new 1-hour JWT on each call (cheap operation)
   - Or optionally caches token until expiration (reuse for ~1 hour)
   - JWT signed locally using `jsonwebtoken` library (no external calls)

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

1. **Service Account Support**
   ```bash
   $ micromegas query "SELECT ..." --service-account-file=my-service.json
   ```

2. **OIDC Support (Simple - Browser on Every Call)**
   ```bash
   # Opens browser for each command (no token caching)
   $ micromegas query "SELECT * FROM logs" --oidc
   Opening browser for authentication...
   # User authenticates, command executes, done

   # For frequent CLI use, prefer service accounts instead:
   $ micromegas query "SELECT ..." --service-account-file=my-service.json
   ```

3. **Design Rationale**:
   - CLI usage is infrequent, so browser popup is acceptable
   - No token storage = simpler code, better security (no credentials on disk)
   - No refresh logic needed in CLI
   - For automation/frequent use → service accounts
   - For interactive/long-running → Python client (with auto-refresh)

4. **Implementation**:
   - Full OIDC flow each invocation
   - Short-lived local callback server (e.g., http://localhost:8080/callback)
   - OAuth redirect handling
   - Use token immediately, then discard

### SDKs (Rust, others)
**Required Changes**:
1. Token generation helpers
2. Credential file loading
3. Examples and documentation

### API Key Deprecation Timeline
**Deprecation Plan - "Soon"**:

- **Phase 1**: Release service account support
  - Server: Service account auth working
  - Python client: ServiceAccount class
  - CLI: Service account support
  - **API keys still work** (backward compatibility maintained)

- **Phase 2**: Grafana plugin major update
  - Bundle auth migration with other planned improvements
  - Single release: service accounts + new features
  - Migration guide published
  - Deprecation warning in API key flow

- **Phase 3**: Python client OIDC support
  - OIDC support with auto-refresh
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
   - How to configure JWT auth mode
   - Identity provider setup (Google, Azure AD, Okta)
   - Admin configuration (MICROMEGAS_ADMINS list)
   - Service account management via SQL UDFs:
     - Creating service accounts
     - Listing and auditing service accounts
     - Disabling/enabling service accounts
     - Handling compromised keys (disable + create new)
   - Migration from API keys to service accounts

2. **User Guide**:
   - How to obtain OIDC tokens for CLI/SDK access
   - Service account credential file usage
   - Token generation in services (code examples)
   - Troubleshooting auth failures

3. **Developer Guide**:
   - AuthProvider trait implementation
   - Service account integration examples (Rust, Python, etc.)
   - OIDC auto-refresh implementation patterns
   - Testing auth changes
   - Security best practices

4. **Integration Guide**:
   - Grafana plugin v2.0: Migration guide for bundled update (auth + features)
   - Python client: Auth provider pattern implementation
   - Python client: OidcAuthProvider with automatic token refresh
   - Python client: ServiceAccountAuthProvider with JWT generation
   - CLI: Simple OIDC flow (browser on each call)
   - Other client SDK updates

5. **Python Library Implementation Guide**:
   - Auth provider interface (Protocol)
   - ServiceAccountAuthProvider: JWT generation with private keys
   - OidcAuthProvider: Token storage format and refresh logic
   - Thread-safe token refresh for concurrent queries
   - Error handling (network failures, expired refresh tokens)
   - Browser-based auth flow integration
   - Testing with mock auth providers

## Open Questions

1. Should we support multiple OIDC providers simultaneously? (e.g., Google + Azure AD)
   - **Answer: Yes** - Configuration already supports multiple issuers
2. Do we need role-based access control (RBAC) or is identity sufficient?
   - **Decision: Simple RBAC** - Single `is_admin` flag for service account management
   - Admin determined by MICROMEGAS_ADMINS config (list of subjects/emails)
   - Sufficient for service account administration via SQL UDFs
3. What's the token refresh strategy for long-running queries?
   - **Services**: Generate new tokens on-demand (1 hour lifetime, no external calls)
   - **OIDC users**: Python client auto-refreshes tokens transparently using refresh tokens
   - **Long queries**: Token refresh happens mid-query if needed (Python client handles it)
4. Do we need emergency token revocation support (JTI blacklist)?
   - **Decision: No** - Use short token lifetime (1 hour) + service account disable instead
5. Timeline for API key deprecation?
   - **Decision: Soon** - Phased approach with backward compatibility
   - Grafana plugin will be updated in single release (auth + other improvements)
   - Final removal when migration complete (major version bump)
6. Should Grafana plugin support both API keys and service accounts during transition?
   - **Decision: No** - Single bundled update with breaking change
   - Clean major version bump, bundle with other improvements
   - Clear migration guide, but no dual-mode complexity
7. Python client library: automatic token generation or manual?
   - **Decision: Automatic via auth provider pattern**
   - Client takes auth_provider parameter (ServiceAccountAuthProvider or OidcAuthProvider)
   - Auth provider handles token generation/refresh transparently before each query
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

### No Key Rotation - Create New Instead

**Decision**: No `rotate_service_account_key()` function. If compromised, disable and create new.

**Rationale:**

1. **Simplicity**
   - No grace period logic (which keys are valid when?)
   - No multiple active keys per service account
   - Clear audit trail (old disabled, new created)

2. **Rare Operation**
   - Key compromise should be rare with proper security
   - When it happens, immediate action is better than gradual rotation
   - Clean break forces immediate remediation

3. **Clear Security Model**
   - Disabled = immediately invalid (after cache TTL)
   - No ambiguity about which key is active
   - Service account ID changes (my-service → my-service-v2) makes migration explicit

4. **Less Code**
   - No rotation UDF
   - No grace period tracking in database
   - No multiple-key validation logic

**Handling Compromise:**
```sql
-- Simple, explicit process:
SELECT disable_service_account('compromised-service');
SELECT create_service_account('compromised-service-v2', 'Replacement') AS new_creds;
-- Deploy new creds, done
```

**Alternative considered**: Rotation with grace period
- More complex (track multiple keys, grace period expiry)
- Benefits marginal (still need to deploy new creds)
- Adds code and testing burden

### Why SQL UDFs Instead of HTTP Endpoints or CLI?

**The Problem:**
- Aurora PostgreSQL is VPC-internal (not publicly accessible)
- CLI tools need database access
- Can't give everyone (e.g., Grafana users) direct DB access
- FlightSQL service is the public interface

**The Solution: Admin SQL UDFs**
- Admins connect to flight-sql-srv (already exposed)
- Service account management via SQL functions
- Authentication/authorization already handled by auth interceptor
- Simple RBAC: check `is_admin` flag before executing UDFs

**Benefits:**

1. **No additional infrastructure**
   - No separate admin HTTP service
   - No CLI tools requiring VPC access
   - Reuses existing FlightSQL connection

2. **Natural interface for SQL service**
   - FlightSQL service → SQL interface for admin operations
   - Consistent with existing query workflow
   - Admins already connect via FlightSQL

3. **Security built-in**
   - AuthContext already has user identity
   - Simple is_admin check in each UDF
   - Audit logging automatic (caller identity known)

4. **Future extensibility**
   - Python client can wrap SQL UDFs in nice API
   - Web UI can call SQL UDFs via FlightSQL
   - Any FlightSQL client can manage service accounts (if admin)

**Example Admin Workflow:**
```bash
# Admin (alice@example.com) connects with their OIDC credentials
$ python
>>> from micromegas.auth import OidcAuthProvider
>>> from micromegas.flightsql.client import FlightSQLClient
>>>
>>> # Create auth provider (loads saved OIDC tokens)
>>> auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")
>>>
>>> # Create client with auth provider
>>> client = FlightSQLClient(
...     "grpc+tls://analytics.example.com:50051",
...     auth_provider=auth
... )

# Run SQL to create service account
# created_by is automatically set to alice@example.com from auth context
>>> result = client.query("""
    SELECT create_service_account(
        'data-pipeline',
        'Production data pipeline'
    ) AS credential_json
""")

# Save credential file (result is a DataFrame)
>>> with open('data-pipeline.json', 'w') as f:
...     f.write(result['credential_json'].iloc[0])

# List service accounts - see who created what
>>> accounts = client.query("SELECT * FROM list_service_accounts()")
>>> print(accounts)
# Shows alice@example.com as created_by for data-pipeline
```

**vs HTTP Endpoints:**
- Would need separate admin service or add HTTP to flight-sql-srv
- Mixed protocols (gRPC FlightSQL + HTTP admin) feels inconsistent
- More complex routing and deployment

**vs CLI Tool:**
- Would need VPC access or call HTTP endpoints
- If calling HTTP, why not just use SQL?
- SQL UDFs give us both: direct DB access + high-level interface

## References

### Rust Crates
- [openidconnect crate docs](https://docs.rs/openidconnect/latest/openidconnect/) - OpenID Connect client library for Rust
- [jsonwebtoken crate docs](https://docs.rs/jsonwebtoken/latest/jsonwebtoken/) - JWT encoding/decoding and validation
- [moka crate docs](https://docs.rs/moka/latest/moka/) - High-performance concurrent caching library with TTL support
- [moka GitHub](https://github.com/moka-rs/moka) - Examples and architecture documentation

### OAuth and OpenID Connect Standards
- [OAuth 2.0 RFC 6749](https://datatracker.ietf.org/doc/html/rfc6749) - OAuth 2.0 Authorization Framework
- [OIDC Core Spec](https://openid.net/specs/openid-connect-core-1_0.html) - OpenID Connect Core 1.0 specification
- [PKCE RFC 7636](https://datatracker.ietf.org/doc/html/rfc7636) - Proof Key for Code Exchange by OAuth Public Clients
- [JWT RFC 7519](https://datatracker.ietf.org/doc/html/rfc7519) - JSON Web Token (JWT) specification

### DataFusion Documentation
- [DataFusion UDF Guide](https://datafusion.apache.org/library-user-guide/functions/adding-udfs.html) - Adding User Defined Functions (Scalar/Window/Aggregate)
- [ScalarUDF API](https://docs.rs/datafusion/latest/datafusion/logical_expr/struct.ScalarUDF.html) - DataFusion Rust API for scalar UDFs
