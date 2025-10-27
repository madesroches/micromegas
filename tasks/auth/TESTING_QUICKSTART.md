# OIDC Authentication Testing - Quick Start Guide

This guide helps you quickly set up and test OIDC authentication with Google.

## Prerequisites

- Google account
- Micromegas services built (`cd rust && cargo build`)
- Python environment with Poetry (`cd python/micromegas && poetry install`)

## Step 1: Create Google OAuth Client (5 minutes)

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project (or select existing)
3. Navigate to: **APIs & Services → OAuth consent screen**
   - Select "External" user type
   - Fill in app name, emails
   - Add yourself as test user
4. Navigate to: **APIs & Services → Credentials**
   - Click "+ CREATE CREDENTIALS" → "OAuth client ID"
   - **Type: "Desktop app"** (for CLI/local testing)
   - Click "Create"
5. **Copy BOTH credentials**:
   - **Client ID** (ends with `.apps.googleusercontent.com`)
   - **Client Secret** (Google requires this even for Desktop apps - see note below)

**Detailed instructions**: See [GOOGLE_OIDC_SETUP.md](GOOGLE_OIDC_SETUP.md)

## Step 2: Set Environment Variables

**Note**: The implementation is **provider-agnostic** and works with any OIDC provider (Google, Azure AD, Okta, Auth0, etc.).

### Required Variables

```bash
# Required for all setups
export OIDC_CLIENT_ID="YOUR_CLIENT_ID"
export OIDC_ISSUER="https://accounts.google.com"  # Or your provider's issuer URL

# Optional: Set yourself as admin
export MICROMEGAS_ADMINS='["your-email@example.com"]'
```

### Client Secret (Provider-Dependent)

**⚠️ Google Requires It**: Google requires `OIDC_CLIENT_SECRET` even for Desktop apps (a deviation from RFC 7636).

```bash
# Required for Google (even Desktop apps)
export OIDC_CLIENT_SECRET="YOUR_CLIENT_SECRET"
```

**Provider Differences:**
- 🔴 **Google**: Requires client_secret even for Desktop apps with PKCE
  - The secret is "public" (safe to distribute with CLI tools)
  - PKCE provides the real security, not the client_secret
  - Known deviation from OAuth 2.0 standard
- 🟢 **Other providers** (Azure AD, Okta, Auth0): May support truly secret-less PKCE per RFC 7636
  - To be tested and documented

**Examples for different providers:**

```bash
# Google (Desktop app - requires secret even with PKCE!)
export OIDC_CLIENT_ID="123-abc.apps.googleusercontent.com"
export OIDC_CLIENT_SECRET="GOCSPX-..."  # Required by Google
export OIDC_ISSUER="https://accounts.google.com"

# Azure AD (secret-less PKCE - to be tested)
export OIDC_CLIENT_ID="<your-application-id>"
export OIDC_ISSUER="https://login.microsoftonline.com/<tenant-id>/v2.0"
# export OIDC_CLIENT_SECRET="<secret>"  # May not be needed

# Okta (secret-less PKCE - to be tested)
export OIDC_CLIENT_ID="<your-client-id>"
export OIDC_ISSUER="https://<your-domain>.okta.com"
# export OIDC_CLIENT_SECRET="<secret>"  # May not be needed
```

## Step 3: Start Services with OIDC

```bash
cd local_test_env/ai_scripts
python3 start_services_with_oidc.py
```

This will:
- Build services if needed
- Start PostgreSQL (if not running)
- Start ingestion server (no auth)
- Start analytics server **with OIDC auth enabled**
- Start admin daemon

**Check logs**: `tail -f /tmp/analytics.log`

## Step 4: Run Tests

### Option A: Manual Interactive Test

```bash
cd local_test_env/ai_scripts
python3 test_oidc_auth.py
```

This will:
1. Open browser for Google login (first run only)
2. Save tokens to `~/.micromegas/tokens.json`
3. Run a test query
4. Show token information

**Second run**: Uses cached tokens, no browser opens.

### Option B: Automated Integration Tests

```bash
cd python/micromegas
poetry run pytest tests/auth/test_oidc_integration.py -v
```

This runs comprehensive tests:
- Authentication flow
- Token persistence and permissions
- Authenticated queries
- Token refresh logic
- Concurrent queries
- Token reuse across instances

**Note**: Browser opens on first run, then tokens are cached.

## Step 5: Verify Everything Works

