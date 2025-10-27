# OIDC Authentication Implementation Plan

## Overview

Implement OpenID Connect (OIDC) authentication for the flight-sql-srv analytics server, enabling human users to authenticate via identity providers (Google, Azure AD, Okta, etc.) with automatic token refresh.

**Parent Plan:** [Analytics Server Authentication Plan](analytics_auth_plan.md)

**Focus:** OIDC authentication only - service accounts will be implemented separately.

## Goals

1. **Server-side:** Validate OIDC ID tokens from multiple identity providers
2. **Python client:** Browser-based login with automatic token refresh and persistence
3. **CLI:** Token persistence with browser login only when needed
4. **Backward compatible:** Existing API key auth continues to work

## Current State

### Existing Auth Implementation
- Simple bearer token via `check_auth` (tonic_auth_interceptor.rs:10)
- API keys in `KeyRing` HashMap (key_ring.rs:51)
- Can be disabled with `--disable_auth` flag
- No identity information or token expiration

### Limitations
- No federated identity providers
- No user context for audit logging
- No automatic token refresh

## Requirements

### Server-Side
1. Validate ID tokens from configured OIDC providers
2. Verify JWT signature using provider's JWKS
3. Validate issuer, audience, expiration claims
4. Extract user identity (sub, email) for audit logging
5. Cache JWKS with TTL refresh (avoid fetching on every request)
6. Cache validated tokens to reduce overhead
7. Support multiple identity providers simultaneously

### Python Client
1. Browser-based login flow (authorization code + PKCE)
2. Token storage (access token + refresh token + expiration)
3. Automatic token refresh with 5-minute buffer before expiration
4. Thread-safe token refresh for concurrent queries
5. Token persistence across sessions (save to ~/.micromegas/tokens.json)
6. Retry logic for 401 responses

### CLI
1. Token persistence to ~/.micromegas/tokens.json (same as Python client)
2. Browser-based login on first use or when tokens expire
3. Automatic token refresh using saved tokens
4. `logout` command to clear saved tokens
5. Simple user experience (browser only opens when needed)

## Architecture

### Server Components

#### 1. OidcAuthProvider
```rust
use openidconnect::{
    core::{CoreClient, CoreProviderMetadata, CoreIdTokenVerifier},
    IssuerUrl, ClientId, RedirectUrl, AuthenticationFlow,
    TokenResponse, OAuth2TokenResponse,
};
use moka::sync::Cache;

pub struct OidcAuthProvider {
    // One client per configured issuer
    clients: HashMap<String, OidcIssuerClient>,
    // Cache for validated tokens
    token_cache: Cache<String, AuthContext>,
}

struct OidcIssuerClient {
    issuer: String,
    audience: String,
    client: CoreClient,
    jwks_cache: JwksCache,
}

impl OidcAuthProvider {
    pub async fn new(config: OidcConfig) -> Result<Self> {
        // For each configured issuer:
        // 1. Discover provider metadata from /.well-known/openid-configuration
        // 2. Create CoreClient for token validation
        // 3. Set up JWKS cache
    }
}

impl AuthProvider for OidcAuthProvider {
    async fn validate_token(&self, token: &str) -> Result<AuthContext> {
        // 1. Check token cache
        if let Some(cached) = self.token_cache.get(token) {
            return Ok(cached);
        }

        // 2. Decode JWT header to identify issuer
        let header = decode_header(token)?;
        let issuer_client = self.clients.get(&header.issuer)?;

        // 3. Fetch JWKS from cache or provider
        let jwks = issuer_client.jwks_cache.get().await?;

        // 4. Verify signature using JWKS
        let verifier = CoreIdTokenVerifier::new_public_client(
            issuer_client.client.client_id().clone(),
            jwks,
        );
        let id_token = verifier.verify(token)?;

        // 5. Validate claims
        let claims = id_token.claims();
        validate_issuer(claims.issuer(), &issuer_client.issuer)?;
        validate_audience(claims.audiences(), &issuer_client.audience)?;
        validate_expiration(claims.expiration())?;

        // 6. Create AuthContext
        let auth_ctx = AuthContext {
            subject: claims.subject().to_string(),
            email: claims.email().map(|e| e.to_string()),
            issuer: claims.issuer().to_string(),
            expires_at: DateTime::from_timestamp(claims.expiration().timestamp(), 0)?,
            auth_type: AuthType::Oidc,
            is_admin: self.check_admin(claims.subject(), claims.email()),
        };

        // 7. Cache validated token
        self.token_cache.insert(token.to_string(), auth_ctx.clone());

        Ok(auth_ctx)
    }
}
```

#### 2. JWKS Cache and JWT Validation

**IMPORTANT:** Use `openidconnect` for both OIDC discovery and JWT validation. The openidconnect crate provides built-in methods that handle edge cases properly and are standards-compliant.

**Recommended Approach:**
- Use `openidconnect::CoreProviderMetadata::discover_async()` for OIDC discovery
- Use `openidconnect::IdTokenVerifier` for JWT validation (standards-compliant, secure)
- Use `moka` for caching validated tokens with automatic TTL expiration

