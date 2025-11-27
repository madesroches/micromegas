# Auth Token Expiration and Multi-Provider OIDC Support

## TL;DR - What Was Accomplished

✅ **Phase 1-3: COMPLETED**
- Auth0 support with custom claims and audience parameter
- Access token support (24h lifetime) + ID token backward compatibility
- Multi-audience support (same issuer, multiple audiences)
- Audience field in auth logs
- Scope parameter support for all providers
- Azure AD configuration with ID tokens (~1.5h lifetime, auto-refresh)

✅ **Key Features**:
- Works with Auth0, Azure AD, Google
- Automatic token refresh (transparent to users)
- No breaking changes for existing deployments
- Email extraction from multiple claim formats

✅ **Security Features**:
- Constant-time API key comparison (timing attack protection)
- SSRF protection in OIDC client (no redirects)
- Auth header sanitization (prevents spoofing)
- PKCE + state validation in OAuth flow (CSRF protection)
- Secure token file permissions (0600)
- API keys cannot be admins (principle of least privilege)

## Final Implementation

### Auth Providers Supported

1. **Auth0** (`~/set_human_auth.sh`)
   - Uses access tokens (24 hour lifetime)
   - Custom claims via Auth0 Actions
   - Audience: `https://api.micromegas.example.com`

2. **Azure AD** (`~/set_azure_auth.sh`)
   - Uses ID tokens (~1.5 hour lifetime)
   - Standard OpenID scopes to avoid Conditional Access
   - Automatic refresh with `offline_access`
   - Audience: client_id

3. **Google** (standard OIDC)
   - Uses ID tokens
   - Standard email claim
   - Works out of the box

### Code Changes Summary

**Rust** (`rust/auth/`):
- `types.rs`: Added `audience: Option<String>` and `allow_delegation: bool` to AuthContext
- `oidc.rs`: Multi-audience support, Auth0 namespaced claims, verified_primary_email, SSRF protection (no redirects)
- `tower.rs`: Log audience in auth messages, sanitize client-provided auth headers
- `api_key.rs`: Set audience=None for API keys, constant-time comparison for timing attack protection

**Python** (`python/micromegas/micromegas/`):
- `auth/oidc.py`: Scope parameter, ID token preference, audience support, thread-safe refresh, JWT validation (alg=none check)
- `oidc_connection.py`: Scope and audience pass-through
- `cli/connection.py`: Environment variable support for scope/audience (MICROMEGAS_OIDC_SCOPE)

**Scripts**:
- `test_oidc_auth.py`: Added scope support, manual browser control
- `start_services_with_oidc.py`: Simplified to use MICROMEGAS_OIDC_CONFIG only

**Configuration**:
- `~/set_human_auth.sh`: Auth0 with access tokens
- `~/set_azure_auth.sh`: Azure AD with ID tokens, standard scopes

### Environment Variables

**Server (Rust)**:
```bash
MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {"issuer": "...", "audience": "..."}
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

**Client (Python)**:
```bash
MICROMEGAS_OIDC_ISSUER=https://...
MICROMEGAS_OIDC_CLIENT_ID=...
MICROMEGAS_OIDC_AUDIENCE=...
MICROMEGAS_OIDC_SCOPE="openid email profile offline_access"  # Optional
```

## Azure AD Notes

### Why ID Tokens Instead of Access Tokens?

Corporate Conditional Access policies block custom API access tokens when:
- Accessing from non-managed devices
- Requesting custom API scopes (`api://.../execute_queries_as_user`)

**Solution**: Use standard OpenID scopes (`openid email profile offline_access`)
- Gets ID tokens with ~1.5 hour lifetime
- Includes email claims (`email`, `preferred_username`)
- Auto-refreshes with refresh token (transparent to users)
- No Conditional Access restrictions

### Attempted Custom API Approach (Blocked)

Tried to expose custom API in Azure Portal to get access tokens:
1. ❌ Added scope `execute_queries_as_user`
2. ❌ Configured optional claims (`verified_primary_email`, `email`)
3. ❌ Requested scope `api://793e1ccf-8d7c-4887-be6e-571c0c408e5c/execute_queries_as_user`

**Result**: Conditional Access policy blocked authentication from home/unmanaged devices.

**Alternative**: Would work with:
- Managed corporate devices
- Specific browser extensions
- Microsoft Edge on Windows
- IT exemption for the app

## Testing

### Auth0
```bash
. ~/set_human_auth.sh
python3 test_oidc_auth.py
```

### Azure AD
```bash
. ~/set_azure_auth.sh
python3 test_oidc_auth.py
```

### Token Refresh
```bash
poetry run python3 -c "
from micromegas.auth import OidcAuthProvider
from pathlib import Path
auth = OidcAuthProvider.from_file(str(Path.home() / '.micromegas' / 'tokens.json'))
auth._refresh_tokens()
print('✅ Refresh successful')
"
```

## Future Work (Deferred)

1. **Server-side token cache expiration check** - Not critical with client-side refresh
2. **Access tokens for Azure AD** - Blocked by Conditional Access, would need IT approval
3. **Multi-tenant support** - Would require IT configuration changes

## Files Modified

### Rust
- `rust/auth/src/types.rs` - AuthContext.audience + allow_delegation fields
- `rust/auth/src/oidc.rs` - Multi-audience, namespaced claims, verified_primary_email, SSRF protection
- `rust/auth/src/tower.rs` - Audience in auth logs, auth header sanitization
- `rust/auth/src/api_key.rs` - audience=None, constant-time comparison

### Python
- `python/micromegas/micromegas/auth/oidc.py` - Scope, ID token preference, thread-safe refresh, alg=none check
- `python/micromegas/micromegas/oidc_connection.py` - Scope/audience parameters
- `python/micromegas/cli/connection.py` - MICROMEGAS_OIDC_SCOPE env var support

### Scripts
- `local_test_env/ai_scripts/test_oidc_auth.py` - Scope/audience support, audience mismatch detection
- `local_test_env/ai_scripts/start_services_with_oidc.py` - Simplified to use MICROMEGAS_OIDC_CONFIG only

### Configuration (User home directory)
- `~/set_human_auth.sh` - Auth0 config (access tokens)
- `~/set_azure_auth.sh` - Azure AD config (ID tokens, standard scopes)
