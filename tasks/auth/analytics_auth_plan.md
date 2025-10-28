# Analytics Server Authentication Plan

## 🎯 Quick Status Summary

**Phase 1 (Server-Side OIDC):** ✅ **COMPLETE** (2025-01-24)
- Multi-provider authentication (API key + OIDC) working in flight-sql-srv
- Validated with Google OAuth and Auth0
- Ready for Azure AD, Okta, or any OIDC provider
- See [OIDC Implementation Subplan](oidc_auth_subplan.md) for details

**Phase 2 (Python Client OIDC):** ✅ **COMPLETE** (2025-10-27)
- Browser-based login with PKCE
- Token persistence to ~/.micromegas/tokens.json
- Automatic token refresh with 5-minute buffer
- Tested with Google OAuth and Auth0 (public client)
- See [OIDC Implementation Subplan](oidc_auth_subplan.md) for details

**Phase 3 (Testing):** ✅ **COMPLETE** (2025-10-27)
- Unit tests for Python client (6 tests passing)
- Integration test suite (test_oidc_integration.py)
- End-to-end testing with Google identity provider ✅
- End-to-end testing with Auth0 identity provider ✅
- Provider-agnostic test scripts

**Phase 4 (Service Accounts):** 📋 Planned
- Server already supports validating service account tokens (Phase 1)
- Need Python/Rust client credentials providers
- OAuth 2.0 client credentials flow implementation

**Phase 5 (CLI):** ✅ **COMPLETE** (2025-10-28)
- CLI integration with token persistence
- Browser login on first use
- Automatic token reuse across CLI tools
- Logout command (micromegas_logout)
- Backward compatible with MICROMEGAS_PYTHON_MODULE_WRAPPER

---

## Overview

Enhance the flight-sql-srv authentication to support both human users (via OIDC) and long-running services (via OAuth 2.0 client credentials), using the `openidconnect` crate.

## Current State (Updated 2025-10-27)

### Implementation Status

**✅ Phase 1 (Server-Side OIDC): COMPLETE** (2025-01-24)
- ✅ Separate `micromegas-auth` crate at `rust/auth/`
- ✅ AuthProvider trait, AuthContext struct, AuthType enum
- ✅ ApiKeyAuthProvider (existing API key system)
- ✅ OidcAuthProvider (OIDC validation with JWKS caching)
- ✅ Unit tests in `tests/` directory (10 tests + 2 doc tests passing)
- ✅ Multi-provider authentication (API key + OIDC simultaneously)
- ✅ Integrated into flight-sql-srv with async tower service
- ✅ Environment variable configuration
- ✅ Backward compatible with `--disable_auth` flag
- ✅ Tested with Google OAuth and Auth0

**✅ Phase 2 (Python Client OIDC): COMPLETE** (2025-10-27)
- ✅ `OidcAuthProvider` class in `python/micromegas/micromegas/auth/oidc.py`
- ✅ Browser-based login with PKCE (authorization code flow)
- ✅ Token persistence to ~/.micromegas/tokens.json (0600 permissions)
- ✅ Automatic token refresh with 5-minute expiration buffer
- ✅ Thread-safe token refresh for concurrent queries
- ✅ Support for true public clients (PKCE without client_secret)
- ✅ Support for Web apps (PKCE + client_secret)
- ✅ FlightSQLClient integration via `auth_provider` parameter
- ✅ Unit tests (6 tests covering token lifecycle)
- ✅ Dependencies: authlib ^1.3.0, requests ^2.32.0
- ✅ Tested with Google OAuth and Auth0

**✅ Phase 3 (Testing): COMPLETE** (2025-10-27)
- ✅ Provider-agnostic test scripts (start_services_with_oidc.py, test_oidc_auth.py)
- ✅ Integration test suite (test_oidc_integration.py)
- ✅ End-to-end testing with Google OAuth (Desktop app with secret)
- ✅ End-to-end testing with Auth0 (Native app - true public client, no secret)
- ✅ Documentation: GOOGLE_OIDC_SETUP.md, AUTH0_TEST_GUIDE.md, WEB_APP_OIDC.md

**🔜 Next:** Phase 4 (Service Accounts - OAuth 2.0 Client Credentials)

### Existing Implementation (Now Enhanced)
- ✅ Bearer token authentication via async AuthProvider
- ✅ API keys via ApiKeyAuthProvider (HashMap lookup - fast path)
- ✅ OIDC tokens via OidcAuthProvider (JWT validation - secondary)
- ✅ Keys loaded from `MICROMEGAS_API_KEYS` environment variable (JSON array)
- ✅ OIDC config from `MICROMEGAS_OIDC_CONFIG` environment variable
- ✅ Can be disabled with `--disable_auth` flag
- ✅ Full identity information in AuthContext
- ✅ Token expiration validation for OIDC

