# Using OIDC with Other Identity Providers

The Micromegas OIDC implementation is **completely provider-agnostic**. While the setup guide uses Google as an example, you can use **any standards-compliant OIDC provider**.

## Important: Provider-Specific Behaviors

### Client Secret Requirements

**⚠️ Google's Quirk**: Google requires `client_secret` even for Desktop/CLI apps using PKCE, which deviates from RFC 7636.

**🟢 Other Providers**: Many providers (Azure AD, Okta, Auth0, Keycloak) support truly secret-less PKCE for public clients per the OAuth 2.0 standard.

**Testing Status:**
- ✅ **Google**: Tested - requires client_secret (even for Desktop apps)
- ⏳ **Azure AD, Okta, Auth0, Keycloak**: Not yet tested - may support secret-less PKCE

### PKCE and Client Secrets

According to **RFC 7636** (OAuth 2.0 PKCE standard):
- PKCE was designed for **public clients** that cannot securely store secrets
- Public clients (CLI tools, Desktop apps) should **not require** client_secret when using PKCE
- PKCE provides security through code_challenge/code_verifier, not client authentication

**For the examples below**: Client secrets are shown but may not be required for Desktop/CLI apps depending on the provider.

## Quick Reference

All providers require these configuration values:

1. **Issuer URL** - The OIDC provider's base URL (from discovery document)
2. **Client ID** - Your application's identifier
3. **Client Secret** - Your application's secret (required by Google, may be optional for others)

## Azure Active Directory (Azure AD)

### Setup

1. Go to [Azure Portal](https://portal.azure.com/)
2. Navigate to "Azure Active Directory" → "App registrations"
3. Click "+ New registration"
4. Configure:
   - **Name**: "Micromegas Analytics"
   - **Supported account types**: Choose based on your needs
   - **Redirect URI**: Web → `http://localhost:48080/callback` (for local dev)
5. Click "Register"
6. Save the **Application (client) ID**
7. Go to "Certificates & secrets" → "+ New client secret"
8. Save the **secret value**
9. Note your **Tenant ID** (from Overview page)

### Environment Variables

```bash
# Azure AD configuration
export OIDC_CLIENT_ID="<your-application-id>"
export OIDC_CLIENT_SECRET="<your-client-secret>"
export OIDC_ISSUER="https://login.microsoftonline.com/<tenant-id>/v2.0"
```

### Server Configuration

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://login.microsoftonline.com/<tenant-id>/v2.0",
    "audience": "<your-application-id>"
  }],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

## Okta

### Setup

1. Go to [Okta Admin Console](https://your-domain-admin.okta.com/)
2. Navigate to "Applications" → "Applications"
3. Click "Create App Integration"
4. Select "OIDC - OpenID Connect"
5. Select "Web Application" or "Native Application"
6. Configure:
   - **App integration name**: "Micromegas Analytics"
   - **Sign-in redirect URIs**: `http://localhost:48080/callback`
7. Save the **Client ID** and **Client Secret**
8. Note your **Okta domain** (e.g., `dev-12345.okta.com`)

### Environment Variables

```bash
# Okta configuration
export OIDC_CLIENT_ID="<your-client-id>"
export OIDC_CLIENT_SECRET="<your-client-secret>"
export OIDC_ISSUER="https://<your-domain>.okta.com"
```

### Server Configuration

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://<your-domain>.okta.com",
    "audience": "<your-client-id>"
  }],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

## Auth0

### Setup

1. Go to [Auth0 Dashboard](https://manage.auth0.com/)
2. Navigate to "Applications" → "Applications"
3. Click "+ Create Application"
4. Configure:
   - **Name**: "Micromegas Analytics"
   - **Type**: "Regular Web Application" or "Native"
5. Save the **Client ID** and **Client Secret**
6. Go to "Settings" → "Allowed Callback URLs"
7. Add: `http://localhost:48080/callback`
8. Note your **Domain** (e.g., `dev-xyz.us.auth0.com`)

### Environment Variables

```bash
# Auth0 configuration
export OIDC_CLIENT_ID="<your-client-id>"
export OIDC_CLIENT_SECRET="<your-client-secret>"
export OIDC_ISSUER="https://<your-domain>.auth0.com/"  # Note trailing slash
```

### Server Configuration

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://<your-domain>.auth0.com/",
    "audience": "<your-client-id>"
  }],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

## Keycloak

### Setup

1. Log in to your Keycloak Admin Console
2. Select your realm (or create a new one)
3. Navigate to "Clients" → "Create client"
4. Configure:
   - **Client ID**: "micromegas-analytics"
   - **Client Protocol**: "openid-connect"
5. Enable "Client authentication"
6. Add redirect URI: `http://localhost:48080/callback`
7. Save the **Client ID** and generate a **Client Secret**
8. Note your Keycloak URL and realm

### Environment Variables

```bash
# Keycloak configuration
export OIDC_CLIENT_ID="micromegas-analytics"
export OIDC_CLIENT_SECRET="<your-client-secret>"
export OIDC_ISSUER="https://<keycloak-server>/realms/<realm-name>"
```

### Server Configuration

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "https://<keycloak-server>/realms/<realm-name>",
    "audience": "micromegas-analytics"
  }],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

## Generic OIDC Provider

For any OIDC-compliant provider:

### 1. Find the Discovery Document