```rust
use moka::future::Cache;
use openidconnect::core::{CoreProviderMetadata, CoreJsonWebKeySet};
use openidconnect::{IssuerUrl, HttpRequest, HttpResponse};
use std::time::Duration;

struct JwksCache {
    issuer_url: IssuerUrl,
    cache: Cache<String, Arc<CoreJsonWebKeySet>>,
}

impl JwksCache {
    fn new(issuer_url: IssuerUrl, ttl: Duration) -> Self {
        // Create cache with TTL for automatic expiration
        let cache = Cache::builder()
            .time_to_live(ttl)  // Auto-expire after TTL
            .build();

        Self {
            issuer_url,
            cache,
        }
    }

    async fn get(&self) -> Result<Arc<CoreJsonWebKeySet>> {
        // Use moka's get_or_try_insert to handle cache miss atomically
        // This prevents duplicate fetches when multiple threads miss cache simultaneously
        let jwks = self.cache
            .try_get_with("jwks".to_string(), async {
                Self::fetch_jwks(&self.issuer_url).await
            })
            .await
            .map_err(|e| anyhow!("Failed to fetch JWKS: {}", e))?;

        Ok(jwks)
    }

    async fn fetch_jwks(issuer_url: &IssuerUrl) -> Result<Arc<CoreJsonWebKeySet>> {
        // Use openidconnect's built-in OIDC discovery
        // This handles /.well-known/openid-configuration discovery properly
        let metadata = CoreProviderMetadata::discover_async(
            issuer_url.clone(),
            async_http_client,  // Use openidconnect's HTTP client
        )
        .await
        .map_err(|e| anyhow!("Failed to discover OIDC metadata: {}", e))?;

        // Fetch JWKS from jwks_uri (also built into openidconnect)
        let jwks = metadata
            .jwks()
            .keys()
            .clone();

        Ok(Arc::new(CoreJsonWebKeySet::new(jwks)))
    }
}
```

**Note:** Current implementation needs refactoring to use `openidconnect` as much as possible:
- Use `openidconnect::CoreProviderMetadata::discover_async()` for OIDC discovery
- Use `openidconnect::IdTokenVerifier` for JWT validation
- Benefits: Better error handling, standards compliance, proper security checks, less custom code

**Benefits of using moka for JWKS cache:**
- Automatic TTL expiration (no manual timestamp checking)
- Thread-safe without manual locking (Arc<RwLock<>>)
- `try_get_with()` prevents duplicate fetches during cache miss
- Consistent pattern with token cache
- Simpler, less error-prone code

#### 3. Configuration
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct OidcConfig {
    pub issuers: Vec<OidcIssuer>,
    pub jwks_refresh_interval_secs: u64,  // Default: 3600 (1 hour)
    pub token_cache_size: usize,          // Default: 1000
    pub token_cache_ttl_secs: u64,        // Default: 300 (5 min)
}

#[derive(Debug, Clone, Deserialize)]
pub struct OidcIssuer {
    pub issuer: String,    // e.g., "https://accounts.google.com"
    pub audience: String,  // e.g., "your-app-id.apps.googleusercontent.com"
}

// Load from environment variable
impl OidcConfig {
    pub fn from_env() -> Result<Self> {
        let json = std::env::var("MICROMEGAS_OIDC_CONFIG")?;
        let config: OidcConfig = serde_json::from_str(&json)?;
        Ok(config)
    }
}
```

Environment variable example:
```bash
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
```

#### 4. Integration with Auth Interceptor
```rust
// In flight_sql_srv.rs

// Initialize auth provider based on mode
let auth_provider: Box<dyn AuthProvider> = match auth_mode {
    AuthMode::Oidc => {
        let config = OidcConfig::from_env()?;
        Box::new(OidcAuthProvider::new(config).await?)
    }
    AuthMode::ApiKey => {
        let keyring = KeyRing::from_env()?;
        Box::new(ApiKeyAuthProvider::new(keyring))
    }
    AuthMode::Disabled => {
        Box::new(NoAuthProvider)
    }
};

// Auth interceptor remains the same
fn check_auth(req: Request<()>) -> Result<Request<()>, Status> {
    let token = extract_bearer_token(&req)?;
    let auth_ctx = auth_provider.validate_token(token).await?;

    // Inject AuthContext into request extensions
    req.extensions_mut().insert(auth_ctx);

    Ok(req)
}
```

### Python Client Components

#### 1. OidcAuthProvider (using authlib)
```python
from typing import Optional
import json
import time
import threading
from pathlib import Path
from authlib.integrations.requests_client import OAuth2Session
from authlib.oauth2.rfc7636 import create_s256_code_challenge


