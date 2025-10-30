# Authentication

The Micromegas Grafana plugin supports two authentication methods.

## Authentication Methods

### API Keys

Simple authentication using a static API key.

- **Best for**: Development, small deployments, quick start
- **Setup time**: 2 minutes

### OAuth 2.0 Client Credentials

Enterprise authentication via identity provider (Google, Auth0, Azure AD, Okta).

- **Best for**: Production, enterprise deployments
- **Setup time**: 15-30 minutes

## API Key Authentication

### Quick Setup

1. **Generate API Key**:
   ```bash
   openssl rand -base64 512
   ```

2. **Configure server** (see [Admin Guide](../admin/authentication.md)):
   ```bash
   export MICROMEGAS_API_KEYS='[
     {"name": "grafana-prod", "key": "YOUR_GENERATED_KEY_HERE"}
   ]'
   ```

3. **Configure Grafana datasource**:
   - Auth Method: API Key
   - API Key: Paste your generated key
   - Save & Test

## OAuth 2.0 Client Credentials

### Quick Setup

1. **Create service account** in your identity provider:
   - **Google**: Service account with JSON key
   - **Auth0**: Machine-to-Machine application
   - **Azure AD**: App registration with client secret
   - **Okta**: Service app

2. **Configure server** with OIDC settings (see [Admin Guide](../admin/authentication.md))

3. **Configure Grafana datasource**:
   - Auth Method: OAuth 2.0 Client Credentials
   - OIDC Issuer: Your provider URL
   - Client ID: From step 1
   - Client Secret: From step 1
   - Audience: (Auth0/Azure AD only)
   - Save & Test

### Provider URLs

| Provider | Issuer URL |
|----------|------------|
| Google | `https://accounts.google.com` |
| Auth0 | `https://YOUR-TENANT.auth0.com` |
| Azure AD | `https://login.microsoftonline.com/TENANT-ID/v2.0` |
| Okta | `https://YOUR-DOMAIN.okta.com` |

### Example: Google Cloud

```bash
# Create service account
gcloud iam service-accounts create grafana-prod \
  --display-name="Grafana Micromegas Datasource"

# Create key
gcloud iam service-accounts keys create credentials.json \
  --iam-account=grafana-prod@PROJECT.iam.gserviceaccount.com
```

**Grafana Configuration**:
```
Auth Method: OAuth 2.0 Client Credentials
OIDC Issuer: https://accounts.google.com
Client ID: grafana-prod@PROJECT.iam.gserviceaccount.com
Client Secret: (from credentials.json)
```

## Testing

Click **Save & Test** to verify connection and authentication.

## Next Steps

- [Configure queries](usage.md)
- [Server authentication setup](../admin/authentication.md)