### Addressed Limitations
- ✅ No support for federated identity providers → OIDC provider implemented & integrated
- ✅ No Python client OIDC support → Browser-based login with token persistence implemented
- ✅ No fine-grained access control → Admin RBAC implemented (is_admin flag)
- ✅ No audit trail of user identity → AuthContext captures and logs identity
- ⏳ Manual API key distribution and rotation → Service accounts planned (Phase 4)
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

#### 4. Enhanced Auth Interceptor (✅ IMPLEMENTED)
Multi-provider async authentication service:
- ✅ Extract bearer token from Authorization header
- ✅ Try API key validation first (fast HashMap lookup)
- ✅ Fall back to OIDC validation (JWT + JWKS)
- ✅ Check if user is admin (MICROMEGAS_ADMINS list)
- ✅ Cache JWKS and validated tokens
- ✅ Inject AuthContext into request extensions
- ✅ Emit audit logs with user/service identity

### Configuration (✅ IMPLEMENTED)

Environment variables:

```bash
# API Key Configuration (existing - backward compatible)
MICROMEGAS_API_KEYS='[{"name": "service1", "key": "secret-key-123"}]'

# OIDC Configuration (new - for both human users and service accounts)
MICROMEGAS_OIDC_CONFIG='{
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

# Admin Configuration (implemented)
MICROMEGAS_ADMINS='["alice@example.com", "bob@example.com"]'
# List of subjects/emails that have admin privileges
# Matches against AuthContext.subject or AuthContext.email
```

CLI flags:
```bash
--disable-auth              # Skip authentication (development) ✅ IMPLEMENTED
```

### Integration with FlightSQL Server (✅ IMPLEMENTED)

Completed modifications to `flight_sql_srv.rs`:

1. ✅ Created `MultiAuthProvider` supporting both API key and OIDC
2. ✅ Initialize providers from environment variables
3. ✅ Async tower service layer for authentication
4. ✅ Store `AuthContext` in request extensions for downstream access
5. ✅ Logging includes user/service identity (subject, email, issuer, admin status)

### Token Validation Flow (✅ IMPLEMENTED)

#### OIDC Flow (Human Users) - Server-Side ✅
1. User authenticates with IdP (external to flight-sql-srv)
2. Client obtains ID token and access token
3. Client sends request with `Authorization: Bearer <id_token>`
4. flight-sql-srv validates token:
   - ✅ Extract bearer token from Authorization header
   - ✅ Try API key validation first (fast O(1) HashMap lookup)
   - ✅ If API key fails, decode JWT header to identify issuer
   - ✅ Check cache for previously validated token
   - ✅ If not cached, fetch JWKS from identity provider
   - ✅ Verify JWT signature using JWKS public keys
   - ✅ Validate issuer, audience, expiration
   - ✅ Extract claims (sub, email, exp, iss) into AuthContext
   - ✅ Store AuthContext in cache and request extensions
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
# OIDC discovery
openidconnect = "4.0"  # For OIDC discovery and metadata

# JWT validation
jsonwebtoken = "9"     # For JWT validation (simpler API, battle-tested)
rsa = "0.9"            # For RSA key handling in JWT verification
base64 = "0.22"        # For base64 decoding in JWKS conversion

# Caching
moka = "0.12"          # For token cache with TTL and thread-safe concurrent access

