# Security TODO for OIDC Authentication

Security issues identified during code review of PR #548.

## 🔴 CRITICAL - Must Fix Before Merge

### 1. ✅ Missing OAuth State Validation (CSRF Vulnerability) - COMPLETED

**File**: `python/micromegas/micromegas/auth/oidc.py:189-210`

**Status**: ✅ **FIXED** on 2025-10-27

**Implementation**: Added state parameter validation in OAuth callback handler:
- Extracts `state` parameter from callback URL before processing auth code
- Validates received state matches expected state generated during authorization
- Returns HTTP 400 with security error message if state is invalid
- Only extracts auth code after successful state validation

**Security properties**:
- Prevents CSRF attacks where attacker could link victim to attacker's account
- Auth code is never set if state validation fails
- Clear error messaging for potential security issues
- Follows OAuth 2.0 Security Best Current Practice

**Testing**: Python unit tests updated and passing (all 6 unit tests pass)

---

### 2. Token File Permission Race Condition

**File**: `python/micromegas/micromegas/auth/oidc.py:298-320`

**Issue**: Token file created with default permissions (potentially 644), then chmod'd to 0600. Brief window where tokens could be world-readable.

**Current Code**:
```python
with open(self.token_file, "w") as f:  # Default perms (644)
    json.dump({...}, f)
Path(self.token_file).chmod(0o600)  # Secure, but too late
```

**Fix Option 1** (Preferred):
```python
import os

# Create file with secure permissions atomically
fd = os.open(
    self.token_file,
    os.O_CREAT | os.O_WRONLY | os.O_TRUNC,
    0o600
)
with os.fdopen(fd, 'w') as f:
    json.dump({...}, f, indent=2)
```

**Fix Option 2**:
```python
# Set umask before opening
import os
old_umask = os.umask(0o077)  # Ensure 600 perms
try:
    with open(self.token_file, "w") as f:
        json.dump({...}, f, indent=2)
finally:
    os.umask(old_umask)
```

**Priority**: CRITICAL - Tokens contain refresh tokens with long lifetime

---

### 3. Insecure Parent Directory Permissions

**File**: `python/micromegas/micromegas/auth/oidc.py:304`

**Issue**: `~/.micromegas/` directory created without explicit secure permissions.

**Current Code**:
```python
Path(self.token_file).parent.mkdir(parents=True, exist_ok=True)
```

**Fix**:
```python
parent_dir = Path(self.token_file).parent
parent_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
# Ensure permissions even if directory already exists
parent_dir.chmod(0o700)
```

**Priority**: HIGH - Directory permissions should be as strict as file permissions

---

## 🟠 HIGH PRIORITY - Should Fix Soon

### 4. Global TCPServer Configuration Mutation

**File**: `python/micromegas/micromegas/auth/oidc.py:218`

**Issue**: Modifies class-level attribute, affecting all TCPServer instances globally.

**Current Code**:
```python
socketserver.TCPServer.allow_reuse_address = True  # Class variable!
server = socketserver.TCPServer(("", callback_port), CallbackHandler)
```

**Fix**:
```python
server = socketserver.TCPServer(("", callback_port), CallbackHandler)
server.allow_reuse_address = True  # Instance variable
```

**Priority**: HIGH - Could affect other code using TCPServer

---

### 5. Missing Server Cleanup on Exception

**File**: `python/micromegas/micromegas/auth/oidc.py:216-247`

**Issue**: If an exception occurs during token exchange, server might not be properly closed.

**Current Code**:
```python
server = socketserver.TCPServer(("", callback_port), CallbackHandler)
try:
    # ... auth flow
finally:
    server.server_close()  # Only closes socket, doesn't guarantee cleanup
```

**Fix**:
```python
server = None
try:
    server = socketserver.TCPServer(("", callback_port), CallbackHandler)
    server.allow_reuse_address = True
    # ... auth flow
finally:
    if server:
        try:
            server.shutdown()  # Stop serving
            server.server_close()  # Close socket
        except Exception:
            pass  # Best effort cleanup
```

**Priority**: HIGH - Port leaks can block subsequent auth attempts

---

## 🟡 MEDIUM PRIORITY - Performance & Defense in Depth

### 6. Inefficient JWT Validation (Potential Timing Attack)