### Check authentication succeeded

```bash
# View saved tokens (secure permissions)
ls -la ~/.micromegas/tokens.json
# Should show: -rw------- (0600)

# View token metadata
cat ~/.micromegas/tokens.json | jq '.issuer, .client_id'
```

### Check server logs for auth events

```bash
tail -f /tmp/analytics.log | grep -i oidc
```

Look for:
- "OIDC authentication successful"
- User email in audit logs
- No validation errors

### Test token reuse

```bash
# Run test again - should NOT open browser
python3 local_test_env/ai_scripts/test_oidc_auth.py
```

## Troubleshooting

### "OIDC_CLIENT_ID not set"
```bash
export OIDC_CLIENT_ID="your-client-id.apps.googleusercontent.com"
export OIDC_ISSUER="https://accounts.google.com"
```

### "Analytics server not running"
```bash
# Check if running
lsof -i :32010

# Start services
cd local_test_env/ai_scripts
python3 start_services_with_oidc.py
```

### "Token validation failed"
- Check server logs: `tail -f /tmp/analytics.log`
- Verify Client ID matches in both places:
  - Google Console
  - MICROMEGAS_OIDC_CONFIG environment variable
- Check OIDC config is set:
  ```bash
  echo $MICROMEGAS_OIDC_CONFIG | jq .
  ```

### "Invalid redirect_uri"
- Make sure `http://localhost:48080/callback` is in Google Console
- Check for typos (http vs https, port number)

### Clear tokens and re-authenticate
```bash
rm ~/.micromegas/tokens.json
python3 test_oidc_auth.py
```

## What's Being Tested

### Server-Side (Rust)
- ✅ OIDC discovery (/.well-known/openid-configuration)
- ✅ JWKS fetching and caching
- ✅ JWT signature verification
- ✅ Token validation (issuer, audience, expiration)
- ✅ User identity extraction (email, subject)
- ✅ Audit logging with user context
- ✅ Multi-provider support

### Client-Side (Python)
- ✅ Browser-based login with PKCE
- ✅ Token persistence to file
- ✅ Secure file permissions (0600)
- ✅ Automatic token refresh (5-min buffer)
- ✅ Thread-safe concurrent queries
- ✅ Token reuse across sessions
- ✅ FlightSQL integration

## Success Checklist

- [ ] Created Google OAuth Client
- [ ] Set GOOGLE_CLIENT_ID environment variable
- [ ] Started services with OIDC enabled
- [ ] Ran manual test (browser opened, tokens saved)
- [ ] Ran test again (no browser, used cached tokens)
- [ ] Ran integration tests (all passed)
- [ ] Checked token file permissions (0600)
- [ ] Checked server logs (auth events visible)

## Next Steps

Once basic testing works:

1. **Test with your application**
   - Use the Python client with `auth_provider=OidcAuthProvider.from_file()`
   - Run queries from Jupyter notebooks
   - Try CLI tools (Phase 3 - coming soon)

2. **Test token refresh**
   - Wait ~1 hour for token to approach expiration
   - Run queries - should auto-refresh
   - Check logs for refresh events

3. **Test with multiple users**
   - Add more test users in Google Console
   - Authenticate from different accounts
   - Verify user identity in server logs

4. **Performance testing**
   - Run many concurrent queries
   - Check JWKS cache hit rate in logs
   - Verify token validation is fast (<10ms)

5. **Security testing**
   - Verify expired tokens are rejected
   - Try invalid tokens (should fail)
   - Check file permissions are secure
   - Verify tokens not logged

## Files Created

```
tasks/auth/
├── GOOGLE_OIDC_SETUP.md          # Detailed setup guide
├── TESTING_QUICKSTART.md         # This file
└── oidc_auth_subplan.md          # Implementation plan

local_test_env/ai_scripts/
├── start_services_with_oidc.py   # Start services with OIDC
└── test_oidc_auth.py             # Manual interactive test

python/micromegas/tests/auth/
└── test_oidc_integration.py      # Automated integration tests
```

## Reference

- **Full setup guide**: [GOOGLE_OIDC_SETUP.md](GOOGLE_OIDC_SETUP.md)
- **Implementation plan**: [oidc_auth_subplan.md](oidc_auth_subplan.md)
- **Google OIDC docs**: https://developers.google.com/identity/protocols/oauth2/openid-connect
- **OIDC Discovery**: https://accounts.google.com/.well-known/openid-configuration
