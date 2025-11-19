# HTTP Gateway Authentication and Header Forwarding Plan

**GitHub Issue**: TBD

## Overview
Enhance the HTTP gateway to support OIDC authentication forwarding and configurable HTTP header forwarding to backend services. This will enable the gateway to act as a transparent proxy for authentication tokens and support proprietary authentication schemes through custom header forwarding.

## Current State

### Gateway Implementation
- **Location**: `rust/public/src/servers/http_gateway.rs` (87 lines)
- **Binary**: `rust/http-gateway/src/http_gateway_srv.rs`
- **Current endpoint**: `POST /gateway/query`
- **Current header handling**: Only forwards `Authorization` header to FlightSQL backend
- **No authentication middleware**: Gateway currently operates without auth validation

### Existing Authentication Infrastructure
- **Auth crate**: `rust/auth/` with comprehensive OIDC and API key support
- **OIDC implementation**: Full JWT validation with JWKS caching
- **Axum middleware**: `rust/auth/src/axum.rs` ready for integration
- **Recent work**: Analytics web app OIDC integration (commit 71ce310ce)

### FlightSQL Query Origin Logging
- **Location**: `rust/public/src/servers/flight_sql_service_impl.rs:217-234`
- **Current tracking**: User ID, email, client type, query details
- **Standard headers**:
  - `x-user-id` - User identifier (defaults to "unknown")
  - `x-user-email` - User email (defaults to "unknown")
  - `x-client-type` - Client application (python, web, grafana, etc.)
  - `x-request-id` - Request correlation ID (optional)
- **Log format example**:
  ```
  INFO execute_query range=None sql="SELECT * FROM processes" limit=Some("1000") user=alice email=alice@example.com client=web
  ```
- **Current client types**: `python`, `web`, `grafana`
- **Gateway requirement**: Append `+gateway` to original client type (e.g., `web+gateway`) to preserve full client chain

## Requirements

### 1. Authentication Header Forwarding (No Validation)
- Forward `Authorization` header transparently to FlightSQL backend
- **No token validation in gateway** - FlightSQL service handles all auth
- Gateway acts as transparent HTTP-to-gRPC proxy
- FlightSQL returns 401 if token is invalid (gateway passes through error)
- Simpler design: gateway has no auth dependencies or configuration

### 2. General HTTP Header Forwarding
- Configure allowlist of HTTP headers to forward to backend
- Support standard headers (Authorization, User-Agent, X-Request-ID, etc.)
- Support custom/proprietary headers (X-Custom-Auth, X-Tenant-ID, etc.)
- Prevent forwarding of sensitive headers (Cookie, Set-Cookie by default)
- Allow wildcard patterns (e.g., `X-Custom-*`)

### 3. Query Origin Tracking and Augmentation
- Augment `x-client-type` by appending `+gateway` to preserve client chain
  - If client provides `x-client-type: web`, gateway forwards `x-client-type: web+gateway`
  - If client doesn't provide type, gateway sends `x-client-type: unknown+gateway`
- Set `x-client-ip` header from actual connection (from socket or X-Forwarded-For)
  - **SECURITY**: Always use real connection IP, ignore any `x-client-ip` from client
  - Prevents IP spoofing in audit logs
- Support request correlation with `x-request-id` header (generate if not present)
- **Forward user attribution headers from client** (if provided)
  - Gateway forwards `x-user-id`, `x-user-email` headers if in allowlist
  - **FlightSQL MUST validate these headers** against Authorization token:
    - **OIDC user tokens**: REJECT if `x-user-id`/`x-user-email` don't match token subject/email
    - **API keys/client credentials**: ALLOW service accounts to specify user on behalf of
    - This prevents user impersonation while allowing service account delegation
- Ensure full transparency in FlightSQL query logs with complete origin chain

### 4. Configuration
- Configuration file or env var for header allowlist
- Backward compatibility: existing behavior preserved
- Clear logging of header forwarding and origin tracking

## Technical Design

