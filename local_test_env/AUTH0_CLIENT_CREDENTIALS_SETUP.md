# Auth0 OAuth 2.0 Client Credentials Setup

This guide walks through setting up Auth0 for testing OAuth 2.0 client credentials flow with Micromegas.

## Step 1: Create Auth0 Account

1. Go to https://auth0.com/
2. Sign up for a free account
3. Create a new tenant (e.g., `micromegas-test`)

## Step 2: Create an API

Client credentials flow requires an API resource (audience):

1. In Auth0 Dashboard, go to **Applications → APIs**
2. Click **Create API**
3. Fill in:
   - **Name**: `Micromegas Analytics`
   - **Identifier**: `https://api.micromegas.example.com` (can be any URI, doesn't need to be real)
   - **Signing Algorithm**: **`RS256`** ⚠️ IMPORTANT: Use RS256 for machine-to-machine (asymmetric)
     - RS256 = RSA with SHA-256 (public/private key pair)
     - Tokens can be verified using public key from JWKS endpoint
     - Required for server-side token validation
     - **Do NOT use HS256** (symmetric, requires shared secret)
4. Click **Create**

5. **Configure Token Settings** (in API → Settings):
   - **Access Token Settings → JWT Profile**: Select **`RFC 9068`** (recommended)
     - **RFC 9068** = Standard JWT Access Token format (new standard, more claims)
     - **Auth0** = Legacy format (minimal claims, works but deprecated)
     - Both work with Micromegas, but RFC 9068 is the modern standard
     - Includes standard claims like `sub`, `iss`, `aud`, `exp`, `iat`
   - **Token Expiration**: 86400 seconds (24 hours) - default is fine
   - **Allow Offline Access**: Leave disabled (not needed for client credentials)

## Step 3: Create a Machine-to-Machine Application

1. Go to **Applications → Applications**
2. Click **Create Application**
3. Fill in:
   - **Name**: `Micromegas Service Account`
   - **Application Type**: Select **Machine to Machine Applications**
4. Click **Create**
5. Select the API you created (`Micromegas Analytics`)
6. Authorize the application (select permissions if needed)
7. Click **Authorize**

## Step 4: Get Your Credentials

On the application settings page, you'll see:

- **Domain**: `YOUR_TENANT.us.auth0.com` (or your region)
- **Client ID**: A long alphanumeric string
- **Client Secret**: A long secret string (click "Show" to reveal)

## Step 5: Configure Environment Variables

Export these variables in your shell:

```bash
# Auth0 configuration
export MICROMEGAS_OIDC_ISSUER="https://YOUR_TENANT.us.auth0.com"
export MICROMEGAS_OIDC_CLIENT_ID="YOUR_CLIENT_ID"
export MICROMEGAS_OIDC_CLIENT_SECRET="YOUR_CLIENT_SECRET"
export MICROMEGAS_OIDC_AUDIENCE="https://api.micromegas.example.com"
```

**Important**: Replace `YOUR_TENANT`, `YOUR_CLIENT_ID`, and `YOUR_CLIENT_SECRET` with your actual values.

## Step 6: Test Token Fetch

Test that you can fetch a token:

```bash
curl --request POST \
  --url "https://YOUR_TENANT.us.auth0.com/oauth/token" \
  --header 'content-type: application/json' \
  --data '{
    "client_id": "YOUR_CLIENT_ID",
    "client_secret": "YOUR_CLIENT_SECRET",
    "audience": "https://api.micromegas.example.com",
    "grant_type": "client_credentials"
  }'
```

You should get a response like:
```json
{
  "access_token": "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9...",
  "token_type": "Bearer",
  "expires_in": 86400
}
```

## Step 7: Configure Analytics Server

Create OIDC config for the analytics server:

```bash
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://YOUR_TENANT.us.auth0.com/",
      "audience": "https://api.micromegas.example.com"
    }
  ],
  "jwks_refresh_interval_secs": 3600,
  "token_cache_size": 1000,
  "token_cache_ttl_secs": 300
}'
```

**Note**: Auth0 issuer URL must end with `/` for OIDC discovery to work.

## Step 8: Test with Python Client

Run the test script:

```bash
cd /home/mad/micromegas
poetry run python local_test_env/test_client_credentials.py
```

## Troubleshooting

### Error: "Invalid issuer"
- Make sure issuer URL ends with `/`
- Auth0 uses `https://YOUR_TENANT.us.auth0.com/` (with trailing slash)

### Error: "Invalid audience"
- Make sure the `audience` parameter is included in token request
- Auth0 requires explicit audience for API access tokens

### Error: "Unauthorized client"
- Verify the Machine-to-Machine app is authorized for the API
- Check that client_id and client_secret are correct

## Key Differences from Google

1. **Audience Required**: Auth0 requires `audience` parameter in token request
2. **Issuer URL**: Must end with `/` for OIDC discovery
3. **Token Endpoint**: Different from standard (`/oauth/token` instead of `/token`)
4. **Longer Token Lifetime**: Auth0 defaults to 24 hours vs Google's 1 hour

## Next Steps

Once you have Auth0 credentials working:

1. Test token fetch with Python client
2. Test query execution with authenticated FlightSQL client
3. Verify token caching and refresh behavior
4. Test with analytics server OIDC validation
