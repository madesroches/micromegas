# Auth0 OIDC Testing Guide (Public Client - No Secret)

This guide shows how to test OIDC authentication with Auth0 as a public client (no client secret required).

## Why Auth0?

- ✅ **True public client support** (no client secret with PKCE)
- ✅ **Free tier** (up to 7,000 active users)
- ✅ **Easy setup** (5 minutes)
- ✅ **Standard OIDC** implementation
- ✅ **Good for testing** alternative IDPs

## Setup Steps

### 1. Create Auth0 Account

1. Go to https://auth0.com/signup
2. Sign up for a free account
3. Create a tenant (e.g., `yourname-dev.auth0.com`)

### 2. Create Native Application (Public Client)

1. In Auth0 Dashboard → Applications → Applications
2. Click **Create Application**
3. Name: `Micromegas Test`
4. Type: **Native** (this creates a public client)
5. Click **Create**

### 3. Configure Application

In the application **Settings** tab:

**Basic Information:**
- Note your **Client ID** (you'll need this)
- Note there's **NO Client Secret** shown (public client!)

**Application URIs:**
- **Allowed Callback URLs**: `http://localhost:48080/callback`
- **Allowed Logout URLs**: `http://localhost:48080`
- **Allowed Web Origins**: `http://localhost:48080`

**Advanced Settings → Grant Types:**
- ✅ Authorization Code
- ✅ Refresh Token

Click **Save Changes**

### 4. Get Configuration Values

You need two values:

1. **Issuer URL**: `https://YOUR-TENANT.auth0.com/`
   - Replace `YOUR-TENANT` with your tenant name
   - **Include the trailing slash!**

2. **Client ID**: Found in application Settings
   - Example: `abc123XYZ789def456GHI012jkl345`

## Testing

### Configure Server

The server needs to be configured to accept Auth0 tokens:

```bash
# Your Auth0 tenant
export AUTH0_DOMAIN="yourname-dev.auth0.com"

# Configure server to accept Auth0 tokens
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://'$AUTH0_DOMAIN'/",
      "audience": "YOUR-CLIENT-ID"
    }
  ]
}'
```

**Note:** For Auth0, the `audience` should be your Client ID.

### Start Services

```bash
cd /home/mad/micromegas
python3 local_test_env/ai_scripts/start_services_with_oidc.py
```

### Test Python Client

```bash
# Set Auth0 configuration
export OIDC_ISSUER="https://yourname-dev.auth0.com/"
export OIDC_CLIENT_ID="YOUR-CLIENT-ID"
# No OIDC_CLIENT_SECRET needed - public client!

# Run test
python3 local_test_env/ai_scripts/test_oidc_auth.py
```

This will:
1. Open browser for Auth0 login
2. You'll see Auth0's login page (not Google's)
3. Create an account or login
4. Save tokens to `~/.micromegas/tokens.json`
5. Test FlightSQL query with Auth0 authentication

### Verify It Worked

Check the server logs to see Auth0 authentication:

```bash
tail -f /tmp/analytics.log | grep -i auth
```

You should see:
- Token validation logs
- Your Auth0 user ID (sub claim)
- No client secret mentioned (it's a public client!)

## Differences from Google OAuth

| Feature | Google OAuth | Auth0 Native App |
|---------|--------------|------------------|
| **Client Type** | Desktop app (but requires secret) | Native app (true public) |
| **Client Secret** | Required (even for desktop) | NOT required |
| **PKCE** | Used with secret | Used alone |
| **Setup Complexity** | More steps (API enable, consent screen) | Simpler |
| **Issuer Format** | `https://accounts.google.com` | `https://tenant.auth0.com/` |
| **Audience** | Client ID | Client ID |

## Quick Test Script

```bash
#!/bin/bash
# test_auth0.sh

# Auth0 configuration
export AUTH0_DOMAIN="yourname-dev.auth0.com"
export AUTH0_CLIENT_ID="your-client-id"

# Configure OIDC
export OIDC_ISSUER="https://${AUTH0_DOMAIN}/"
export OIDC_CLIENT_ID="${AUTH0_CLIENT_ID}"

# Configure server
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://'${AUTH0_DOMAIN}'/",
      "audience": "'${AUTH0_CLIENT_ID}'"
    }
  ]
}'

# Start services
cd /home/mad/micromegas
python3 local_test_env/ai_scripts/start_services_with_oidc.py

# Wait for services to start
sleep 3

# Test authentication
python3 local_test_env/ai_scripts/test_oidc_auth.py
```

## Troubleshooting

### "Callback URL mismatch" error

Make sure you added `http://localhost:48080/callback` to **Allowed Callback URLs** in Auth0 application settings.

### "Invalid issuer" error on server

Make sure the issuer in `MICROMEGAS_OIDC_CONFIG` matches exactly, including trailing slash:
- ✅ `https://yourname-dev.auth0.com/`
- ❌ `https://yourname-dev.auth0.com`

### "Invalid audience" error on server

Auth0 uses the Client ID as the audience. Make sure the `audience` field in server config matches your Client ID exactly.

### Browser doesn't open

The script should open your browser automatically. If not:
1. Check the console for the authorization URL
2. Copy and paste it into your browser manually

## Expected Results

When testing with Auth0, you should see:

1. ✅ Browser opens to Auth0's login page (not Google)
2. ✅ You can create a new Auth0 account or use existing
3. ✅ Token is saved without any client secret
4. ✅ Server validates Auth0 tokens successfully
5. ✅ FlightSQL queries work with Auth0 authentication
6. ✅ Second run uses saved tokens (no browser)

## Next Steps

After confirming Auth0 works:

1. Test with another provider (Azure AD, Okta) to validate multi-provider support
2. Test token refresh after ~1 hour
3. Update main documentation with Auth0 as a recommended option
4. Consider documenting Auth0 as the easiest option for development/testing

## References

- [Auth0 Native Apps](https://auth0.com/docs/get-started/applications/application-types#native-applications)
- [Auth0 PKCE Flow](https://auth0.com/docs/get-started/authentication-and-authorization-flow/authorization-code-flow-with-proof-key-for-code-exchange-pkce)
- [Auth0 Quickstarts](https://auth0.com/docs/quickstarts)