### Project Structure Updates
```
rust/
├── http-gateway/
│   ├── Cargo.toml (add uuid dependency for request IDs)
│   └── src/
│       ├── http_gateway_srv.rs (add ConnectInfo layer for client IP)
│       └── config.rs (new: header forwarding config)
└── public/src/servers/
    └── http_gateway.rs (update to forward headers and track origin)
```

### Key Components

#### 1. Gateway Server Setup (No Auth Middleware)
**File**: `rust/http-gateway/src/http_gateway_srv.rs`

Simplified server without authentication middleware:
```rust
use axum::extract::connect_info::ConnectInfo;
use std::net::SocketAddr;

#[micromegas_main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    // Load header forwarding configuration
    let header_config = Arc::new(HeaderForwardingConfig::from_env()?);

    let app = servers::http_gateway::register_routes(Router::new())
        .layer(Extension(header_config))
        // Add ConnectInfo layer to get client socket address
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = tokio::net::TcpListener::bind(args.listen_endpoint_http).await?;
    info!("Server running on {}", args.listen_endpoint_http);
    axum::serve(listener, app).await?;
    Ok(())
}
```

**Key differences from before**:
- No `micromegas-auth` dependency
- No authentication middleware
- Only adds ConnectInfo layer for client IP extraction
- Simpler configuration (just header forwarding rules)

#### 2. Header Forwarding Configuration
**File**: `rust/http-gateway/src/config.rs` (new)

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct HeaderForwardingConfig {
    /// Exact header names to forward (case-insensitive)
    pub allowed_headers: Vec<String>,

    /// Header prefixes to forward (e.g., "X-Custom-")
    pub allowed_prefixes: Vec<String>,

    /// Headers to explicitly block (overrides allows)
    pub blocked_headers: Vec<String>,
}

impl Default for HeaderForwardingConfig {
    fn default() -> Self {
        Self {
            // Default safe headers to forward
            allowed_headers: vec![
                "Authorization".to_string(),      // FlightSQL validates auth
                "X-Request-ID".to_string(),
                "X-Correlation-ID".to_string(),
                "User-Agent".to_string(),
                "X-Client-Type".to_string(),      // Augmented by gateway
                "X-User-ID".to_string(),          // Forwarded to FlightSQL
                "X-User-Email".to_string(),       // Forwarded to FlightSQL
                "X-User-Name".to_string(),        // Forwarded to FlightSQL
            ],
            allowed_prefixes: vec![],
            blocked_headers: vec![
                "Cookie".to_string(),
                "Set-Cookie".to_string(),
                // SECURITY: Gateway always sets this from actual connection
                "X-Client-IP".to_string(),
            ],
        }
    }
}

impl HeaderForwardingConfig {
    pub fn from_env() -> Result<Self> {
        if let Ok(config_json) = std::env::var("MICROMEGAS_GATEWAY_HEADERS") {
            serde_json::from_str(&config_json)
                .context("Failed to parse MICROMEGAS_GATEWAY_HEADERS")
        } else {
            Ok(Self::default())
        }
    }

