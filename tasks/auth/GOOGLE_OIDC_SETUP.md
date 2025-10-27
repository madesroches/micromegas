# OIDC Authentication Setup Guide (Using Google as Example)

This guide walks you through setting up OIDC authentication for Micromegas using **Google as an example**.

**Important**: The Micromegas OIDC implementation is **completely provider-agnostic** and works with:
- ✅ Google OAuth
- ✅ Azure Active Directory
- ✅ Okta
- ✅ Auth0
- ✅ Keycloak
- ✅ Any standards-compliant OIDC provider

This guide uses Google because it's easy to set up for testing, but the same principles apply to all providers.

## Prerequisites

- A Google account
- Access to [Google Cloud Console](https://console.cloud.google.com/)
- Micromegas services built (`cargo build` in rust/ directory)

## Step 1: Create Google OAuth Client

### 1.1 Create or Select a Google Cloud Project

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project or select an existing one
   - Click the project dropdown at the top
   - Click "New Project"
   - Name it (e.g., "Micromegas OIDC Test")
   - Click "Create"

### 1.2 Configure OAuth Consent Screen

1. Navigate to "APIs & Services" → "OAuth consent screen"
2. Select "External" user type (for testing with any Google account)
3. Click "Create"
4. Fill in the required fields:
   - **App name**: "Micromegas Local Test"
   - **User support email**: Your email
   - **Developer contact information**: Your email
5. Click "Save and Continue"
6. Skip "Scopes" (click "Save and Continue")
7. Add test users:
   - Click "Add Users"
   - Add your Google email address
   - Click "Save and Continue"
8. Click "Back to Dashboard"

### 1.3 Create OAuth Client Credentials

**Choose the appropriate client type for your use case:**

#### Option A: Desktop App (for CLI tools, local scripts)

1. Navigate to "APIs & Services" → "Credentials"
2. Click "+ CREATE CREDENTIALS" → "OAuth client ID"
3. Select **"Desktop app"** as the application type
4. Configure:
   - **Name**: "Micromegas CLI Client"
   - **Note**: Desktop apps don't require redirect URIs
5. Click "Create"
6. **IMPORTANT**: Copy **both** credentials:
   - **Client ID** (ends with `.apps.googleusercontent.com`)
   - **Client Secret** (Google requires this even for Desktop apps with PKCE!)
7. Click "OK"

**⚠️ Google-Specific Requirement**:
- Unlike other providers, Google requires client_secret even for Desktop apps using PKCE
- The secret is considered "public" (safe to distribute with your application)
- PKCE provides the actual security, not the client_secret
- This is a known deviation from OAuth 2.0 RFC 7636

**Use Desktop app when:**
- Running from CLI/terminal
- Local Python scripts
- Jupyter notebooks on your machine
- Any scenario where you can't securely store a secret

#### Option B: Web Application (for web apps, server-side)

1. Navigate to "APIs & Services" → "Credentials"
2. Click "+ CREATE CREDENTIALS" → "OAuth client ID"
3. Select **"Web application"** as the application type
4. Configure:
   - **Name**: "Micromegas Web App"
   - **Authorized redirect URIs**: Add your callback URL
     - Local dev: `http://localhost:48080/callback`
     - Production: `https://your-domain.com/auth/callback`
5. Click "Create"
6. **IMPORTANT**: Save both credentials:
   - **Client ID** (ends with `.apps.googleusercontent.com`)
   - **Client Secret** - Store securely (environment variable, secret manager)
7. Click "OK"

**Use Web application when:**
- Building a web application
- Client secret can be stored securely on server
- Users authenticate through your web app
- Need more control over redirect URIs

## Step 2: Set Environment Variables

Export your OAuth credentials:

```bash
# Required
export OIDC_CLIENT_ID="YOUR_CLIENT_ID.apps.googleusercontent.com"
export OIDC_ISSUER="https://accounts.google.com"

# Required for Google (even Desktop apps!)
export OIDC_CLIENT_SECRET="YOUR_CLIENT_SECRET"
```

### ⚠️ Important: Google's PKCE Implementation

**Google requires `client_secret` even for Desktop apps**, which deviates from the OAuth 2.0 standard (RFC 7636).

- ✅ **PKCE provides the real security** through code_challenge/code_verifier
- ⚠️ **client_secret is required** but is **not treated as a secret** by Google
- 📦 **It's safe to distribute** with CLI tools and Desktop applications
- 🔒 **Never commit it** to version control, but it's okay to ship with your app

**Why this matters:**
- RFC 7636 designed PKCE for public clients that **cannot securely store secrets**
- Most providers (Azure AD, Okta, Auth0) support truly secret-less PKCE
- Google deviates from this standard and still requires the client_secret parameter
- Google's own documentation states: "the client secret is obviously not treated as a secret" for Desktop apps

**From Google's documentation:**
> "In this context, the client secret is obviously not treated as a secret."

This is a known Google-specific quirk. See:
- [Stack Overflow: Google OAuth 2.0 with PKCE requires client secret](https://stackoverflow.com/questions/76528208)
- [Google Issue: Client secrets in desktop open-source apps](https://github.com/googleapis/google-auth-library-nodejs/issues/959)

## Step 3: Configure Micromegas Server

Create a configuration file with your Google OAuth settings:

```bash
# Create OIDC config for the server
cat > /tmp/oidc_config.json << EOF
{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "${OIDC_CLIENT_ID}"
    }
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}
EOF

# Export as environment variable
export MICROMEGAS_OIDC_CONFIG=$(cat /tmp/oidc_config.json)

# Optional: Set admin users (use your Google email)
export MICROMEGAS_ADMINS='["your-email@gmail.com"]'
```

## Step 3: Start Services with OIDC Authentication

### Option A: Use the provided script

```bash
# From the micromegas root directory
cd local_test_env/ai_scripts
python3 start_services_with_oidc.py
```

### Option B: Manual startup

```bash
cd rust/

# Build services
cargo build

# Terminal 1: Start ingestion server (no auth)
cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000

# Terminal 2: Start analytics server WITH OIDC auth
export MICROMEGAS_OIDC_CONFIG='...'  # From Step 2
cargo run -p flight-sql-srv
```

## Step 4: Test Python Client Authentication

### 4.1 Install Python dependencies

```bash
cd python/micromegas
poetry install
```

### 4.2 Test browser-based login

```bash
# Set environment variables
export OIDC_CLIENT_ID="YOUR_CLIENT_ID.apps.googleusercontent.com"
export OIDC_ISSUER="https://accounts.google.com"
# Optional: export OIDC_CLIENT_SECRET="YOUR_CLIENT_SECRET"

# Run the test script
python3 ../../local_test_env/ai_scripts/test_oidc_auth.py
```

This will:
1. Open your browser for Google authentication
2. Save tokens to `~/.micromegas/tokens.json`
3. Test a FlightSQL query with the authenticated client
4. Test automatic token refresh

## Step 5: Verify Authentication

### 5.1 Check server logs

The flight-sql-srv should log authentication events:

```bash
tail -f /tmp/analytics.log
```

Look for:
- OIDC token validation messages
- User identity information (email, subject)
- Audit log entries

### 5.2 Check saved tokens

```bash
# View saved tokens (with secure permissions)
ls -la ~/.micromegas/tokens.json
# Should show: -rw------- (0600 permissions)

# View token contents (redacted)
cat ~/.micromegas/tokens.json | jq '.issuer, .client_id'
```

### 5.3 Test token reuse

Run the test script again - it should NOT open the browser:

```bash
python3 ../../local_test_env/ai_scripts/test_oidc_auth.py
```

The second run should use cached tokens and complete immediately.

## Troubleshooting

### Browser doesn't open

- Check if port 48080 is available: `lsof -i :48080`
- Try a different redirect URI and update both Google Console and the script

### "Token validation failed"

- Verify `MICROMEGAS_OIDC_CONFIG` is set correctly
- Check that the `audience` matches your Client ID exactly
- Check server logs: `tail -f /tmp/analytics.log`

### "Discovery failed"

- Check internet connectivity
- Verify issuer URL is correct: `https://accounts.google.com`
- Test discovery endpoint manually:
  ```bash
  curl https://accounts.google.com/.well-known/openid-configuration
  ```

### "Invalid redirect_uri"

- Make sure `http://localhost:48080/callback` is added to Google Console
- Check for typos (http vs https, port number)

### Token refresh fails

- Check if refresh token is present: `cat ~/.micromegas/tokens.json | jq '.token.refresh_token'`
- Delete tokens and re-authenticate: `rm ~/.micromegas/tokens.json`

## Security Notes

1. **Client ID is public** - it's safe to commit to version control
2. **Never commit tokens** - `tokens.json` should be in `.gitignore`
3. **Token file permissions** - automatically set to 0600 (owner read/write only)
4. **Redirect URI** - only use `localhost` for development
5. **Production** - use HTTPS redirect URIs and proper OAuth consent

## Next Steps

Once testing is complete:

1. Update CLI to support OIDC (Phase 3 in the plan)
2. Add integration tests with mock OIDC provider
3. Write end-user documentation
4. Test with other providers (Azure AD, Okta)

## Reference

- Google OIDC documentation: https://developers.google.com/identity/protocols/oauth2/openid-connect
- OAuth 2.0 Playground: https://developers.google.com/oauthplayground/
- OIDC Discovery: https://accounts.google.com/.well-known/openid-configuration