# DateTime
chrono = "0.4"         # For DateTime types (likely already a dependency)
```

**Why moka over basic lru?**
- Built-in TTL (time-to-live) expiration support
- Thread-safe concurrent access without explicit locking
- High performance with lock-free reads
- Production-proven (powers crates.io)
- Combines LRU eviction with LFU admission policy for better hit rates

**Why hybrid approach (openidconnect + jsonwebtoken)?**
- openidconnect designed for OAuth clients, not server-side token validation
- IdTokenVerifier API is internal/private, not accessible for our use case
- jsonwebtoken provides simple, clear API for JWT validation
- Using each library for what it does well: discovery vs. validation
- Battle-tested combination in production use

Note: `chrono` is likely already in use for timestamp handling throughout the codebase.

## Implementation Phases

### Phase 1: Server-Side OIDC Integration ✅ COMPLETE (2025-01-24)

**Summary:** Successfully integrated OIDC authentication into flight-sql-srv, enabling support for both API key and OIDC authentication simultaneously.

#### Implementation Checklist
- ✅ Extract AuthProvider trait
- ✅ Create ApiKeyAuthProvider wrapping current KeyRing
- ✅ Add AuthContext struct
- ✅ Create separate `micromegas-auth` crate (`rust/auth/`)
- ✅ Add OidcAuthProvider with JWT validation
- ✅ Add unit tests for API key mode and OIDC mode
- ✅ Code style improvements (module-level imports, documented structs)
- ✅ Tests moved to `tests/` directory (separate from source)
- ✅ Wire up AuthProvider in tonic_auth_interceptor.rs
- ✅ Add flight-sql-srv configuration and initialization
- ✅ Multi-provider support (API key + OIDC simultaneously)
- ✅ Async tower service layer for authentication
- ✅ JWKS caching with TTL
- ✅ Token validation caching
- ✅ Admin users support (MICROMEGAS_ADMINS)
- ✅ AuthContext injection for audit logging

#### Key Features Delivered

**1. Multi-Provider Authentication**
- API Key Authentication (fast path - O(1) HashMap lookup)
- OIDC Authentication (JWT validation with JWKS caching)
- Both methods work simultaneously
- Tries API key first, falls back to OIDC
- Users choose their preferred auth method

**2. Auth Crate Components (`rust/auth/`)**
- `AuthProvider` trait for extensible authentication
- `ApiKeyAuthProvider` wrapping existing KeyRing
- `OidcAuthProvider` with JWT validation
- `MultiAuthProvider` supporting both methods
- OIDC discovery using `openidconnect::CoreProviderMetadata::discover_async()`
- JWT validation using `jsonwebtoken` (hybrid approach)
- JWKS caching with TTL using moka
- Token validation caching
- SSRF protection (HTTP client with `redirect(Policy::none())`)
- Test utilities for generating test tokens
- All tests passing (10 tests + 2 doc tests)

**3. Integration with flight-sql-srv**
- Async tower service layer for authentication
- AuthContext injection into request extensions
- Audit logging with user identity (subject, email, issuer, admin status)
- Environment variable configuration
- Backward compatible with `--disable_auth` flag

**4. Authentication Flow**
1. Extract Bearer token from Authorization header
2. Try API key validation (fast HashMap lookup)
3. If API key fails, try OIDC validation:
   - Check token cache
   - Fetch JWKS from OIDC provider (cached with TTL)
   - Verify JWT signature
   - Validate issuer, audience, expiration
   - Extract claims into AuthContext
4. Inject AuthContext into request extensions
5. Log authentication success with identity details

**5. AuthContext Structure**
```rust
pub struct AuthContext {
    pub subject: String,              // Unique user/service identifier
    pub email: Option<String>,        // Email (if available)
    pub issuer: String,               // Identity provider or "api_key"
    pub expires_at: Option<DateTime>, // Token expiration
    pub auth_type: AuthType,          // ApiKey or Oidc
    pub is_admin: bool,               // Admin privilege flag
}
```

#### Configuration Examples

**Environment Variables:**
```bash
# API Key authentication (existing - backward compatible)
MICROMEGAS_API_KEYS='[{"name": "user1", "key": "secret-key-123"}]'

# OIDC authentication (new)
MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "your-app-id.apps.googleusercontent.com"
    }
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'