class OidcAuthProvider:
    """OIDC authentication provider with automatic token refresh.

    Uses authlib for OIDC flows (discovery, PKCE, token refresh).
    """

    def __init__(
        self,
        issuer: str,
        client_id: str,
        token_file: Optional[str] = None,
        token: Optional[dict] = None,
    ):
        """Initialize OIDC auth provider.

        Args:
            issuer: OIDC issuer URL (e.g., "https://accounts.google.com")
            client_id: Client ID from identity provider
            token_file: Path to save/load tokens (default: ~/.micromegas/tokens.json)
            token: Pre-loaded token dict (for testing or manual token management)
        """
        self.issuer = issuer
        self.client_id = client_id
        self.token_file = token_file or str(Path.home() / ".micromegas" / "tokens.json")
        self._lock = threading.Lock()  # Thread-safe token refresh

        # Create OAuth2Session with OIDC discovery
        self.client = OAuth2Session(
            client_id=client_id,
            scope="openid email profile",
            token=token,
            token_endpoint_auth_method="none",  # Public client (no client secret)
        )

        # Fetch OIDC metadata via discovery
        self.metadata = self.client.fetch_server_metadata(
            f"{issuer}/.well-known/openid-configuration"
        )

        # Set token if provided
        if token:
            self.client.token = token

    @classmethod
    def login(
        cls,
        issuer: str,
        client_id: str,
        token_file: Optional[str] = None,
        redirect_uri: str = "http://localhost:8080/callback",
    ) -> "OidcAuthProvider":
        """Perform browser-based OIDC login flow.

        Args:
            issuer: OIDC issuer URL
            client_id: Client ID from identity provider
            token_file: Where to save tokens after login
            redirect_uri: Local callback URI for OAuth redirect

        Returns:
            OidcAuthProvider with valid tokens
        """
        # Create temporary session for login
        temp_client = OAuth2Session(
            client_id=client_id,
            scope="openid email profile",
            redirect_uri=redirect_uri,
            token_endpoint_auth_method="none",
        )

        # Fetch OIDC metadata
        metadata = temp_client.fetch_server_metadata(
            f"{issuer}/.well-known/openid-configuration"
        )

        # Perform authorization code flow with PKCE
        token = cls._perform_auth_flow(temp_client, metadata, redirect_uri)

        # Create provider with token
        provider = cls(issuer, client_id, token_file, token=token)

        # Save tokens if file specified
        if token_file:
            provider.save()

        return provider

    @staticmethod
    def _perform_auth_flow(client: OAuth2Session, metadata: dict, redirect_uri: str) -> dict:
        """Perform authorization code flow with PKCE using authlib.

        Args:
            client: Configured OAuth2Session
            metadata: OIDC provider metadata
            redirect_uri: Local callback URI

        Returns:
            Token dict with access_token, id_token, refresh_token, etc.
        """
        import webbrowser
        import http.server
        import socketserver
        from urllib.parse import parse_qs

        # Generate authorization URL with PKCE (authlib handles code_challenge automatically)
        auth_url, state = client.create_authorization_url(
            metadata["authorization_endpoint"],
            code_challenge_method="S256",  # Use PKCE with S256
        )

        # Start local callback server
        auth_code = None
        callback_port = int(redirect_uri.split(':')[-1].split('/')[0])

        class CallbackHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                nonlocal auth_code

                # Parse authorization code from query string
                query = parse_qs(self.path.split('?')[1] if '?' in self.path else '')
                auth_code = query.get('code', [None])[0]

                # Send response to browser
                self.send_response(200)
                self.send_header('Content-type', 'text/html')
                self.end_headers()

                if auth_code:
                    self.wfile.write(b'<html><body><h1>Authentication successful!</h1><p>You can close this window.</p></body></html>')
                else:
                    self.wfile.write(b'<html><body><h1>Authentication failed</h1><p>No authorization code received.</p></body></html>')

            def log_message(self, format, *args):
                pass  # Suppress logging

        # Start callback server
        server = socketserver.TCPServer(("", callback_port), CallbackHandler)
        server_thread = threading.Thread(target=server.handle_request)
        server_thread.daemon = True
        server_thread.start()

        # Open browser for user authentication
        print(f"Opening browser for authentication...")
        webbrowser.open(auth_url)

        # Wait for callback
        server_thread.join(timeout=300)  # 5 minute timeout
        server.server_close()

        if not auth_code:
            raise Exception("Authentication failed - no authorization code received")

        # Exchange authorization code for tokens (authlib handles code_verifier automatically)
        token = client.fetch_token(
            metadata["token_endpoint"],
            authorization_response=f"{redirect_uri}?code={auth_code}&state={state}",
        )

        return token

    def get_token(self) -> str:
        """Get valid ID token, refreshing if necessary.

        This method is called before each query by the FlightSQL client.
        Thread-safe for concurrent queries.

        Returns:
            Valid ID token for Authorization header
        """
        with self._lock:
            if not self.client.token:
                raise Exception("No tokens available. Please call login() first.")

            # Check if token needs refresh (5 min buffer)
            expires_at = self.client.token.get("expires_at", 0)
            if expires_at > time.time() + 300:
                # Token still valid
                return self.client.token["id_token"]

            # Token expired or expiring soon - refresh it
            if self.client.token.get("refresh_token"):
                try:
                    self._refresh_tokens()
                    return self.client.token["id_token"]
                except Exception as e:
                    raise Exception(f"Token refresh failed: {e}. Please re-authenticate.")
            else:
                raise Exception("No refresh token available. Please re-authenticate.")

    def _refresh_tokens(self):
        """Refresh access token using refresh token (authlib handles everything)."""
        # authlib automatically refreshes using refresh_token
        new_token = self.client.fetch_token(
            self.metadata["token_endpoint"],
            grant_type="refresh_token",
            refresh_token=self.client.token["refresh_token"],
        )

        # Update token (authlib updates self.client.token automatically)
        # Save updated tokens to file
        if self.token_file:
            self.save()

    def save(self):
        """Save tokens to file."""
        Path(self.token_file).parent.mkdir(parents=True, exist_ok=True)

        # Save token with metadata
        with open(self.token_file, 'w') as f:
            json.dump({
                "issuer": self.issuer,
                "client_id": self.client_id,
                "token": self.client.token,  # authlib's token dict
            }, f, indent=2)

    @classmethod
    def from_file(cls, token_file: str) -> "OidcAuthProvider":
        """Load tokens from file.

        Args:
            token_file: Path to token file

        Returns:
            OidcAuthProvider with loaded tokens
        """
        with open(token_file) as f:
            data = json.load(f)

        return cls(
            issuer=data["issuer"],
            client_id=data["client_id"],
            token_file=token_file,
            token=data["token"],  # authlib token dict
        )
```

**Benefits of using authlib:**
- **Automatic PKCE:** `create_authorization_url()` handles code_challenge generation
- **Automatic token refresh:** `fetch_token()` with grant_type="refresh_token" does everything
- **Standards-compliant:** Implements all OAuth2/OIDC specs correctly
- **Discovery support:** `fetch_server_metadata()` auto-discovers endpoints
- **Token management:** authlib handles token storage and expiration internally
- **Less code:** ~200 lines saved vs manual implementation
- **Well-tested:** Production-proven library with thousands of users
```

#### 2. Client Usage
```python
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient

# First time login (opens browser)
auth = OidcAuthProvider.login(
    issuer="https://accounts.google.com",
    client_id="your-app-id.apps.googleusercontent.com",
    token_file="~/.micromegas/tokens.json"
)

# Create client with auth provider
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)

# Use client - tokens auto-refresh before each query
df = client.query("SELECT * FROM log_entries", begin, end)

# Later sessions - load from file
auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")
client = FlightSQLClient(uri, auth_provider=auth)
```

### CLI Components

The CLI tools use the existing `connection.connect()` pattern. Update `connection.py` to support OIDC:

```python
# In cli/connection.py

import importlib
import os
from pathlib import Path


def connect():
    """Create FlightSQL client with authentication support.

    Uses MICROMEGAS_PYTHON_MODULE_WRAPPER if set (corporate auth),
    otherwise uses OIDC if configured, or falls back to simple connect().
    """
    # Corporate wrapper takes precedence
    micromegas_module_name = os.environ.get("MICROMEGAS_PYTHON_MODULE_WRAPPER")
    if micromegas_module_name:
        micromegas_module = importlib.import_module(micromegas_module_name)
        return micromegas_module.connect()

    # Try OIDC authentication
    oidc_issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    oidc_client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")

    if oidc_issuer and oidc_client_id:
        import micromegas
        from micromegas.auth import OidcAuthProvider
        from micromegas.flightsql.client import FlightSQLClient

        token_file = os.environ.get(
            "MICROMEGAS_TOKEN_FILE",
            str(Path.home() / ".micromegas" / "tokens.json")
        )

        # Try to load existing tokens
        if Path(token_file).exists():
            try:
                auth = OidcAuthProvider.from_file(token_file)
            except Exception as e:
                # Token file corrupted or refresh failed - re-authenticate
                print(f"Token refresh failed: {e}")
                print("Re-authenticating...")
                auth = OidcAuthProvider.login(
                    issuer=oidc_issuer,
                    client_id=oidc_client_id,
                    token_file=token_file,
                )
        else:
            # First time - login with browser
            print("No saved tokens found. Opening browser for authentication...")
            auth = OidcAuthProvider.login(
                issuer=oidc_issuer,
                client_id=oidc_client_id,
                token_file=token_file,
            )

        uri = os.environ.get("MICROMEGAS_ANALYTICS_URI", "grpc://localhost:50051")
        return FlightSQLClient(uri, auth_provider=auth)

    # Fall back to simple connect (no auth)
    import micromegas
    return micromegas.connect()
```

**Optional: Add logout tool**

```python
# In cli/logout.py

import argparse
import os
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas_logout",
        description="Clear saved OIDC authentication tokens",
    )
    args = parser.parse_args()

    token_file = os.environ.get(
        "MICROMEGAS_TOKEN_FILE",
        str(Path.home() / ".micromegas" / "tokens.json")
    )

    if Path(token_file).exists():
        Path(token_file).unlink()
        print(f"Tokens cleared from {token_file}")
    else:
        print("No saved tokens found")


if __name__ == "__main__":
    main()
```

**CLI User Experience:**

```bash
# Set OIDC config
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="your-app-id.apps.googleusercontent.com"

# First time - opens browser for authentication
$ python -m micromegas.cli.query_processes
No saved tokens found. Opening browser for authentication...
# Browser opens, user authenticates
# Tokens saved to ~/.micromegas/tokens.json
# Query executes

# Subsequent calls - uses saved tokens (no browser)
$ python -m micromegas.cli.query_process_log <process-id>
# Query executes immediately (tokens auto-refresh if needed)

# Clear saved tokens (if logout.py is added)
$ python -m micromegas.cli.logout
Tokens cleared from ~/.micromegas/tokens.json
```

**No changes needed to existing CLI tools** - they already use `connection.connect()`, so OIDC support works automatically.

## Implementation Plan

### Overall Progress

- **Phase 1 (Server-Side OIDC):** ~90% complete ✅
  - Auth crate: ✅ Complete
  - Integration: ⏳ In progress
- **Phase 2 (Python Client):** Not started
- **Phase 3 (CLI):** Not started
- **Phase 4 (Documentation):** Not started

### Current Status (2025-01-24)

**✅ Completed (Auth Crate - needs refactoring):**
- ✅ **Separate `micromegas-auth` crate created** (`rust/auth/`)
- ✅ `AuthProvider` trait with `AuthContext` struct
- ✅ `ApiKeyAuthProvider` with KeyRing parsing
- ✅ `OidcAuthProvider` with token validation and JWKS caching
- ⚠️ **JWKS fetching** (currently manual reqwest - needs refactoring to openidconnect)
- ⚠️ **JWT validation** (currently jsonwebtoken - needs refactoring to openidconnect::IdTokenVerifier)
- ✅ Test utilities for generating test tokens
- ✅ **Tests moved to separate files** (`tests/` directory)
- ✅ **Code style improvements:**
  - ✅ `use` statements moved to module level
  - ✅ `Claims` struct moved to module level with field documentation
  - ✅ Renamed `check_admin` to `is_admin`
  - ✅ Admin users hidden in Debug output for security
- ✅ All tests passing (10 tests + 2 doc tests)

**⏳ In Progress (Integration):**
- ⏳ Wire up AuthProvider in tonic_auth_interceptor.rs
- ⏳ Add flight-sql-srv configuration and initialization
- ⏳ Refactor to use openidconnect for discovery AND validation (remove jsonwebtoken dependency)
- ⏳ Add integration tests with wiremock
- ⏳ Test end-to-end with real OIDC provider (Google/Azure AD)

**📦 Auth Crate Structure:**
```
rust/auth/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Public API with re-exports
│   ├── types.rs        # AuthContext, AuthProvider trait, AuthType
│   ├── api_key.rs      # ApiKeyAuthProvider + KeyRing
│   ├── oidc.rs         # OidcAuthProvider (JWKS caching included)
│   └── test_utils.rs   # Test token generation utilities
└── tests/
    ├── api_key_tests.rs  # API key unit tests
    └── oidc_tests.rs     # OIDC unit tests
```

**✨ Key Improvements:**
- JWKS cache uses `CoreProviderMetadata::discover_async()` (proper OIDC discovery)
- HTTP client configured with `redirect(Policy::none())` for SSRF protection
- Clean separation: no dependency on micromegas-tracing
- Faster builds: auth crate compiles independently
- All dependencies properly scoped (no leaking to public crate)
- Tests follow project pattern (unit tests in `tests/`, not inline)
- Claims struct properly documented with JWT field descriptions

**🔄 Next Steps:**
1. Update `tonic_auth_interceptor.rs` to use `AuthProvider` trait
2. Wire up `OidcAuthProvider` in `flight_sql_srv.rs`
3. Add integration tests with wiremock
4. Test end-to-end with real OIDC provider (Google/Azure AD)

