# Multi-Auth Provider Rework

## Objective
Refactor `MultiAuthProvider` to use a vector of `Arc<dyn AuthProvider>` instead of hardcoded `ApiKeyAuthProvider` and `OidcAuthProvider` fields. This will enable users to add their enterprise authentication providers to the authentication chain.

## Current State
**Location:** `rust/auth/src/multi.rs`

**Current Structure:**
```rust
pub struct MultiAuthProvider {
    pub api_key_provider: Option<Arc<ApiKeyAuthProvider>>,
    pub oidc_provider: Option<Arc<OidcAuthProvider>>,
}
```

**Current Implementation:**
- Hardcoded to only support API key (tried first) and OIDC (tried second)
- Uses if-let chains to try each provider sequentially
- Returns first successful authentication or fails with "authentication failed with all providers"

**Current Usage Sites:**
1. `rust/telemetry-ingestion-srv/src/main.rs` (lines 141-144)
2. `rust/flight-sql-srv/src/flight_sql_srv.rs` (lines 85-88)

Both services follow the same pattern:
```rust
Some(Arc::new(MultiAuthProvider {
    api_key_provider,
    oidc_provider,
}) as Arc<dyn AuthProvider>)
```

## Proposed Changes

### 1. Update MultiAuthProvider Structure
**File:** `rust/auth/src/multi.rs`

Change from:
```rust
pub struct MultiAuthProvider {
    pub api_key_provider: Option<Arc<ApiKeyAuthProvider>>,
    pub oidc_provider: Option<Arc<OidcAuthProvider>>,
}
```

To:
```rust
pub struct MultiAuthProvider {
    providers: Vec<Arc<dyn AuthProvider>>,
}
```

### 2. Add Builder API
Add methods for construction:
```rust
impl MultiAuthProvider {
    pub fn new() -> Self {
        Self { providers: Vec::new() }
    }
    
    pub fn with_provider(mut self, provider: Arc<dyn AuthProvider>) -> Self {
        self.providers.push(provider);
        self
    }
}
```

### 3. Update validate_token Implementation
Simplify to iterate over providers:
```rust
async fn validate_token(&self, token: &str) -> anyhow::Result<AuthContext> {
    for provider in &self.providers {
        if let Ok(auth_ctx) = provider.validate_token(token).await {
            return Ok(auth_ctx);
        }
    }
    anyhow::bail!("authentication failed with all providers")
}
```

### 4. Update Tests
**File:** `rust/auth/src/multi.rs` (lines 77-122)

Update three existing tests to use new builder API:
- `test_multi_provider_api_key`
- `test_multi_provider_no_providers`
- `test_multi_provider_invalid_token`

### 5. Update Usage Sites

**File:** `rust/telemetry-ingestion-srv/src/main.rs` (lines 113-154)

Change from:
```rust
Some(Arc::new(MultiAuthProvider {
    api_key_provider,
    oidc_provider,
}) as Arc<dyn AuthProvider>)
```

To:
```rust
let mut multi = MultiAuthProvider::new();
if let Some(provider) = api_key_provider {
    multi = multi.with_provider(provider);
}
if let Some(provider) = oidc_provider {
    multi = multi.with_provider(provider);
}
Some(Arc::new(multi) as Arc<dyn AuthProvider>)
```

**File:** `rust/flight-sql-srv/src/flight_sql_srv.rs` (lines 58-95)

Apply same transformation.

### 6. Update Documentation

#### Rust Documentation
**File:** `rust/auth/src/multi.rs` (lines 1-48)

Update module documentation and example to show:
- How to use the builder pattern
- How to add custom enterprise auth providers
- Order of providers matters (first match wins)

**File:** `rust/auth/src/lib.rs`

Update crate-level documentation to mention extensibility.

#### MkDocs Documentation
**File:** `mkdocs/docs/admin/authentication.md` (line 12)

Current text:
```
Both methods can be enabled simultaneously, with API key validation tried first (fast path) before falling back to OIDC validation.
```

Update to be more general:
```
Both methods can be enabled simultaneously. When multiple providers are configured, they are tried in order until one succeeds (API key first for performance, then OIDC).
```

**Analysis:** The documentation is general enough and focuses on the two built-in providers (API key and OIDC). The new implementation maintains backward compatibility, so no other documentation changes are required. The mention of "multi-provider" on line 606 refers to multiple OIDC issuers, not the MultiAuthProvider type.

## Benefits
- **Extensibility:** Users can inject custom `AuthProvider` implementations for enterprise SSO, SAML, LDAP, etc.
- **Flexibility:** Dynamic composition of authentication chains at runtime
- **Backward Compatibility:** Existing API key and OIDC providers work unchanged
- **Simplicity:** Cleaner implementation with single loop instead of nested if-let chains

## Implementation Considerations
- Provider order matters for authentication precedence (first successful match wins)
- Empty provider list should return error immediately (no providers available)
- Thread-safety is maintained through `Arc<dyn AuthProvider>`
- Error handling matches current behavior (returns first success or fails after trying all)
- No need to accumulate individual errors - current behavior just returns generic failure message

## Testing Strategy
1. Run existing unit tests in `rust/auth/src/multi.rs`
2. Build both services: `cargo build -p micromegas-telemetry-ingestion-srv -p micromegas-flight-sql-srv`
3. Run full auth crate tests: `cargo test -p micromegas-auth`
4. Verify backward compatibility with current usage patterns

## Migration Path for Users
No breaking changes for current built-in providers. New capability adds:
```rust
use micromegas_auth::multi::MultiAuthProvider;

let mut auth = MultiAuthProvider::new()
    .with_provider(Arc::new(ApiKeyAuthProvider::new(keyring)))
    .with_provider(Arc::new(OidcAuthProvider::new(config).await?))
    .with_provider(Arc::new(MyEnterpriseAuthProvider::new())); // Custom!
```