# Admin users (optional)
MICROMEGAS_ADMINS='["alice@example.com"]'
```

**CLI Flags:**
```bash
flight-sql-srv --disable-auth  # Development mode
```

#### Files Modified
- `rust/auth/` - New crate with all authentication logic
- `rust/public/src/lib.rs` - Added `auth` module export
- `rust/public/src/servers/tonic_auth_interceptor.rs` - Updated to use AuthProvider trait
- `rust/flight-sql-srv/Cargo.toml` - Added dependencies
- `rust/flight-sql-srv/src/flight_sql_srv.rs` - Multi-provider integration

#### Testing Status
- **Unit Tests:** ✅ Complete (10 tests + 2 doc tests passing)
- **Build Status:** ✅ All code compiles successfully
- **Integration Tests:** 📋 Planned for Phase 4

#### What's Supported Now
The flight-sql-srv can now validate:
- ✅ API keys (existing functionality)
- ✅ Google OIDC ID tokens
- ✅ Azure AD OIDC tokens
- ✅ Okta OIDC tokens
- ✅ Any standards-compliant OIDC provider

#### Architecture Benefits
- **Multi-Provider Design**: Users choose their preferred auth method
- **Migration Path**: Existing API key users unaffected
- **Performance**: Fast path for API keys (O(1) lookup)
- **No Breaking Changes**: 100% backward compatible
- **Security**: JWKS caching, token caching, SSRF protection, proper JWT validation
- **Observability**: Every authenticated request logged with full identity context

**Status**: ✅ COMPLETE - Server can validate both API keys and OIDC tokens from multiple providers

### Phase 2: Python Client OIDC Support ✅ COMPLETE (2025-10-27)

**Summary:** Successfully implemented browser-based OIDC authentication in Python client with token persistence and automatic refresh.

**Python Client:**
- ✅ Implemented `OidcAuthProvider` class using authlib
- ✅ Browser-based login flow (authorization code + PKCE)
- ✅ Token storage (access + refresh tokens + id_token)
- ✅ Automatic token refresh with 5-minute expiration buffer
- ✅ Thread-safe token refresh for concurrent queries
- ✅ Token persistence to ~/.micromegas/tokens.json with 0600 permissions
- ✅ Support for true public clients (PKCE without client_secret)
- ✅ Support for Web apps (PKCE + client_secret)
- ✅ `FlightSQLClient` integration via `auth_provider` parameter
- ✅ Port reuse fix for callback server (try/finally block)
- ✅ Unit tests (6 tests covering token lifecycle)

**Testing:**
- ✅ End-to-end testing with Google OAuth (Desktop app with secret)
- ✅ End-to-end testing with Auth0 (Native app - true public client, no secret)
- ✅ Token reuse verified (no browser on second run)
- ✅ Server-side token validation verified

**Goal**: ✅ ACHIEVED - Human users can authenticate via browser with transparent token refresh

### Phase 3: Integration Testing ✅ COMPLETE (2025-10-27)

**Summary:** Comprehensive testing with real OIDC providers and provider-agnostic test infrastructure.

- ✅ Provider-agnostic test scripts (start_services_with_oidc.py, test_oidc_auth.py)
- ✅ Integration test suite (test_oidc_integration.py)
- ✅ End-to-end testing with Google OAuth
- ✅ End-to-end testing with Auth0
- ✅ Multi-issuer server configuration validated
- ✅ Token validation caching verified
- ✅ JWKS caching verified
- ✅ Server audit logging verified (user identity extraction)
- ✅ Documentation: GOOGLE_OIDC_SETUP.md, AUTH0_TEST_GUIDE.md, WEB_APP_OIDC.md

**Future Testing:**
- ⏳ wiremock tests with mock OIDC provider (deferred)
- ⏳ Azure AD and Okta testing (ready, not tested yet)

**Goal**: ✅ ACHIEVED - Multi-provider OIDC authentication validated end-to-end

### Phase 4: Service Account Support (OAuth 2.0 Client Credentials) - PLANNED

**Note**: Server already supports validating these tokens (Phase 1 complete)

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
- **Goal**: Support service authentication via standard OAuth 2.0 flow

### Phase 5: CLI Integration ✅ COMPLETE (2025-10-28)

**Summary:** CLI tools now support OIDC authentication with token persistence and automatic refresh.

**CLI Integration:**
- ✅ Updated `cli/connection.py` to support OIDC
- ✅ Token persistence across CLI invocations (shared with Python client at ~/.micromegas/tokens.json)
- ✅ Browser login only on first use or expiration
- ✅ Environment variable configuration (MICROMEGAS_OIDC_ISSUER, MICROMEGAS_OIDC_CLIENT_ID, MICROMEGAS_OIDC_CLIENT_SECRET)
- ✅ Token sharing between Python client and CLI
- ✅ Logout command: `micromegas_logout` to clear saved tokens
- ✅ Backward compatible with MICROMEGAS_PYTHON_MODULE_WRAPPER (corporate auth wrapper)

**Implementation Details:**
- Updated `python/micromegas/cli/connection.py` with `_connect_with_oidc()` function
- Uses `OidcAuthProvider.from_file()` when tokens exist, `OidcAuthProvider.login()` when not
- Falls back to token re-authentication on refresh failure
- Created `python/micromegas/cli/logout.py` with logout command
- Added `micromegas_logout` script entry point to pyproject.toml
- All existing CLI tools work without modification (query_processes.py, query_process_log.py, query_process_metrics.py)

**User Experience:**
```bash
# Set environment variables
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="your-app-id.apps.googleusercontent.com"
export MICROMEGAS_OIDC_CLIENT_SECRET="your-secret"  # Optional for some providers

# First use: Opens browser for authentication
python3 -m micromegas.cli.query_processes --since 1h

# Subsequent uses: No browser interaction, tokens auto-refresh
python3 -m micromegas.cli.query_process_log <process_id>