    pub fn should_forward(&self, header_name: &str) -> bool {
        let name_lower = header_name.to_lowercase();

        // Check blocked list first
        if self.blocked_headers.iter().any(|h| h.to_lowercase() == name_lower) {
            return false;
        }

        // Check exact matches
        if self.allowed_headers.iter().any(|h| h.to_lowercase() == name_lower) {
            return true;
        }

        // Check prefixes
        self.allowed_prefixes.iter().any(|prefix| {
            name_lower.starts_with(&prefix.to_lowercase())
        })
    }
}
```

#### 3. Gateway Handler Updates
**File**: `rust/public/src/servers/http_gateway.rs`

Update handler to forward headers and augment origin tracking:
```rust
pub async fn handle_query(
    Extension(config): Extension<Arc<HeaderForwardingConfig>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>, // Client connection info
    headers: HeaderMap,
    Json(request): Json<QueryRequest>,
) -> Result<String, GatewayError> {
    let flightsql_url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
        .context("MICROMEGAS_FLIGHTSQL_URL not set")?;

    // Build FlightSQL client
    let channel = create_grpc_channel(&flightsql_url).await?;
    let mut client_builder = FlightSqlServiceClient::new(channel);

    // Build origin tracking metadata
    let origin_metadata = build_origin_metadata(&headers, &addr);

    let mut metadata = tonic::metadata::MetadataMap::new();

    // Add origin metadata first (x-client-type, x-request-id, x-client-ip)
    metadata.extend(origin_metadata);

    // Forward allowed headers from client
    for (name, value) in headers.iter() {
        let header_name = name.as_str();

        // Skip headers already set by origin metadata
        if metadata.contains_key(header_name) {
            continue; // Origin metadata takes precedence
        }

        if config.should_forward(header_name) {
            if let Ok(metadata_value) = value.to_str() {
                if let Ok(parsed) = metadata_value.parse() {
                    metadata.insert(header_name, parsed);
                }
            }
        }
    }

    // Log request
    let request_id = metadata.get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let client_type = metadata.get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    info!("Gateway request: request_id={}, client_type={}, sql={}",
          request_id, client_type, &request.sql);

    // Execute query with forwarded headers
    let mut client = client_builder.with_interceptor(move |mut req| {
        *req.metadata_mut() = metadata.clone();
        Ok(req)
    });

    // ... rest of query execution
}
```

**Key changes**:
- No `AuthContext` parameter - no auth validation in gateway
- Simpler logging - just request ID and client type
- FlightSQL handles all authentication based on forwarded headers

#### 4. Origin Tracking Implementation
**File**: `rust/public/src/servers/http_gateway.rs`

Add a dedicated function for building origin metadata:
```rust
/// Build origin tracking metadata for FlightSQL queries
/// Augments the client type by appending "+gateway" to preserve the full client chain
///
/// This function only sets origin tracking headers that the gateway controls:
/// - x-client-type: augmented with "+gateway"
/// - x-request-id: generated if not present
/// - x-client-ip: extracted from actual connection (prevents spoofing)
///
/// User attribution headers (x-user-id, x-user-email) are forwarded from client
/// if present in allowed_headers config. FlightSQL validates these against the
/// Authorization token.
fn build_origin_metadata(
    headers: &HeaderMap,
    addr: &SocketAddr,
) -> tonic::metadata::MetadataMap {
    let mut metadata = tonic::metadata::MetadataMap::new();

    // 1. Client Type - augment existing or set to "unknown+gateway"
    let original_client_type = headers
        .get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let augmented_client_type = format!("{}+gateway", original_client_type);
    metadata.insert("x-client-type", augmented_client_type.parse().unwrap());

    // 2. Request ID - generate UUID if not present
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    metadata.insert("x-request-id", request_id.parse().unwrap());

    // 3. Client IP - ALWAYS extract from connection (never from client header)
    // SECURITY: Prevents IP spoofing in audit logs
    let client_ip = http_utils::get_client_ip(headers, addr);
    metadata.insert("x-client-ip", client_ip.parse().unwrap());

    metadata
}
```

**Expected FlightSQL Log Output** (client provides user info):
```
INFO execute_query range=None sql="SELECT * FROM streams" limit=None user=alice@example.com email=alice@example.com client=web+gateway
```

**Expected FlightSQL Log Output** (client doesn't provide user info, unauthenticated):
```
INFO execute_query range=None sql="SELECT * FROM streams" limit=None user=unknown email=unknown client=unknown+gateway
```

**Client Chain Examples**:
- Web app through gateway: `client=web+gateway`
- Python client through gateway: `client=python+gateway`
- Direct curl/unknown client: `client=unknown+gateway`
- Multi-hop scenario (future): `client=web+proxy+gateway`

**User Attribution Flow**:

**Case 1: OIDC User Token (End User)**:
1. Client sends `Authorization: Bearer <oidc_token>` + optionally `x-user-id`, `x-user-email`
2. Gateway forwards all headers
3. FlightSQL validates OIDC token, extracts subject/email
4. FlightSQL checks if `x-user-id`/`x-user-email` match token claims
5. If mismatch: **REJECT with 403 Forbidden** (user impersonation attempt)
6. If match or not provided: Use token claims for logging

**Case 2: API Key / Client Credentials (Service Account)**:
1. Service sends `Authorization: Bearer <api_key>` + `x-user-id: alice@example.com`
2. Gateway forwards all headers
3. FlightSQL validates API key (identifies service account)
4. FlightSQL **ALLOWS** service account to specify user on behalf of
5. Uses `x-user-id` from header for logging (acting as alice)

**Security Invariant**:
- OIDC user tokens: User identity MUST match token (no impersonation)
- API keys/client credentials: Service accounts CAN act on behalf of users (delegation)

### Environment Variables

#### Header Forwarding
- `MICROMEGAS_GATEWAY_HEADERS`: JSON configuration for header forwarding
  ```json
  {
    "allowed_headers": ["Authorization", "X-Request-ID", "X-Tenant-ID"],
    "allowed_prefixes": ["X-Custom-"],
    "blocked_headers": ["Cookie", "Set-Cookie"]
  }
  ```

#### Gateway Configuration
- `MICROMEGAS_FLIGHTSQL_URL`: FlightSQL backend gRPC endpoint (existing)

## Implementation Phases

### Phase 1: FlightSQL User Impersonation Prevention (CRITICAL) ✅ COMPLETED
**Goal**: Prevent OIDC users from impersonating others while allowing service account delegation

**Status**: ✅ Completed - All tasks finished and tested

#### Completed Tasks:

1. ✅ **Updated `AuthContext`** in `rust/auth/src/types.rs:31`
   - Added `allow_delegation: bool` field
   - Field indicates whether authentication allows user delegation
   - `auth_type: AuthType` field already existed

2. ✅ **Updated authentication providers** in `rust/auth/src/`
   - `oidc.rs:447`: Set `allow_delegation: false` - OIDC users cannot impersonate
   - `api_key.rs:125`: Set `allow_delegation: true` - Service accounts can delegate
   - Added security comments documenting admin restrictions (API keys always `is_admin: false`)

3. ✅ **Created user attribution validation module** `rust/auth/src/user_attribution.rs`
   - **New reusable function**: `validate_and_resolve_user_attribution()`
   - Validates `x-user-id` and `x-user-email` headers against authenticated identity
   - **OIDC users**: Headers MUST match token claims or return 403 Forbidden
   - **API keys**: Can specify user on behalf of (delegation allowed)
   - **Unauthenticated**: Pass through client-provided attribution
   - Returns `(user_id, user_email, service_account_name)` tuple
   - Integrated into FlightSQL service at `rust/public/src/servers/flight_sql_service_impl.rs:220`

4. ✅ **Updated Tower authentication service** `rust/auth/src/tower.rs:102-122`
   - Injects authentication metadata into gRPC headers:
     - `x-auth-subject`: Authenticated user's subject/ID
     - `x-auth-email`: Authenticated user's email
     - `x-auth-issuer`: Authentication issuer
     - `x-allow-delegation`: Whether delegation is allowed

5. ✅ **Updated query logging** in FlightSQL service
   - Logs service account identity when delegation is used
   - Example with delegation: `user=alice@example.com service_account=backend-service client=web+gateway`
   - Example without delegation: `user=alice@example.com email=alice@example.com client=web`

6. ✅ **Comprehensive test coverage** - 8 tests in `rust/auth/src/user_attribution.rs:119-309`
   - ✅ OIDC user with matching headers → 200 OK
   - ✅ OIDC user with no user headers → 200 OK (uses token claims)
   - ✅ OIDC user with mismatched x-user-id → 403 Forbidden
   - ✅ OIDC user with mismatched x-user-email → 403 Forbidden
   - ✅ API key with x-user-id delegation → 200 OK
   - ✅ API key without delegation → 200 OK (uses service account name)
   - ✅ Unauthenticated with user headers → 200 OK
   - ✅ Unauthenticated without user headers → 200 OK (defaults to unknown)
   - All tests passing ✅

#### Security Benefits Achieved:
- ✅ Prevents OIDC user impersonation (Alice cannot claim to be Bob)
- ✅ Allows service account delegation (Backend services can act on behalf of users)
- ✅ Enforces admin-only for OIDC (API keys can never be admins)
- ✅ Complete audit trail (Logs show both acting user and service account when delegation occurs)

#### Phase 1 Success Criteria - All Met ✅:
- ✅ User impersonation prevention implemented and tested
- ✅ OIDC users cannot forge x-user-id or x-user-email headers
- ✅ API keys can delegate (act on behalf of users)
- ✅ FlightSQL validates user attribution against authentication context
- ✅ Query logs show service account when delegation is used
- ✅ 8 comprehensive tests covering all scenarios (100% passing)
- ✅ No breaking changes to existing authentication flows
- ✅ API keys cannot be admins (hardcoded to false)

### Phase 2: Gateway Origin Tracking and Basic Header Forwarding
**Goal**: Add gateway origin tracking and configurable header forwarding

1. Create `rust/http-gateway/src/config.rs`
   - Implement `HeaderForwardingConfig` struct
   - Add `should_forward()` logic with prefix matching
   - Add `from_env()` for environment-based configuration
   - Write unit tests for header matching logic

2. Update `rust/public/src/servers/http_gateway.rs`
   - Add `build_origin_metadata()` function for origin tracking
   - Accept `Extension<Arc<HeaderForwardingConfig>>` in handler
   - Accept `ConnectInfo<SocketAddr>` for client IP extraction
   - Read incoming `x-client-type` header (if present)
   - Augment client type by appending `+gateway` (e.g., `web+gateway`, `unknown+gateway`)
   - Generate `x-request-id` if not present (UUID v4)
   - Extract client IP using `http_utils::get_client_ip()`
   - Set default user attribution: `x-user-id: anonymous`, `x-user-email: unknown`
   - Iterate through incoming headers and filter with `should_forward()`
   - Build gRPC metadata from filtered headers + origin tracking
   - Add request interceptor to include metadata in FlightSQL calls
   - Log request ID, client chain, and origin info for debugging

3. Update `rust/http-gateway/src/http_gateway_srv.rs`
   - Load configuration from environment
   - Add configuration as Axum extension/layer
   - Add ConnectInfo layer for client IP tracking

4. Testing
   - Unit tests for header filtering logic
   - Unit tests for `build_origin_metadata()` function
   - Unit test: client type augmentation (test `web` → `web+gateway`, `unknown` → `unknown+gateway`)
   - Integration test: send request with `x-client-type: python`, verify `python+gateway` forwarded
   - Integration test: send request without client type, verify `unknown+gateway` set
   - Integration test: verify request ID generation when not present
   - Integration test: verify request ID forwarded when provided
   - Test wildcard prefix matching
   - Test blocked header enforcement
   - **Security test**: Send `x-client-ip: 1.2.3.4` header, verify it's blocked and real connection IP is used instead
   - **Security test**: Send `x-user-id: test-user` header, verify it's forwarded to FlightSQL
   - **Security test**: Verify FlightSQL can validate and override user attribution based on Authorization token
   - Check FlightSQL logs for proper origin attribution with client chain

### Phase 3: Error Handling and Robustness
**Goal**: Improve gateway error handling and edge cases

1. Add comprehensive error handling
   - Handle FlightSQL connection failures gracefully
   - Return proper HTTP status codes (401, 500, etc.) based on gRPC errors
   - Map gRPC status codes to HTTP status codes
   - Include error details in response body

2. Add request validation
   - Validate SQL query is not empty
   - Validate request payload size limits
   - Add timeout configuration for FlightSQL requests

3. Improve logging
   - Log query execution time
   - Log errors with full context (request ID, client type, error details)
   - Add structured logging for better observability

4. Testing
   - Test FlightSQL service unreachable (connection error)
   - Test FlightSQL returns authentication error (401)
   - Test FlightSQL returns authorization error (403)
   - Test FlightSQL returns invalid query error (400)
   - Test very large query payloads
   - Test connection timeout scenarios

### Phase 4: Enhanced Security and Observability
**Goal**: Add security hardening and monitoring for both gateway and FlightSQL

1. Security improvements
   - Add rate limiting per authenticated user
   - Add audit logging for authentication events
   - Document security best practices
   - Add header size limits to prevent header bombing

2. Observability
   - Add metrics for authenticated vs unauthenticated requests
   - Track header forwarding statistics
   - Monitor auth provider cache hit rates
   - Add tracing spans for auth validation

3. Documentation
   - Update gateway README with authentication setup
   - Add example configurations for common use cases
   - Document header forwarding patterns and security considerations
   - Add troubleshooting guide for auth issues

### Phase 5: Advanced Features (Optional)
**Goal**: Support additional use cases

1. Multi-backend routing
   - Support routing to different backends based on headers/auth
   - Allow per-backend header forwarding configuration

2. Header transformation
   - Allow renaming headers (e.g., `X-User-ID` → `X-Subject`)
   - Support injecting headers from auth context

3. WebSocket support
   - Extend authentication to WebSocket upgrades
   - Forward headers in WebSocket handshake

## Origin Tracking Examples

### Example Query Flow with Authentication

**Client Request**:
```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Authorization: Bearer eyJhbGc..." \
  -H "X-Request-ID: req-12345-abc" \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT * FROM processes LIMIT 10"}'