### Phase 1: Server-Side OIDC Validation (Rust)
**Goal:** flight-sql-srv can validate OIDC ID tokens

**Status:** ~90% complete (auth crate done, needs integration with flight-sql-srv)

**Completed:**
1. ✅ Created `micromegas-auth` crate (instead of `flight-sql-srv/src/auth/`)
   - ✅ `types.rs` - AuthProvider trait, AuthContext, AuthType
   - ✅ `api_key.rs` - ApiKeyAuthProvider with KeyRing
   - ✅ `oidc.rs` - OidcAuthProvider + JwksCache (combined)
   - ✅ `test_utils.rs` - Test token generation

2. ✅ OIDC implementation:
   - ✅ `OidcAuthProvider` struct with multi-issuer support
   - ✅ `AuthProvider` trait implementation
   - ✅ JWKS caching with TTL (using moka)
   - ✅ Token validation cache
   - ✅ Proper OIDC discovery using `CoreProviderMetadata::discover_async()`
   - ✅ SSRF protection (HTTP client with `redirect(Policy::none())`)

3. ✅ Configuration:
   - ✅ `OidcConfig::from_env()` reads `MICROMEGAS_OIDC_CONFIG`
   - ✅ JSON parsing into `OidcConfig` struct
   - ✅ Admin users support via `MICROMEGAS_ADMINS`

4. ✅ Unit tests:
   - ✅ API key validation
   - ✅ OIDC config parsing
   - ✅ Token generation and verification
   - ✅ Expired token handling
   - ✅ All 10 tests + 2 doc tests passing

**Remaining:**
5. ⏳ Update `tonic_auth_interceptor.rs`:
   - Use `AuthProvider` trait instead of direct KeyRing
   - Support both API key and OIDC auth modes

6. ⏳ Update `flight_sql_srv.rs`:
   - Initialize `OidcAuthProvider` or `ApiKeyAuthProvider` based on mode
   - Pass provider to auth interceptor

7. ⏳ Add integration tests:
   - wiremock-based mock OIDC provider
   - End-to-end token validation
   - Multi-issuer scenarios

**Acceptance Criteria:**
- ⏳ Server validates Google OIDC tokens (needs wiring)
- ⏳ Server validates Azure AD OIDC tokens (needs wiring)
- ✅ JWKS cache reduces external calls
- ✅ Token cache reduces validation overhead
- ⏳ Integration tests pass with mock OIDC provider

### Phase 2: Python Client OIDC Support
**Goal:** Python client can authenticate users and refresh tokens

1. Create `python/micromegas/micromegas/auth/__init__.py`:
   - Export `OidcAuthProvider`

2. Create `python/micromegas/micromegas/auth/oidc.py`:
   - Implement `OidcAuthProvider` class
   - Browser-based login flow
   - PKCE implementation
   - Token refresh logic
   - Token file persistence

3. Update `python/micromegas/micromegas/flightsql/client.py`:
   - Add `DynamicAuthMiddleware` class
   - Add `DynamicAuthMiddlewareFactory` class
   - Update `FlightSQLClient.__init__()` to accept `auth_provider`

4. Add dependencies to `python/micromegas/pyproject.toml`:
   ```toml
   authlib = "^1.3.0"
   ```

5. Add unit tests:
   - Token refresh logic
   - Thread-safe concurrent refresh
   - File persistence

6. Add integration tests:
   - Full auth flow with mock OIDC provider
   - Token refresh scenarios
   - Concurrent query handling

7. Update documentation:
   - Add OIDC authentication guide
   - Code examples

**Acceptance Criteria:**
- ✅ Browser-based login flow works
- ✅ Tokens saved to ~/.micromegas/tokens.json
- ✅ Tokens auto-refresh before expiration
- ✅ Concurrent queries handle refresh safely
- ✅ Integration tests pass

### Phase 3: CLI OIDC Support
**Goal:** CLI tools support OIDC authentication with token persistence

1. Update `cli/connection.py` to support OIDC:
   - Check environment variables:
     - `MICROMEGAS_OIDC_ISSUER`
     - `MICROMEGAS_OIDC_CLIENT_ID`
     - `MICROMEGAS_TOKEN_FILE` (optional, default: ~/.micromegas/tokens.json)
   - Maintain backward compatibility with `MICROMEGAS_PYTHON_MODULE_WRAPPER`
   - Implement token persistence flow:
     - Check for existing token file
     - Load and use saved tokens if available
     - Browser login only on first use or token expiration
     - Auto-refresh using saved tokens

2. (Optional) Add `cli/logout.py` to clear saved tokens

3. Add examples to documentation

4. Test with existing CLI tools (query_processes, query_process_log, etc.)

**Acceptance Criteria:**
- ✅ First invocation opens browser and saves tokens
- ✅ Subsequent invocations use saved tokens (no browser)
- ✅ Tokens auto-refresh transparently
- ✅ All existing CLI tools work without modification
- ✅ Backward compatible with MICROMEGAS_PYTHON_MODULE_WRAPPER
- ✅ Shares same token file format as Python client

### Phase 4: Documentation and Examples
**Goal:** Users can easily set up OIDC authentication

1. Write admin guide:
   - How to register app with Google/Azure AD/Okta
   - How to configure MICROMEGAS_OIDC_CONFIG
   - Security best practices

2. Write user guide:
   - How to use OidcAuthProvider in Python
   - How to use OIDC with CLI
   - Troubleshooting common issues

3. Add examples:
   - Google authentication example
   - Azure AD authentication example
   - Jupyter notebook example

**Deliverables:**
- ✅ Admin setup guide
- ✅ User authentication guide
- ✅ Working examples for major providers

## Configuration Reference

### Server Configuration

```bash
# Enable OIDC authentication mode
export MICROMEGAS_AUTH_MODE=oidc

# OIDC provider configuration
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

# Admin users (can manage service accounts)
export MICROMEGAS_ADMINS='["alice@example.com", "bob@example.com"]'
```