# Clear saved tokens
micromegas_logout
```

**Testing:**
- ✅ Verified with Google OAuth (Desktop app)
- ✅ Verified with Auth0 (Native app - public client)
- ✅ Verified with Azure AD (Desktop app)
- ✅ Token persistence works across all CLI tools
- ✅ Automatic token refresh verified
- ✅ Logout command clears tokens

**Goal**: ✅ ACHIEVED - CLI tools support OIDC authentication with token reuse

### Phase 6: Audit and Security Hardening - FUTURE
- Enhance audit logging with structured events
- Add rate limiting per user/service
- Add metrics for auth failures
- Document credential management in OIDC providers
- Security review and penetration testing
- Documentation and deployment guide
- **Goal**: Production-ready authentication

## Security Considerations

1. **Token Storage**: ✅ Never log or store tokens, only validation results
2. **Client Secret Protection** (Service Accounts - Future Phase 2):
   - Store client secrets in secret managers (AWS Secrets Manager, GCP Secret Manager, Azure Key Vault)
   - Never commit client secrets to git or config files
   - Use environment variables or secret mounting for runtime access
   - Rotate client secrets periodically in OIDC provider
3. **JWKS Caching**: ✅ Implemented with configurable TTL (default 1 hour) to detect key rotation
4. **Clock Skew**: ✅ Handled by jsonwebtoken library (60s tolerance)
5. **TLS/HTTPS**: Handled by load balancer - all traffic encrypted in transit
6. **Token Revocation**:
   - ✅ Token validation cache implemented (configurable TTL, default 5 min)
   - When user/service account disabled in OIDC provider:
     - OIDC provider stops issuing NEW tokens immediately
     - Server continues to accept EXISTING valid tokens until cache expiry (5 min) + token expiration (~1 hour)
   - **Worst case revocation delay**: ~65 minutes (5 min cache + 60 min token)
   - **For immediate revocation**: Restart analytics server to clear validation cache
   - **Better approach**: Use short-lived tokens (15-30 min) to reduce revocation window
7. **Audience Validation**: ✅ Strictly validates token audience to prevent token substitution
8. **Audit Logging**: ✅ Logs all authentication attempts with identity (subject, email, issuer, admin status)
9. **OIDC Provider Security**: ✅ Relies on OIDC provider's security (Google, Azure AD, Okta)
10. **SSRF Protection**: ✅ HTTP client configured with `redirect(Policy::none())`

## Testing Strategy

1. **Unit Tests**: ✅ IMPLEMENTED
   - ✅ Each AuthProvider implementation (API key, OIDC)
   - ✅ Token validation logic
   - ✅ Claims extraction
   - ✅ Expired token handling
   - ✅ 10 tests + 2 doc tests passing

2. **Integration Tests**: PLANNED
   - Mock OIDC provider (using wiremock)
   - End-to-end FlightSQL requests with auth
   - Multi-issuer scenarios
   - JWKS cache refresh behavior

3. **Performance Tests**: PLANNED
   - Auth interceptor latency
   - Cache hit/miss ratios
   - Concurrent auth request handling
   - JWKS fetch impact

4. **Security Tests**: PLANNED
   - Invalid token rejection
   - Expired token rejection
   - Wrong audience/issuer rejection
   - Signature verification

## Migration Path

### For Existing API Key Users
**✅ Backward-compatible transition implemented:**

1. **Current release**: Multi-provider support
   - ✅ API keys still work (no breaking changes)
   - ✅ OIDC tokens work alongside API keys
   - ✅ Server tries API key first, then OIDC
   - ✅ Choose authentication method per client

2. **Migration process (when ready):**

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

- **Phase 1**: Release OAuth 2.0 client credentials support (Planned)
  - Server: ✅ OIDC auth validates tokens from both human users and service accounts
  - Python client: ⏳ OidcClientCredentialsProvider class (not implemented yet)
  - CLI: ⏳ OAuth 2.0 client credentials support (not implemented yet)
  - **API keys still work** (backward compatibility maintained)

- **Phase 2**: Grafana plugin major update (Planned)
  - Bundle auth migration with other planned improvements
  - Single release: OAuth 2.0 client credentials + new features
  - Migration guide published
  - Deprecation warning in API key flow

- **Phase 3**: Python client OIDC support (for human users) ✅ COMPLETE (2025-10-27)
  - ✅ OIDC authorization code flow with auto-refresh
  - ✅ Token persistence and reuse
  - ✅ Tested with Google OAuth and Auth0
  - ⏳ Deprecation warnings for API key usage (not added yet)
  - ⏳ Migration documentation (partially complete)

- **Phase 4**: Deprecation enforcement (Future)
  - API keys disabled by default
  - Opt-in flag: `--enable-legacy-api-keys` (for stragglers)
  - Strong migration communication

- **Phase 5**: API key removal (Future)
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

