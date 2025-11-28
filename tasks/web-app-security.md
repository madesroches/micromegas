# Web App Security Improvements

## Goal
Tighten security of analytics-web-srv so it properly validates tokens at the web tier instead of being a "dumb proxy" that forwards requests to FlightSQL for validation.

## Current State (After Phase 1)
- Web tier now performs **full JWT signature validation** using JWKS
- Tokens are validated at the web tier before forwarding to FlightSQL
- Uses the same `micromegas-auth` crate as FlightSQL for consistent validation
- `MICROMEGAS_OIDC_CONFIG` is now **required** when auth is enabled
- `start_analytics_web.py` auto-detects OIDC config and uses `--disable-auth` if not present

## Security Tasks

### Phase 1: Token Validation at Web Tier ✅ COMPLETED

- [x] **Integrate `micromegas-auth` crate into analytics-web-srv**
  - Added dependency to Cargo.toml (already present)
  - Used `OidcAuthProvider` for JWKS caching and signature validation
  - Replaced manual JWT decoding in `auth_me()` with proper validation via OidcAuthProvider
  - File: `rust/analytics-web-srv/src/auth.rs`

- [x] **Add full JWT validation in middleware**
  - Validate signature using JWKS (with caching via moka)
  - Validate issuer claim against configured providers
  - Validate audience claim
  - Reject invalid tokens before forwarding to FlightSQL
  - File: `rust/analytics-web-srv/src/auth.rs` (cookie_auth_middleware)

- [x] **Extract and cache user claims**
  - Parse validated JWT claims into `ValidatedUser` struct
  - Made user context (sub, email, issuer, is_admin) available to handlers via request extensions
  - Useful for audit logging and future authorization decisions

- [x] **Update development scripts**
  - Updated `analytics-web-app/start_analytics_web.py` to detect OIDC config
  - Automatically uses `--disable-auth` when `MICROMEGAS_OIDC_CONFIG` is not set
  - Runs with full auth when OIDC config is present in environment

### Phase 2: Rate Limiting & Protection

- [ ] **Add authentication rate limiting**
  - Rate limit `/auth/login` to prevent enumeration attacks
  - Rate limit `/auth/refresh` to prevent token refresh abuse
  - Rate limit failed authentication attempts by IP
  - Consider using tower-governor or similar middleware

- [ ] **Add request rate limiting per user**
  - Once tokens are validated, rate limit by user subject (sub)
  - Prevents authenticated users from abusing API
  - Different limits for different endpoints (data queries vs health checks)

### Phase 3: Audit & Observability

- [ ] **Add security event logging**
  - Log all authentication failures with details (IP, token fragment, reason)
  - Log successful logins (user sub, provider)
  - Log token refresh events
  - Use structured logging for easy analysis

- [ ] **Add user attribution to API requests**
  - Include user subject in request spans/logs
  - Enables per-user query analysis
  - Helps debug issues and track usage patterns

### Phase 4: Token Revocation (Future)

- [ ] **Design token revocation mechanism**
  - Options: blacklist table, short-lived tokens + refresh, or distributed cache
  - Consider trade-offs: latency vs security vs complexity
  - Document decision and rationale

- [ ] **Implement revocation check in middleware**
  - Check token against revocation list
  - Handle revocation at logout time
  - Admin ability to revoke all tokens for a user

### Phase 5: Multi-Provider Support

- [ ] **Support multiple OIDC issuers in web app**
  - Currently enforces exactly 1 issuer (unlike FlightSQL which supports multiple)
  - Update parsing to accept array of issuers
  - Validate token against appropriate issuer based on `iss` claim
  - File: `rust/analytics-web-srv/src/auth.rs` (OidcIssuerConfig parsing)

## Implementation Notes

### Using micromegas-auth crate
The `micromegas-auth` crate already has:
- JWKS caching with configurable TTL
- Signature validation for RS256/RS384/RS512
- Multi-issuer support
- Audience validation
- `OidcAuthProvider` trait implementation

Example integration:
```rust
// In auth.rs or new validation module
use micromegas_auth::{OidcAuthProvider, OidcIssuerConfig};

let provider = micromegas_auth::default_provider::provider().await?;
let claims = provider.validate_token(&token).await?;
```

### Cookie Middleware Changes
Current middleware (`cookie_auth_middleware`) does:
1. Extract token from cookie
2. Check JWT has 3 parts
3. Decode claims (no signature check)
4. Check expiration

Should become:
1. Extract token from cookie
2. Validate signature via JWKS (cached)
3. Validate issuer and audience
4. Check expiration
5. Extract validated claims into request extensions

### Error Handling
Return appropriate HTTP status codes:
- 401 Unauthorized: Invalid/expired token, signature mismatch
- 403 Forbidden: Valid token but insufficient permissions (future)
- 429 Too Many Requests: Rate limited

## Testing Plan

- [ ] Unit tests for token validation logic
- [ ] Integration tests with mock JWKS endpoint
- [ ] Test expired token rejection
- [ ] Test invalid signature rejection
- [ ] Test rate limiting behavior
- [ ] Test multi-issuer scenarios (once implemented)
- [ ] Load test to ensure JWKS caching works under load

## Related Files

| File | Changes |
|------|---------|
| `rust/analytics-web-srv/Cargo.toml` | Add micromegas-auth dependency |
| `rust/analytics-web-srv/src/auth.rs` | Main validation logic changes |
| `rust/analytics-web-srv/src/main.rs` | Middleware setup, rate limiting |
| `rust/auth/src/oidc.rs` | Reference implementation |