```

**Gateway Processing** (with OIDC auth enabled):
1. Validates Bearer token via OIDC provider
2. Extracts user info from token: `subject=alice@example.com`, `email=alice@example.com`
3. Reads incoming headers - no `x-client-type` provided
4. Builds origin metadata:
   - `x-client-type: unknown+gateway` (augmented with +gateway)
   - `x-request-id: req-12345-abc` (forwarded from client)
   - `x-client-ip: 192.168.1.100` (from X-Forwarded-For or socket)
   - `x-user-id: alice@example.com`
   - `x-user-email: alice@example.com`
5. Forwards to FlightSQL with all metadata

**Gateway Logs**:
```
INFO Authenticated request from user: alice@example.com (alice@example.com), client_chain: unknown+gateway, request_id: req-12345-abc
```

**FlightSQL Logs**:
```
INFO execute_query range=None sql="SELECT * FROM processes LIMIT 10" limit=None user=alice@example.com email=alice@example.com client=unknown+gateway
```

### Example Query Flow without Authentication

**Client Request**:
```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT * FROM streams"}'
```

**Gateway Processing** (no auth configured):
1. No authentication validation
2. Reads incoming headers - no `x-client-type` provided
3. Builds origin metadata with defaults:
   - `x-client-type: unknown+gateway` (augmented)
   - `x-request-id: 550e8400-e29b-41d4-a716-446655440000` (generated UUID)
   - `x-client-ip: 192.168.1.100`
   - `x-user-id: anonymous`
   - `x-user-email: unknown`
4. Forwards to FlightSQL

**Gateway Logs**:
```
INFO Unauthenticated request, client_chain: unknown+gateway, request_id: 550e8400-e29b-41d4-a716-446655440000
```

**FlightSQL Logs**:
```
INFO execute_query range=None sql="SELECT * FROM streams" limit=None user=anonymous email=unknown client=unknown+gateway
```

### Example Query Flow with Client Type Header

**Client Request** (web app providing client type):
```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Authorization: Bearer eyJhbGc..." \
  -H "X-Client-Type: web" \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT * FROM traces"}'
