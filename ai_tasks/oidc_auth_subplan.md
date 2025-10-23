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

#### 2. JWKS Cache
```rust
use moka::future::Cache;
use std::time::Duration;

struct JwksCache {
    issuer_url: IssuerUrl,
    cache: Cache<String, CoreJsonWebKeySet>,
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

    async fn get(&self) -> Result<CoreJsonWebKeySet> {
        // Use moka's get_or_try_insert to handle cache miss atomically
        // This prevents duplicate fetches when multiple threads miss cache simultaneously
        let jwks = self.cache
            .try_get_with("jwks".to_string(), async {
                self.fetch_jwks().await
            })
            .await
            .map_err(|e| anyhow!("Failed to fetch JWKS: {}", e))?;

        Ok(jwks)
    }

    async fn fetch_jwks(&self) -> Result<CoreJsonWebKeySet> {
        // Fetch from {issuer}/.well-known/openid-configuration
        let metadata = CoreProviderMetadata::discover_async(
            self.issuer_url.clone(),
            async_http_client,
        ).await?;

        // Get JWKS from jwks_uri
        let jwks = metadata.jwks().await?;
        Ok(jwks)
    }
}
```

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

### Phase 1: Server-Side OIDC Validation (Rust)
**Goal:** flight-sql-srv can validate OIDC ID tokens

1. Add dependencies to `rust/Cargo.toml`:
   ```toml
   openidconnect = "4.0"
   moka = "0.12"
   ```

2. Create `rust/flight-sql-srv/src/auth/oidc_provider.rs`:
   - Implement `OidcAuthProvider` struct
   - Implement `AuthProvider` trait
   - JWKS caching with TTL
   - Token validation cache

3. Create `rust/flight-sql-srv/src/auth/jwks_cache.rs`:
   - JWKS fetching and caching
   - Background refresh on TTL expiration

4. Update `rust/flight-sql-srv/src/auth/mod.rs`:
   - Export `OidcAuthProvider`
   - Add OIDC config parsing

5. Update `rust/flight-sql-srv/src/flight_sql_srv.rs`:
   - Initialize `OidcAuthProvider` when auth_mode=oidc
   - Pass to auth interceptor

6. Add configuration parsing:
   - Read `MICROMEGAS_OIDC_CONFIG` env var
   - Parse JSON into `OidcConfig` struct

7. Add unit tests:
   - Token validation with mock JWKS
   - Cache hit/miss scenarios
   - Expiration handling

8. Add integration tests:
   - Mock OIDC provider
   - End-to-end token validation

**Acceptance Criteria:**
- ✅ Server validates Google OIDC tokens
- ✅ Server validates Azure AD OIDC tokens
- ✅ JWKS cache reduces external calls
- ✅ Token cache reduces validation overhead
- ✅ Tests pass with mock OIDC provider

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

### Unit Tests

**Server (Rust):**
- Token validation with mock JWKS
- Claim extraction and validation
- Cache hit/miss behavior
- Expired token handling
- Invalid signature rejection
- Wrong audience/issuer rejection

**Client (Python):**
- Token refresh logic
- Expiration detection (5 min buffer)
- Thread-safe concurrent refresh
- File save/load
- Browser flow components (mocked)

### Integration Tests

**Server:**
- Mock OIDC provider (using openidconnect test utilities)
- End-to-end token validation
- Multi-issuer support
- JWKS refresh on expiration

**Client:**
- Full auth flow with mock provider
- Token refresh scenarios
- Concurrent query handling
- Error handling (network failures, expired refresh tokens)

### Manual Testing

1. Google OAuth setup and authentication
2. Azure AD OAuth setup and authentication
3. Concurrent queries from multiple threads
4. Token refresh after 1 hour
5. Browser-based CLI authentication

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
```toml
openidconnect = "4.0"   # OIDC client library
moka = "0.12"           # High-performance caching
```

### Python Packages
```toml
authlib = "^1.3.0"     # OAuth2/OIDC client library (includes JWT, PKCE, discovery, refresh)
```

Note: `authlib` includes everything needed for OIDC (no need for separate `pyjwt` or `requests-oauthlib`)

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