### Python Client Configuration

```python
# Option 1: Login and save tokens
from micromegas.auth import OidcAuthProvider

auth = OidcAuthProvider.login(
    issuer="https://accounts.google.com",
    client_id="your-app-id.apps.googleusercontent.com",
    token_file="~/.micromegas/tokens.json"
)

# Option 2: Load saved tokens
auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")

# Create client
from micromegas.flightsql.client import FlightSQLClient
client = FlightSQLClient(
    "grpc+tls://analytics.example.com:50051",
    auth_provider=auth
)
```

### CLI Configuration

```bash
# Set environment variables
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="your-app-id.apps.googleusercontent.com"

# First time - opens browser, saves tokens to ~/.micromegas/tokens.json
python -m micromegas.cli.query_processes

# Subsequent calls - uses saved tokens (no browser)
python -m micromegas.cli.query_process_log <process-id>

# Use custom token file location
export MICROMEGAS_TOKEN_FILE="~/.config/micromegas/tokens.json"
python -m micromegas.cli.query_processes

# Clear saved tokens (if logout.py is added)
python -m micromegas.cli.logout
```

## Testing Strategy

### Three-Layer Testing Approach

Our testing strategy uses three complementary layers: fast unit tests for logic validation, integration tests with mock OIDC endpoints, and manual testing with real providers for final validation.

### Layer 1: Unit Tests (Fast, No Network)

**Approach:** Use `jsonwebtoken` crate to create test tokens with generated RSA key pairs.

**Server (Rust):**

```rust
// Test utilities for generating tokens
use jsonwebtoken::{encode, decode, Header, Algorithm, EncodingKey, DecodingKey};
use rsa::RsaPrivateKey;
use rsa::pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey};

fn generate_test_keypair() -> (EncodingKey, DecodingKey) {
    let private_key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public_key = private_key.to_public_key();

    let private_pem = private_key.to_pkcs1_pem(Default::default()).unwrap();
    let public_pem = public_key.to_pkcs1_pem(Default::default()).unwrap();

    (
        EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
        DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap()
    )
}

fn create_test_id_token(claims: MyClaims, encoding_key: &EncodingKey) -> String {
    encode(&Header::new(Algorithm::RS256), &claims, encoding_key).unwrap()
}
```

**What to test:**
- ✅ Valid token validation
- ✅ Claim extraction (email, sub, issuer)
- ✅ Expired token rejection
- ✅ Wrong audience rejection
- ✅ Wrong issuer rejection
- ✅ Invalid signature rejection
- ✅ Token cache hit/miss behavior
- ✅ JWKS cache behavior

**Dependencies:**
```toml
[dev-dependencies]
jsonwebtoken = "9"
rsa = "0.9"  # For generating test RSA keys
rand = "0.8"
```

**Benefits:**
- Fast (no network calls)
- Deterministic results
- Easy to test edge cases
- No external dependencies

**Client (Python):**
- Token refresh logic with mocked time
- Expiration detection (5 min buffer)
- Thread-safe concurrent refresh
- File save/load operations
- Browser flow components (mocked HTTP server)

**Python testing dependencies:**
```toml
[tool.poetry.dev-dependencies]
pytest = "^7.0"
pytest-asyncio = "^0.21"
responses = "^0.23"  # Mock HTTP responses
freezegun = "^1.2"   # Mock time for expiration tests
```

### Layer 2: Integration Tests (Mock OIDC Endpoints)

**Approach:** Use `wiremock` to mock OIDC provider endpoints (discovery, JWKS).

**Server (Rust):**

```rust
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path};

#[tokio::test]
async fn test_oidc_provider_with_mock_server() {
    // Start mock OIDC server
    let mock_server = MockServer::start().await;

    // Mock discovery endpoint
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": mock_server.uri(),
            "jwks_uri": format!("{}/jwks", mock_server.uri()),
            "authorization_endpoint": format!("{}/authorize", mock_server.uri()),
            "token_endpoint": format!("{}/token", mock_server.uri()),
        })))
        .mount(&mock_server)
        .await;

    // Mock JWKS endpoint with test public keys
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
        .mount(&mock_server)
        .await;

    // Initialize provider with mock server URL
    let config = OidcConfig {
        issuers: vec![OidcIssuer {
            issuer: mock_server.uri(),
            audience: "test-audience".to_string(),
        }],
        ..Default::default()
    };

    let provider = OidcAuthProvider::new(config).await.unwrap();

    // Create token signed with corresponding private key
    let token = create_test_token_for_issuer(&mock_server.uri());

    // Validate token
    let auth_ctx = provider.validate_token(&token).await.unwrap();
    assert_eq!(auth_ctx.email, Some("test@example.com".to_string()));
}
```

**What to test:**
- ✅ OIDC discovery flow (/.well-known/openid-configuration)
- ✅ JWKS fetching and parsing
- ✅ JWKS cache TTL refresh
- ✅ End-to-end token validation
- ✅ Multi-issuer support
- ✅ Network error handling (500 errors, timeouts)
- ✅ Malformed JWKS response handling

**Dependencies:**
```toml
[dev-dependencies]
wiremock = "0.6"
```

**Benefits:**
- Tests full OIDC discovery and validation flow
- No external services required
- Fast and reliable
- Full control over mock responses
- Can simulate network failures

**Client (Python):**
- Use `oidc-server-mock` Docker container for full auth flow testing
- Start/stop container in test fixtures
- Test full authorization code + PKCE flow
- Test token refresh with real OAuth flow
- Test concurrent query handling
- Test error scenarios (network failures, expired refresh tokens)