All OIDC providers must have a discovery document at:
```
<issuer-url>/.well-known/openid-configuration
```

For example:
```bash
curl https://accounts.google.com/.well-known/openid-configuration
```

### 2. Extract Required Information

From the discovery document, note:
- **issuer** - The issuer URL (use this for `OIDC_ISSUER`)
- **authorization_endpoint** - Where users authenticate
- **token_endpoint** - Where tokens are exchanged
- **jwks_uri** - Where public keys are published

### 3. Create OAuth Client

In your provider's console:
1. Create an OAuth 2.0 / OIDC client
2. Set redirect URI to: `http://localhost:48080/callback` (or your production URL)
3. Choose "Web application" or "Desktop/Native" client type
4. Save the **Client ID** and **Client Secret**

### 4. Configure Micromegas

```bash
# Generic OIDC configuration
export OIDC_CLIENT_ID="<your-client-id>"
export OIDC_CLIENT_SECRET="<your-client-secret>"
export OIDC_ISSUER="<issuer-from-discovery-doc>"

# Server configuration
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [{
    "issuer": "<issuer-from-discovery-doc>",
    "audience": "<your-client-id>"
  }]
}'
```

## Multi-Provider Setup

You can configure **multiple identity providers simultaneously**:

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "google-client-id.apps.googleusercontent.com"
    },
    {
      "issuer": "https://login.microsoftonline.com/tenant-id/v2.0",
      "audience": "azure-application-id"
    },
    {
      "issuer": "https://your-domain.okta.com",
      "audience": "okta-client-id"
    }
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

Users can then authenticate with **any** of the configured providers.

## Testing with Python Client

The Python client works identically regardless of provider:

```python
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient

# Works with ANY provider - just change the issuer URL
auth = OidcAuthProvider.login(
    issuer="<your-provider-issuer-url>",
    client_id="<your-client-id>",
    client_secret="<your-client-secret>",
)

client = FlightSQLClient("grpc://localhost:50051", auth_provider=auth)
```

## Common Issues

### Wrong Issuer Format

**Problem**: Token validation fails with "invalid issuer"

**Solution**: Use the **exact** issuer string from the discovery document. Common mistakes:
- Missing or extra trailing slash
- Wrong protocol (http vs https)
- Missing version suffix (e.g., `/v2.0` for Azure)

### Audience Mismatch

**Problem**: Token validation fails with "invalid audience"

**Solution**: The `audience` in server config must match:
- Google: Client ID
- Azure: Application ID (or API identifier if using API scopes)
- Okta: Client ID
- Auth0: Client ID (or API identifier)

### Discovery Fails

**Problem**: "Failed to discover OIDC metadata"

**Solution**:
1. Verify issuer URL is reachable: `curl <issuer>/.well-known/openid-configuration`
2. Check for typos in issuer URL
3. Ensure network access to the provider

## Provider-Specific Notes

### Google
- **⚠️ Requires client_secret even for Desktop apps** (deviates from RFC 7636)
- Uses single string for `aud` claim (handled automatically)
- Tokens expire in 1 hour
- Refresh tokens don't expire but can be revoked
- See [GOOGLE_OIDC_SETUP.md](GOOGLE_OIDC_SETUP.md) for details on the client_secret requirement

### Azure AD
- **May support secret-less PKCE for public clients** (not yet tested)
- `aud` can be Application ID or API identifier
- Supports both v1.0 and v2.0 endpoints (use v2.0 for OIDC)
- Tenant-specific or multi-tenant issuers

### Okta
- **May support secret-less PKCE for public clients** (not yet tested)
- Domain format: `https://<org>.okta.com` or `https://<org>.oktapreview.com`
- Tokens expire based on org policy
- Can use custom authorization servers

### Auth0
- **May support secret-less PKCE for public clients** (not yet tested)
- Issuer URL requires trailing slash: `https://<domain>.auth0.com/`
- Can use custom domains
- Supports audience for API authorization

## Testing Secret-less PKCE with Other Providers

To test if a provider supports truly secret-less PKCE (without client_secret):

1. **Create a public client** (Desktop app, Native app, or CLI app type)
2. **Set environment variables without client_secret**:
   ```bash
   export OIDC_CLIENT_ID="<your-client-id>"
   export OIDC_ISSUER="<provider-issuer-url>"
   # Do NOT set OIDC_CLIENT_SECRET
   ```
3. **Run the test script**:
   ```bash
   python3 test_oidc_auth.py
   ```
4. **Expected results**:
   - ✅ If it works: Provider supports RFC 7636 compliant secret-less PKCE
   - ❌ If it fails with "client_secret is missing": Provider requires secret like Google

**Please report your findings** so we can update this documentation!

## Reference

- [OpenID Connect Specification](https://openid.net/connect/)
- [OAuth 2.0 RFC 6749](https://tools.ietf.org/html/rfc6749)
- [**PKCE RFC 7636**](https://tools.ietf.org/html/rfc7636) - The standard for secret-less public clients
- [Google OIDC](https://developers.google.com/identity/protocols/oauth2/openid-connect)
- [Azure AD OIDC](https://learn.microsoft.com/en-us/azure/active-directory/develop/v2-protocols-oidc)
- [Okta OIDC](https://developer.okta.com/docs/guides/implement-grant-type/authcode/main/)
- [Auth0 OIDC](https://auth0.com/docs/authenticate/protocols/openid-connect-protocol)