**File**: `rust/auth/src/oidc.rs:298-358`

**Issue**: Code tries all issuers and all keys sequentially. Should decode JWT header first to extract issuer (iss) and key ID (kid).

**Current Approach**:
```rust
// Line 302: Comment acknowledges this is suboptimal
// "This is a simplified approach - in production we'd decode the payload first"
for client in self.clients.values() {
    for key in jwks.keys() {
        // Try validation...
    }
}
```

**Recommended Fix**:
```rust
use jsonwebtoken::decode_header;

async fn validate_id_token(&self, token: &str) -> Result<AuthContext> {
    // Decode header (unsigned) to get issuer and kid
    let header = decode_header(token)?;
    let kid = header.kid.ok_or_else(|| anyhow!("JWT missing kid"))?;

    // Decode payload (unsigned) to get issuer
    let unverified: Claims = jsonwebtoken::dangerous_insecure_decode(token)?.claims;

    // Look up specific issuer
    let client = self.clients.get(&unverified.iss)
        .ok_or_else(|| anyhow!("Unknown issuer"))?;

    // Get JWKS and find specific key by kid
    let jwks = client.jwks_cache.get().await?;
    let key = jwks.keys()
        .find(|k| k.key_id() == Some(&kid))
        .ok_or_else(|| anyhow!("Key not found"))?;

    // Validate with specific key
    let decoding_key = jwk_to_decoding_key(key)?;
    // ... rest of validation
}
```

**Benefits**:
- Eliminates timing side-channels
- Much faster (O(1) lookup vs O(n*m) iteration)
- Standard JWT validation pattern

**Priority**: MEDIUM - Works correctly now, but could leak information through timing

---

### 7. Missing Key ID (kid) Validation

**File**: `rust/auth/src/oidc.rs:310-323`

**Issue**: JWT header contains `kid` that should be matched against JWKS. Code tries all keys instead.

**Fix**: Same as issue #6 above - extract kid from header and match.

**Priority**: MEDIUM - Part of the same optimization

---

### 8. API Key Timing Attack (Theoretical)

**File**: `rust/auth/src/api_key.rs:77-91`

**Issue**: HashMap lookup doesn't use constant-time comparison. Sophisticated attacker with precise timing could potentially determine API key prefixes.

**Current Code**:
```rust
if let Some(name) = self.keyring.get(&key) {
    Ok(AuthContext { ... })
} else {
    Err(anyhow!("invalid API token"))
}
```

**Fix** (if needed for high-security environments):
```rust
use subtle::ConstantTimeEq;

// Compare all keys in constant time
let mut found: Option<AuthContext> = None;
for (stored_key, name) in &self.keyring {
    let matches = stored_key.value.as_bytes()
        .ct_eq(token.as_bytes())
        .unwrap_u8() == 1;

    if matches {
        found = Some(AuthContext {
            subject: name.clone(),
            // ...
        });
    }
}

found.ok_or_else(|| anyhow!("invalid API token"))
```

**Note**: This is a very difficult attack to exploit in practice. HashMap lookups are already fairly constant-time due to hashing. Only needed for extremely high-security environments.

**Priority**: LOW-MEDIUM - Difficult to exploit, but proper for security-critical systems

---

## 🟢 LOW PRIORITY - Future Improvements

### 9. Rate Limiting for Authentication Failures

**Files**: `rust/auth/src/oidc.rs`, `rust/auth/src/tower.rs`

**Issue**: No rate limiting on failed authentication attempts. Allows unlimited brute force attempts.

**Recommendation**:
- Add rate limiting middleware using Tower rate limit layer
- Track failed attempts per IP or per token prefix
- Exponential backoff for repeated failures

**Example**:
```rust
use tower::limit::RateLimitLayer;
use tower::ServiceBuilder;

let layer = ServiceBuilder::new()
    .layer(RateLimitLayer::new(
        100, // max requests
        Duration::from_secs(60) // per minute
    ))
    .layer(layer_fn(move |inner| AuthService { ... }))
    .into_inner();
```

**Priority**: LOW - Defense in depth, not critical for initial release

---

### 10. Token Revocation Checking

**File**: `rust/auth/src/oidc.rs:364-378`

**Issue**: Validated tokens cached for 5 minutes. Revoked tokens remain valid until cache expires.