```

**Gateway Processing**:
1. Validates Bearer token
2. Reads incoming `x-client-type: web`
3. Augments to `web+gateway`
4. Forwards to FlightSQL

**FlightSQL Logs**:
```
INFO execute_query sql="SELECT * FROM traces" user=alice@example.com email=alice@example.com client=web+gateway
```

This preserves the full client chain, showing the query originated from the web app but went through the gateway.

## Configuration Examples

### Example 1: OIDC with Standard Headers
```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://accounts.google.com",
    "audience": "myapp.apps.googleusercontent.com"
  }]
}'

export MICROMEGAS_GATEWAY_HEADERS='{
  "allowed_headers": ["Authorization", "X-Request-ID", "User-Agent"],
  "blocked_headers": ["Cookie"]
}'

cargo run --bin http-gateway -- --listen-endpoint-http 0.0.0.0:3000
```

### Example 2: API Key with Custom Headers
```bash
export MICROMEGAS_API_KEYS='[
  {"name": "service1", "key": "secret-key-123"},
  {"name": "service2", "key": "secret-key-456"}
]'

export MICROMEGAS_GATEWAY_HEADERS='{
  "allowed_headers": ["Authorization"],
  "allowed_prefixes": ["X-Custom-", "X-Tenant-"],
  "blocked_headers": []
}'

