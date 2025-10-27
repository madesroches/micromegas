# Proposal: Unified JWKS Architecture for Service Accounts

> **STATUS: SUPERSEDED**
> **Date:** 2025-10-24
> **Reason:** After further discussion, we decided to use OAuth 2.0 client credentials flow for service accounts instead of self-signed JWTs with local JWKS. This simplifies the architecture significantly.
> **See:** [Service Account Strategy Change](service_account_strategy_change.md) for details.
> **This document is kept for historical reference to document the decision-making process.**

## Summary

This PR proposes a **unified JWT validation architecture** that reuses the OIDC infrastructure for service account authentication instead of implementing a separate validation path.

## The Change

**Before (original plan)**:
- Two separate authentication implementations
  - `OidcAuthProvider` - validates human user tokens (remote JWKS)
  - `ServiceAccountAuthProvider` - validates service tokens (custom logic)
- Duplicated JWT validation logic
- ~500-800 lines of code

**After (this proposal)**:
- Single unified authentication implementation
  - `UnifiedJwtAuthProvider` - validates ALL JWT tokens
  - Supports multiple JWKS sources (remote AND local)
- Shared validation logic, cache, error handling
- ~300-400 lines of code (~40-50% reduction)

## Key Insight

**The system already requires OIDC/OAuth infrastructure for human users.**

Why implement a separate custom JWT validator for services when we can:
1. Use the **same validation code** for both humans and services
2. Support **local JWKS** (hard-coded/database) alongside remote JWKS
3. Keep **offline token generation** (no network calls)
4. Get **standard OAuth compatibility** for free

## Technical Approach

```rust
// Unified validator with pluggable JWKS sources
struct UnifiedJwtAuthProvider {
    jwks_sources: Vec<JwksSource>,
    token_cache: Cache<String, AuthContext>,
}

enum JwksSource {
    Remote {
        issuer: String,
        jwks_url: Url,  // For OIDC providers (humans)
    },
    Local {
        issuer: String,
        keys: JsonWebKeySet,  // For service accounts (from database)
    },
}
```

**Configuration example**:
```rust
// Human users - remote OIDC
JwksSource::Remote {
    issuer: "https://accounts.google.com",
    jwks_url: "https://www.googleapis.com/oauth2/v3/certs",
}

// Service accounts - local JWKS (from database)
JwksSource::Local {
    issuer: "micromegas-service-accounts",
    keys: load_service_account_public_keys_from_db(),
}
```

## What We're NOT Changing

✅ Services still generate tokens **offline** (no network calls)  
✅ Token validation still happens **offline** (local JWKS)  
✅ No external OAuth server needed  
✅ No additional dependencies  
✅ Same performance characteristics  
✅ All original requirements still met  

## What We're Gaining

✅ **Code reuse**: Single validation path instead of two  
✅ **Less code**: ~40-50% reduction in implementation  
✅ **Consistency**: Same token format, same errors, same caching  
✅ **Future-proofing**: Easy migration if requirements change  
✅ **Standard OAuth format**: Works with OAuth debugging tools  
✅ **Maintainability**: One codebase to test and maintain  

## Why This Makes Sense

The original plan treats OIDC and service accounts as completely separate concerns, requiring two implementations:

1. **OIDC validation** - fetch remote JWKS, validate JWT
2. **Service account validation** - fetch local public keys, validate JWT

But these are **fundamentally the same operation** with different JWKS sources!

By abstracting the JWKS source, we get:
- One JWT validator that works with any JWKS
- Local JWKS for service accounts (offline)
- Remote JWKS for OIDC providers (humans)
- Same validation logic for both

## Implementation Impact

**Phase 1**: Build UnifiedJwtAuthProvider with JWKS abstraction  
**Phase 2**: Add local JWKS from service account database  
**Phase 3**: Add remote JWKS for OIDC providers  

**Result**: Less code, same functionality, better architecture.

## Migration Path

If requirements change and we need a real OAuth server later:

**Current plan**: Rewrite ServiceAccountAuthProvider  
**This plan**: Just change local JWKS to remote JWKS source

Example:
```rust
// Current: Local JWKS
JwksSource::Local {
    issuer: "micromegas-service-accounts",
    keys: load_from_database(),
}

// Future: Point to OAuth server
JwksSource::Remote {
    issuer: "https://oauth.example.com",
    jwks_url: "https://oauth.example.com/.well-known/jwks.json",
}
```

No code changes needed in validation logic.

## Conclusion

This proposal maintains all the benefits of the original plan (offline operation, no external dependencies, high performance) while:
- Reducing code complexity
- Improving maintainability
- Future-proofing the architecture
- Using standard OAuth patterns

The key insight: **Since we're already building OIDC infrastructure, we should reuse it with local JWKS instead of building a parallel system.**