**Current Behavior**: This is an acceptable trade-off for performance.

**Future Enhancement** (if needed):
- Add OIDC token introspection endpoint support
- Check revocation for high-privilege operations
- Add manual cache invalidation API

**Priority**: LOW - 5-minute TTL is reasonable for most use cases

---

### 11. Add Security Headers to OAuth Callback Response

**File**: `python/micromegas/micromegas/auth/oidc.py:198-211`

**Issue**: OAuth callback HTML response doesn't include security headers.

**Enhancement**:
```python
def do_GET(self):
    # ... existing code ...

    self.send_response(200)
    self.send_header("Content-type", "text/html; charset=utf-8")
    self.send_header("X-Content-Type-Options", "nosniff")
    self.send_header("X-Frame-Options", "DENY")
    self.send_header("Content-Security-Policy", "default-src 'none'")
    self.end_headers()

    # ... rest of response
```

**Priority**: LOW - Callback page is minimal and temporary

---

## ✅ Security Strengths (Already Implemented)

Good security practices already in place:

1. ✅ **SSRF Protection**: HTTP client disables redirects (`redirect::Policy::none()`)
2. ✅ **PKCE Support**: Properly implements PKCE for OAuth (S256)
3. ✅ **Secret Redaction**: API keys display as `<sensitive key>` in logs
4. ✅ **No Secret Persistence**: `client_secret` not saved to token files
5. ✅ **Token File Permissions**: Set to 0600 (after race condition fix)
6. ✅ **Generic Error Messages**: Don't leak information about which issuer/key failed
7. ✅ **HTTPS Required**: OIDC discovery requires HTTPS URLs
8. ✅ **Token Expiration**: Properly checks JWT expiration times
9. ✅ **Audience Validation**: Validates aud claim matches expected client_id
10. ✅ **Issuer Validation**: Validates iss claim matches configured issuer
11. ✅ **Thread-Safe Token Refresh**: Uses locks to prevent race conditions
12. ✅ **Automatic Token Refresh**: 5-minute buffer prevents mid-request expiration

---

## Testing Checklist

After fixing critical issues, verify:

- [x] OAuth state validation rejects mismatched state ✅ **DONE** - Implemented in oidc.py:198-207
- [ ] Token file created with 0600 permissions (check with `ls -la`)
- [ ] Parent directory created with 0700 permissions
- [ ] Multiple concurrent auth attempts don't leak ports
- [ ] JWT validation fails for invalid issuer
- [ ] JWT validation fails for invalid audience
- [ ] JWT validation fails for expired tokens
- [ ] API key validation uses constant-time comparison (if implemented)
- [ ] Rate limiting blocks excessive failed attempts (if implemented)

---

## References

- [OAuth 2.0 Security Best Current Practice](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-security-topics)
- [RFC 7636: Proof Key for Code Exchange (PKCE)](https://datatracker.ietf.org/doc/html/rfc7636)
- [RFC 7519: JSON Web Token (JWT)](https://datatracker.ietf.org/doc/html/rfc7519)
- [OpenID Connect Core 1.0](https://openid.net/specs/openid-connect-core-1_0.html)
- [OWASP Authentication Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Authentication_Cheat_Sheet.html)

---

## Test Infrastructure Improvements

**Date**: 2025-10-27

Fixed pre-existing issues in Python unit tests (`tests/auth/test_oidc_unit.py`):

**Problem**: 5 unit tests were failing with network timeout errors because `OidcAuthProvider.__init__()` makes a real HTTP request to fetch OIDC discovery metadata via `requests.get()`. Tests only mocked `OAuth2Session` but not the network call.

**Solution**: Added proper mocking for `requests.get()` in all affected tests:
- `test_oidc_token_save_and_load`
- `test_oidc_get_token_valid`
- `test_oidc_get_token_needs_refresh`
- `test_oidc_get_token_no_token`
- `test_oidc_thread_safety`

**Result**: All 6 unit tests now pass in 0.49s without network calls.

---

## Timeline

**Before Merge (Critical)**: ~~Issues 1-3~~ Issue 1 ✅ Complete, Issues 2-3 remain
**Within 1 week**: Issues 4-5
**Within 1 month**: Issues 6-8
**Future**: Issues 9-11

---

Last Updated: 2025-10-27
Reviewer: Claude Code Security Review