**Python approach:**
```python
import pytest
import docker
import requests

@pytest.fixture(scope="module")
def oidc_mock_server():
    """Start oidc-server-mock in Docker for testing."""
    client = docker.from_env()
    container = client.containers.run(
        "ghcr.io/soluto/oidc-server-mock:latest",
        detach=True,
        ports={'80/tcp': 8080},
        environment={'ASPNETCORE_ENVIRONMENT': 'Development'}
    )

    # Wait for server to be ready
    for _ in range(30):
        try:
            resp = requests.get('http://localhost:8080/.well-known/openid-configuration')
            if resp.status_code == 200:
                break
        except:
            time.sleep(0.1)

    yield 'http://localhost:8080'

    container.stop()
    container.remove()

def test_full_auth_flow(oidc_mock_server):
    """Test complete OIDC login flow."""
    auth = OidcAuthProvider.login(
        issuer=oidc_mock_server,
        client_id='test-client',
        # ... test browser flow with mocked interactions
    )
    assert auth.get_token() is not None
```

### Layer 3: Manual/E2E Testing (Real Providers)

**Approach:** Use actual OIDC providers for final validation.

**Option A: Google OAuth (Recommended for development)**
1. Create OAuth2 credentials at https://console.cloud.google.com/
2. Configure redirect URI: `http://localhost:8080/callback`
3. Set environment variable:
   ```bash
   export MICROMEGAS_OIDC_CONFIG='{
     "issuers": [{
       "issuer": "https://accounts.google.com",
       "audience": "YOUR-CLIENT-ID.apps.googleusercontent.com"
     }]
   }'
   ```
4. Run server and test with real Google tokens

**Option B: Keycloak (Docker, for controlled testing)**
```bash
# Run Keycloak
docker run -p 8080:8080 \
  -e KEYCLOAK_ADMIN=admin \
  -e KEYCLOAK_ADMIN_PASSWORD=admin \
  quay.io/keycloak/keycloak:latest start-dev

# Access admin console: http://localhost:8080
# Create test realm, client, and users
```

**Option C: oidc-server-mock (Docker, lightweight)**
```bash
docker run -d -p 8080:80 \
  -e ASPNETCORE_ENVIRONMENT=Development \
  ghcr.io/soluto/oidc-server-mock:latest
```

**What to test:**
1. Google OAuth setup and authentication
2. Azure AD OAuth setup and authentication (if available)
3. End-to-end server token validation with real tokens
4. Python client browser-based login flow
5. CLI browser-based authentication
6. Token refresh after 1 hour (wait or mock time)
7. Concurrent queries from multiple threads/processes
8. Performance with real JWKS fetching

**Testing checklist:**
- ✅ Server validates real Google ID tokens
- ✅ Server validates real Azure AD tokens (if configured)
- ✅ Python client login flow opens browser correctly
- ✅ Python client saves tokens to file
- ✅ Python client auto-refreshes before expiration
- ✅ CLI login flow works (browser opens only when needed)
- ✅ CLI shares token file with Python client
- ✅ Multiple concurrent queries don't cause race conditions

### Test Organization

**File structure:**
```
rust/public/src/servers/auth/
├── mod.rs
├── oidc_provider.rs
├── jwks_cache.rs
├── test_utils.rs          # Test token generation utilities
└── tests/
    ├── unit_tests.rs      # Fast unit tests
    └── integration_tests.rs  # wiremock-based tests

python/micromegas/tests/auth/
├── test_oidc_unit.py      # Unit tests with mocked HTTP
├── test_oidc_integration.py  # Docker-based integration tests
└── conftest.py            # Shared fixtures
```

### CI/CD Integration

**Rust CI:**
```bash
# Run in CI pipeline
cargo test --workspace           # Runs unit tests
cargo test --workspace --ignored # Runs integration tests (wiremock)
```

**Python CI:**
```bash
# Unit tests (fast, no Docker)
poetry run pytest tests/auth/test_oidc_unit.py

# Integration tests (requires Docker)
poetry run pytest tests/auth/test_oidc_integration.py
```

**Note:** Integration tests requiring Docker should be marked with `#[ignore]` or `@pytest.mark.integration` to allow fast CI runs that skip them.

### Test Development Workflow

1. **Write unit test** for specific behavior (e.g., expired token rejection)
2. **Implement minimal code** to make test pass
3. **Add integration test** to verify behavior with mock OIDC endpoints
4. **Run manual test** with Google OAuth for final validation
5. **Repeat** for next feature

This TDD approach ensures each component is well-tested at multiple levels before moving to the next feature.

## Security Considerations

1. **Token Storage:**
   - Tokens saved with 0600 permissions
   - Never log tokens
   - Clear sensitive data on errors

2. **JWKS Validation:**
   - Always verify signature using provider's public keys
   - Validate issuer matches expected value
   - Validate audience matches configured value
   - Check expiration with clock skew tolerance (60s)

3. **Token Refresh:**
   - Use refresh token only (never store passwords)
   - Handle refresh token rotation
   - Re-authenticate if refresh token expired

4. **Network Security:**
   - Always use HTTPS for token endpoints
   - Verify TLS certificates
   - Use PKCE for authorization code flow

5. **Cache Security:**
   - Cache contains validated tokens only
   - Automatic TTL expiration
   - No tokens in logs or error messages

## Success Metrics

1. ✅ OIDC login flow completes in <5 seconds (including browser)
2. ✅ Token validation adds <10ms latency per request
3. ✅ Token refresh adds <1s latency when needed
4. ✅ Cache hit rate >95% for repeated requests
5. ✅ Support Google, Azure AD, and Okta providers
6. ✅ Python client auto-refresh works for weeks without re-auth
7. ✅ CLI uses saved tokens - browser only opens on first use or expiration
8. ✅ Zero token validation failures due to race conditions
9. ✅ Complete documentation and examples

## Dependencies

### Rust Crates

**Production dependencies:**
```toml
# OIDC and JWT - use openidconnect for everything
openidconnect = "4.0"   # OIDC client library (discovery, metadata, JWT validation)

# Caching
moka = { version = "0.12", features = ["future"] }  # High-performance async caching
```

**Test dependencies:**
```toml
[dev-dependencies]
wiremock = "0.6"        # Mock HTTP server for integration tests
```