cargo run --bin http-gateway
```

### Example 3: No Authentication (Current Behavior)
```bash
# No auth env vars set - gateway operates without authentication

export MICROMEGAS_GATEWAY_HEADERS='{
  "allowed_headers": ["X-Request-ID", "X-Correlation-ID"]
}'

cargo run --bin http-gateway
```

### Example 4: Proprietary Auth Scheme
```bash
# Use custom header forwarding for proprietary authentication
# Backend service validates X-Proprietary-Token header

export MICROMEGAS_GATEWAY_HEADERS='{
  "allowed_headers": ["X-Proprietary-Token", "X-Tenant-ID", "X-Session-ID"],
  "allowed_prefixes": ["X-Custom-"],
  "blocked_headers": ["Cookie", "Set-Cookie"]
}'

cargo run --bin http-gateway
```

## Testing Strategy

### Unit Tests
- Header forwarding logic (`config::should_forward`)
- Header name matching (case-insensitive)
- Prefix matching with wildcards
- Blocked header enforcement

### Integration Tests
1. **Header Forwarding Tests**
   - Send request with 10 headers, verify only allowed headers forwarded
   - Test prefix matching with various custom headers
   - Verify blocked headers are never forwarded

2. **Authentication Tests**
   - Valid OIDC token → 200 with forwarded headers
   - Invalid token → 401 Unauthorized
   - Expired token → 401 Unauthorized
   - Valid API key → 200 with forwarded headers
   - Invalid API key → 401 Unauthorized
   - No auth configured → 200 (no validation)

3. **End-to-End Tests**
   - Start gateway with OIDC config
   - Obtain real OIDC token from test provider
   - Send query with token and custom headers
   - Verify FlightSQL receives correct headers and executes query

### Performance Tests
- Measure auth validation latency (should use token cache)
- Test header forwarding overhead (should be minimal)
- Verify JWKS cache effectiveness

## Security Considerations

### Authentication
- **Gateway does not validate authentication** - acts as transparent proxy
- All authentication handled by FlightSQL service
- Authorization header forwarded as-is
- FlightSQL validates tokens and returns 401/403 errors
- Gateway passes through auth errors to client

### FlightSQL User Impersonation Prevention (CRITICAL)
**FlightSQL MUST implement these security checks**:

1. **Detect Token Type**:
   - OIDC token: JWT with issuer claim (e.g., `iss: "https://accounts.google.com"`)
   - API key: Simple bearer token or Basic auth (no JWT structure)
   - Client credentials: OAuth2 client_credentials grant (service account)

2. **Validate User Attribution Headers**:
   ```rust
   match auth_type {
       AuthType::Oidc => {
           // OIDC user token - user cannot impersonate others
           if let Some(claimed_user) = metadata.get("x-user-id") {
               if claimed_user != token_subject {
                   return Err(Status::permission_denied(
                       "x-user-id must match OIDC token subject"
                   ));
               }
           }
           if let Some(claimed_email) = metadata.get("x-user-email") {
               if claimed_email != token_email {
                   return Err(Status::permission_denied(
                       "x-user-email must match OIDC token email"
                   ));
               }
           }
           // Use token claims for logging
           user_id = token_subject;
           user_email = token_email;
       }
       AuthType::ApiKey | AuthType::ClientCredentials => {
           // Service account - can act on behalf of users
           user_id = metadata.get("x-user-id").unwrap_or("service-account");
           user_email = metadata.get("x-user-email").unwrap_or("unknown");
           // Log both service account and acting-as user
       }
   }
   ```

3. **Error Responses**:
   - OIDC user tries to impersonate: `403 Forbidden: "User impersonation not allowed"`
   - Missing/invalid token: `401 Unauthorized`
   - Valid service account with delegation: `200 OK`

**This prevents scenarios like**:
- Alice (OIDC user) sends `x-user-id: bob@example.com` → **403 Forbidden**
- Service account sends `x-user-id: alice@example.com` → **200 OK** (delegation allowed)

### Client IP Security
- **Client IP ALWAYS extracted from connection**, never from client headers
- Gateway blocks `x-client-ip` header from clients
- Supports `X-Forwarded-For` from trusted reverse proxies
- Prevents IP spoofing in audit logs and analytics

### Header Forwarding
- Default configuration blocks sensitive headers (Cookie, Set-Cookie)
- Gateway blocks `x-client-ip` header (always sets from connection)
- **User attribution headers forwarded** (x-user-id, x-user-email, x-user-name)
  - Gateway forwards these headers if present
  - **FlightSQL validates against Authorization token** (see impersonation prevention above)
  - OIDC users: Must match token claims or be omitted
  - Service accounts: Can delegate (act on behalf of users)
- Explicit allowlist approach (deny by default)
- Header size limits to prevent resource exhaustion
- Case-insensitive matching to prevent bypass attempts
- Origin metadata takes precedence over client headers for: x-client-type, x-request-id, x-client-ip

### Audit and Compliance
- Gateway logs all requests with request ID and client type
- FlightSQL logs authentication events and user attribution
- Full traceability through X-Request-ID forwarding (gateway → FlightSQL)
- Client IP tracking for security analysis

## Backward Compatibility

### Guarantee
- Gateway operates as transparent HTTP-to-gRPC proxy
- Existing clients continue to work without changes
- Default header forwarding config includes Authorization and common headers
- No breaking changes to API contract

### Migration Path
1. Deploy updated gateway with default configuration
2. Verify existing functionality unchanged
3. Optionally configure additional header forwarding rules
4. Clients can start using new origin tracking headers (x-client-type, x-request-id)
5. Verify FlightSQL logs show augmented client types (e.g., web+gateway)

## Success Criteria

### Functional Requirements
- [ ] Authorization header forwarded to FlightSQL (no validation in gateway)
- [ ] Configurable header allowlist with prefix matching
- [ ] Blocked headers never forwarded
- [ ] Gateway acts as transparent proxy
- [ ] FlightSQL authentication errors passed through to client
- [ ] Headers forwarded to FlightSQL backend via gRPC metadata

### Origin Tracking Requirements
- [ ] `x-client-type` augmented by appending `+gateway` to preserve client chain
- [ ] Client type examples work correctly:
  - `web` → `web+gateway`
  - `python` → `python+gateway`
  - `grafana` → `grafana+gateway`
  - (not provided) → `unknown+gateway`
- [ ] `x-request-id` generated (UUID v4) if not provided by client
- [ ] `x-request-id` forwarded if provided by client
- [ ] `x-client-ip` extracted from connection and set by gateway
- [ ] `x-client-ip` header from client always ignored (security)
- [ ] User attribution headers forwarded from client if present:
  - `x-user-id` forwarded (FlightSQL validates against token)
  - `x-user-email` forwarded (FlightSQL validates against token)
  - `x-user-name` forwarded (FlightSQL validates against token)
- [ ] FlightSQL logs show correct origin information with full client chain
- [ ] Request ID appears in both gateway logs and FlightSQL logs for correlation

### Non-Functional Requirements
- [ ] Header forwarding adds < 1ms overhead
- [ ] No authentication overhead in gateway (delegated to FlightSQL)
- [ ] No breaking changes to existing clients
- [ ] Comprehensive test coverage (>80%)
- [ ] Clear documentation and examples

### Documentation Requirements
- [ ] README updated with authentication setup instructions
- [ ] Environment variable reference documented
- [ ] Security best practices guide
- [ ] Configuration examples for common scenarios
- [ ] Troubleshooting guide for auth issues

## Related Work
- Analytics web app OIDC implementation (commit 71ce310ce)
- Existing auth crate with OIDC and API key support
- FlightSQL service authentication (already supports auth headers)
- Python client auth_provider parameter (#595)

## Future Enhancements
- mTLS support for backend connections
- Request/response body transformation
- GraphQL gateway endpoint
- WebSocket proxying
- Multi-backend routing based on headers
- Header injection from auth context (X-User-Email from token)