**Why these choices:**
- `openidconnect` - Standards-compliant OIDC implementation (discovery, JWKS, JWT validation)
- `moka` - Best-in-class async caching with TTL support
- `wiremock` - Industry standard for HTTP mocking in Rust

**Removed dependencies (after refactoring):**
- ~~`jsonwebtoken`~~ - Replaced by openidconnect's IdTokenVerifier
- ~~`rsa`~~ - Not needed, openidconnect handles JWKS conversion
- ~~`base64`~~ - Not needed when using openidconnect
- ~~`reqwest`~~ - openidconnect has built-in HTTP client

**Benefits of using openidconnect:**
- Fewer dependencies (smaller attack surface)
- Standards-compliant implementation
- Less custom code to maintain
- Automatic security updates from openidconnect crate

### Python Packages
```toml
authlib = "^1.3.0"     # OAuth2/OIDC client library (includes JWT, PKCE, discovery, refresh)
```

**Note:** `authlib` includes everything needed for OIDC (no need for separate `pyjwt` or `requests-oauthlib`)

## Architecture Decisions & Trade-offs

### 1. JWT Validation: Use openidconnect

**Decision:** Use `openidconnect` crate for both OIDC discovery and JWT validation.

**Rationale:**
- **Standards compliance:** openidconnect implements OIDC spec correctly
- **Security:** Built-in JWT verification with proper security checks (nonce, signature, claims)
- **Less custom code:** No manual JWKS conversion or claim validation needed
- **Smaller attack surface:** Avoid custom crypto code
- **Well-maintained:** Actively developed with security updates
- **Proper error handling:** Detailed error types for validation failures

**Benefits:**
- Automatic JWKS conversion from discovery endpoint
- Proper nonce validation for replay prevention
- Built-in claim validation (iss, aud, exp, nbf)
- Standards-compliant implementation reduces security risks

**Implementation Status:**
- ⚠️ Current implementation uses jsonwebtoken (needs refactoring)
- ⏳ Refactor to use `openidconnect::IdTokenVerifier`
- ⏳ Remove jsonwebtoken and rsa dependencies

### 2. Implementation Approach: Use openidconnect Throughout

**Goal:** Maximize use of openidconnect crate, minimize custom implementation.

**Use openidconnect for:**
- ✅ OIDC discovery (`CoreProviderMetadata::discover_async()`)
- ✅ JWT validation (`IdTokenVerifier`)
- ✅ JWKS handling (automatic conversion from discovery)
- ✅ Claim extraction (standard claims via `IdTokenClaims`)
- ✅ Error handling (use openidconnect's error types)

**Current implementation gaps:**
- ⚠️ Uses manual `reqwest::get()` for discovery
- ⚠️ Uses `jsonwebtoken` for validation
- ⚠️ Manual JWKS to DecodingKey conversion

**Action items:**
1. Refactor OIDC discovery to use `CoreProviderMetadata::discover_async()`
2. Replace jsonwebtoken validation with `IdTokenVerifier`
3. Remove manual JWKS conversion code
4. Remove jsonwebtoken and rsa dependencies

### 3. Caching Strategy: moka

**Decision:** Use `moka` for both JWKS cache and token validation cache.

**Rationale:**
- Best-in-class async caching for Rust
- TTL support prevents stale data
- `try_get_with()` prevents thundering herd (multiple threads fetching same key)
- Lock-free implementation for high performance
- Simple API

**Alternatives considered:**
- `cached` crate - less mature, fewer features
- Manual `Arc<RwLock<HashMap>>` - harder to implement correctly, no TTL
- `mini-moka` - smaller but missing async support

### 4. Token Validation Flow: Multi-issuer iteration

**Decision:** Iterate through all configured issuers and try each JWKS key until validation succeeds.

**Rationale:**
- Simple implementation
- Supports multiple identity providers
- No need to parse token payload before verification

**Trade-off:**
- Less efficient (tries multiple issuers on failure)
- Could be optimized by decoding payload first to get `iss` claim

**Future optimization:** Decode JWT payload without verification to extract issuer, then only try that issuer's JWKS.

### 5. Auth Crate Location

**Decision:** Created separate `micromegas-auth` crate at `rust/auth/`.

**Rationale:**
- Better modularity and separation of concerns
- Faster build times (auth code compiles independently)
- Easier testing in isolation
- Cleaner dependency graph
- Can be reused by other services

**Benefits realized:**
- ✅ Auth crate compiles independently
- ✅ No dependency on micromegas-tracing
- ✅ All dependencies properly scoped
- ✅ Tests in separate directory following project pattern

**Status:** ✅ Complete - auth crate created and fully functional

## References

### Rust Crates
- [openidconnect crate docs](https://docs.rs/openidconnect/latest/openidconnect/) - OIDC client library for Rust
- [moka crate docs](https://docs.rs/moka/latest/moka/) - High-performance concurrent caching

### Python Libraries
- [authlib documentation](https://docs.authlib.org/en/latest/) - OAuth2/OIDC client library for Python
- [authlib GitHub](https://github.com/authlib/authlib) - Source code and examples

### Standards
- [OIDC Core Spec](https://openid.net/specs/openid-connect-core-1_0.html) - OpenID Connect Core 1.0 specification
- [OAuth 2.0 RFC 6749](https://datatracker.ietf.org/doc/html/rfc6749) - OAuth 2.0 Authorization Framework
- [PKCE RFC 7636](https://datatracker.ietf.org/doc/html/rfc7636) - Proof Key for Code Exchange
- [JWT RFC 7519](https://datatracker.ietf.org/doc/html/rfc7519) - JSON Web Token specification

### Provider Documentation
- [Google OAuth 2.0](https://developers.google.com/identity/protocols/oauth2) - Google Cloud OAuth setup
- [Azure AD OAuth](https://learn.microsoft.com/en-us/azure/active-directory/develop/v2-oauth2-auth-code-flow) - Microsoft Azure AD setup
- [Okta OIDC](https://developer.okta.com/docs/guides/implement-grant-type/authcode/main/) - Okta authentication guide
