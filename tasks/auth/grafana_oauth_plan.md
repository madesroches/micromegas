# Grafana Plugin OAuth 2.0 Authentication Plan

## Overview

Update the Micromegas Grafana datasource plugin to support OAuth 2.0 client credentials authentication while maintaining backward compatibility with API keys.

**Plugin Repository**: https://github.com/madesroches/grafana-micromegas-datasource/

## Current State

The Grafana plugin currently uses API keys for authentication:
- API key configured in datasource settings
- API key sent as Bearer token in Authorization header
- Simple string-based authentication

## Goals

1. **Add OAuth 2.0 client credentials support** (new option)
2. **Maintain API key support** (backward compatible)
3. **Automatic token fetching and caching** (OAuth only)
4. **Transparent token refresh** (OAuth only)
5. **Secure credential storage** (both methods)
6. **User choice of authentication method**

## Architecture Design

### 1. Datasource Configuration

**New Configuration Fields:**

```typescript
interface MicromegasDataSourceOptions extends DataSourceJsonData {
  // Authentication method selection
  authMethod: 'oauth' | 'apikey';  // User choice

  // OAuth 2.0 Client Credentials (when authMethod === 'oauth')
  oidcIssuer?: string;           // e.g., "https://accounts.google.com"
  oidcClientId?: string;         // e.g., "grafana-prod@project.iam.gserviceaccount.com"
  oidcAudience?: string;         // Optional, required for Auth0/Azure AD
}

interface MicromegasSecureJsonData {
  // OAuth 2.0 client secret (encrypted by Grafana, when authMethod === 'oauth')
  oidcClientSecret?: string;

  // API key (encrypted by Grafana, when authMethod === 'apikey')
  apiKey?: string;
}
```

**Configuration UI (datasource.tsx or similar):**

```tsx
// Authentication Method Selection
<FormField label="Authentication Method">
  <Select
    value={authMethod}
    options={[
      { label: 'API Key', value: 'apikey' },
      { label: 'OAuth 2.0 Client Credentials', value: 'oauth' }
    ]}
    onChange={onAuthMethodChange}
  />
</FormField>

{authMethod === 'apikey' && (
  <>
    <FormField label="API Key">
      <SecretInput
        value={secureJsonData.apiKey || ''}
        placeholder="Enter API key"
        isConfigured={secureJsonFields.apiKey}
        onReset={onResetApiKey}
        onChange={onApiKeyChange}
      />
    </FormField>
    <InlineFieldRow>
      <InlineField>
        <span className="help-text">
          Simple API key authentication. For better security and management,
          consider using OAuth 2.0 client credentials.
        </span>
      </InlineField>
    </InlineFieldRow>
  </>
)}

{authMethod === 'oauth' && (
  <>
    <FormField label="OIDC Issuer" required>
      <Input
        value={options.jsonData.oidcIssuer || ''}
        placeholder="https://accounts.google.com"
        onChange={onIssuerChange}
      />
      <InlineFieldRow>
        <InlineField>
          <span className="help-text">
            The OIDC provider URL (Google, Auth0, Azure AD, Okta)
          </span>
        </InlineField>
      </InlineFieldRow>
    </FormField>

    <FormField label="Client ID" required>
      <Input
        value={options.jsonData.oidcClientId || ''}
        placeholder="service-account@project.iam.gserviceaccount.com"
        onChange={onClientIdChange}
      />
    </FormField>

    <FormField label="Client Secret" required>
      <SecretInput
        value={secureJsonData.oidcClientSecret || ''}
        placeholder="Enter client secret"
        isConfigured={secureJsonFields.oidcClientSecret}
        onReset={onResetClientSecret}
        onChange={onClientSecretChange}
      />
    </FormField>

    <FormField label="Audience (optional)">
      <Input
        value={options.jsonData.oidcAudience || ''}
        placeholder="https://api.micromegas.example.com"
        onChange={onAudienceChange}
      />
      <InlineFieldRow>
        <InlineField>
          <span className="help-text">
            Required for Auth0 and Azure AD, optional for Google
          </span>
        </InlineField>
      </InlineFieldRow>
    </FormField>
  </>
)}

<Button onClick={onTestConnection}>Test Connection</Button>
```

### 2. OAuth 2.0 Client Credentials Implementation

**Token Manager Class:**

```typescript
// src/auth/OAuthTokenManager.ts

interface TokenCache {
  accessToken: string;
  expiresAt: number;  // Unix timestamp in milliseconds
}

interface OAuthConfig {
  issuer: string;
  clientId: string;
  clientSecret: string;
  audience?: string;
}

class OAuthTokenManager {
  private config: OAuthConfig;
  private cache: TokenCache | null = null;
  private tokenEndpoint: string | null = null;
  private refreshPromise: Promise<string> | null = null;

  constructor(config: OAuthConfig) {
    this.config = config;
  }

  /**
   * Get valid access token, fetching or refreshing if necessary
   */
  async getToken(): Promise<string> {
    // Check if cached token is still valid (with 5 min buffer)
    if (this.cache && this.cache.expiresAt > Date.now() + 5 * 60 * 1000) {
      return this.cache.accessToken;
    }

    // If a refresh is already in progress, wait for it
    if (this.refreshPromise) {
      return this.refreshPromise;
    }

    // Start new token fetch
    this.refreshPromise = this.fetchToken();

    try {
      const token = await this.refreshPromise;
      return token;
    } finally {
      this.refreshPromise = null;
    }
  }

  /**
   * Fetch new token from OIDC provider
   */
  private async fetchToken(): Promise<string> {
    // Discover token endpoint if not cached
    if (!this.tokenEndpoint) {
      this.tokenEndpoint = await this.discoverTokenEndpoint();
    }

    // Build token request
    const params = new URLSearchParams({
      grant_type: 'client_credentials',
      client_id: this.config.clientId,
      client_secret: this.config.clientSecret,
    });

    // Add audience if provided (required for Auth0/Azure AD)
    if (this.config.audience) {
      params.append('audience', this.config.audience);
    }

    // Fetch token from OIDC provider
    const response = await fetch(this.tokenEndpoint, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: params.toString(),
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(`Token fetch failed: ${response.status} ${errorText}`);
    }

    const data = await response.json();

    // Cache token with expiration
    const expiresIn = data.expires_in || 3600; // Default to 1 hour
    this.cache = {
      accessToken: data.access_token,
      expiresAt: Date.now() + expiresIn * 1000,
    };

    return data.access_token;
  }

  /**
   * Discover token endpoint from OIDC provider
   */
  private async discoverTokenEndpoint(): Promise<string> {
    const issuer = this.config.issuer.replace(/\/$/, ''); // Remove trailing slash
    const discoveryUrl = `${issuer}/.well-known/openid-configuration`;

    const response = await fetch(discoveryUrl);
    if (!response.ok) {
      throw new Error(`OIDC discovery failed: ${response.status}`);
    }

    const metadata = await response.json();
    return metadata.token_endpoint;
  }

  /**
   * Clear cached token (useful for testing or after errors)
   */
  clearCache() {
    this.cache = null;
  }
}

export { OAuthTokenManager, OAuthConfig };
```

### 3. Datasource Integration

**Update DataSource class:**

```typescript
// src/datasource.ts

import { DataSourceInstanceSettings } from '@grafana/data';
import { DataSourceWithBackend } from '@grafana/runtime';
import { MicromegasDataSourceOptions, MicromegasSecureJsonData } from './types';
import { OAuthTokenManager } from './auth/OAuthTokenManager';

export class MicromegasDataSource extends DataSourceWithBackend<
  MicromegasQuery,
  MicromegasDataSourceOptions
> {
  private oauthManager: OAuthTokenManager | null = null;
  private legacyApiKey: string | null = null;

  constructor(instanceSettings: DataSourceInstanceSettings<MicromegasDataSourceOptions>) {
    super(instanceSettings);

    // Initialize OAuth token manager if configured
    if (instanceSettings.jsonData.oidcIssuer &&
        instanceSettings.jsonData.oidcClientId &&
        instanceSettings.secureJsonData?.oidcClientSecret) {

      this.oauthManager = new OAuthTokenManager({
        issuer: instanceSettings.jsonData.oidcIssuer,
        clientId: instanceSettings.jsonData.oidcClientId,
        clientSecret: instanceSettings.secureJsonData.oidcClientSecret,
        audience: instanceSettings.jsonData.oidcAudience,
      });
    }
    // Legacy API key support
    else if (instanceSettings.secureJsonData?.apiKey) {
      this.legacyApiKey = instanceSettings.secureJsonData.apiKey;
    }
  }

  /**
   * Get authorization header with fresh token
   */
  private async getAuthHeader(): Promise<Record<string, string>> {
    if (this.oauthManager) {
      // Fetch token (may use cached token or fetch new one)
      const token = await this.oauthManager.getToken();
      return {
        Authorization: `Bearer ${token}`,
      };
    } else if (this.legacyApiKey) {
      // Legacy API key authentication
      return {
        Authorization: `Bearer ${this.legacyApiKey}`,
      };
    }

    throw new Error('No authentication configured');
  }

  /**
   * Override query method to add auth header
   */
  async query(request: DataQueryRequest<MicromegasQuery>): Promise<DataQueryResponse> {
    // Get fresh token before each query
    const authHeader = await this.getAuthHeader();

    // Add auth header to request
    const requestWithAuth = {
      ...request,
      headers: {
        ...request.headers,
        ...authHeader,
      },
    };

    // Call parent query method with auth
    return super.query(requestWithAuth);
  }

  /**
   * Test datasource connection
   */
  async testDatasource(): Promise<any> {
    try {
      // Get auth header (this will validate OAuth config and fetch token)
      const authHeader = await this.getAuthHeader();

      // Make test query to validate connection
      const testQuery = {
        targets: [{
          refId: 'A',
          rawSql: 'SELECT 1 as test',
        }],
        range: getDefaultTimeRange(),
        headers: authHeader,
      };

      await super.query(testQuery as any);

      return {
        status: 'success',
        message: 'Data source is working',
      };
    } catch (error) {
      return {
        status: 'error',
        message: `Connection test failed: ${error.message}`,
      };
    }
  }
}
```

### 4. Error Handling

**Common OAuth errors to handle:**

```typescript
// src/auth/errors.ts

export class OAuthError extends Error {
  constructor(message: string, public originalError?: any) {
    super(message);
    this.name = 'OAuthError';
  }
}

export class TokenFetchError extends OAuthError {
  constructor(message: string, originalError?: any) {
    super(message, originalError);
    this.name = 'TokenFetchError';
  }
}

export class DiscoveryError extends OAuthError {
  constructor(message: string, originalError?: any) {
    super(message, originalError);
    this.name = 'DiscoveryError';
  }
}

// Error messages for common issues
export const ERROR_MESSAGES = {
  INVALID_ISSUER: 'Invalid OIDC issuer URL. Check that the URL is correct and accessible.',
  DISCOVERY_FAILED: 'Failed to discover OIDC endpoints. Verify issuer URL and network connectivity.',
  TOKEN_FETCH_FAILED: 'Failed to fetch access token. Check client ID and secret.',
  INVALID_CLIENT: 'Invalid client credentials. Verify client ID and secret are correct.',
  NETWORK_ERROR: 'Network error while communicating with OIDC provider.',
};
```

**Error handling in token manager:**

```typescript
// Update fetchToken() method with better error handling
private async fetchToken(): Promise<string> {
  try {
    if (!this.tokenEndpoint) {
      try {
        this.tokenEndpoint = await this.discoverTokenEndpoint();
      } catch (error) {
        throw new DiscoveryError(ERROR_MESSAGES.DISCOVERY_FAILED, error);
      }
    }

    const params = new URLSearchParams({
      grant_type: 'client_credentials',
      client_id: this.config.clientId,
      client_secret: this.config.clientSecret,
    });

    if (this.config.audience) {
      params.append('audience', this.config.audience);
    }

    const response = await fetch(this.tokenEndpoint, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: params.toString(),
    });

    if (!response.ok) {
      const errorText = await response.text();

      // Parse OAuth error if available
      try {
        const errorJson = JSON.parse(errorText);
        if (errorJson.error === 'invalid_client') {
          throw new TokenFetchError(ERROR_MESSAGES.INVALID_CLIENT);
        }
      } catch (e) {
        // Not JSON, use raw error text
      }

      throw new TokenFetchError(
        `${ERROR_MESSAGES.TOKEN_FETCH_FAILED}: ${response.status} ${errorText}`
      );
    }

    const data = await response.json();

    if (!data.access_token) {
      throw new TokenFetchError('No access token in response');
    }

    const expiresIn = data.expires_in || 3600;
    this.cache = {
      accessToken: data.access_token,
      expiresAt: Date.now() + expiresIn * 1000,
    };

    return data.access_token;
  } catch (error) {
    if (error instanceof OAuthError) {
      throw error;
    }

    // Network or other unexpected errors
    throw new OAuthError(ERROR_MESSAGES.NETWORK_ERROR, error);
  }
}
```

### 5. Dependencies

**Add to package.json:**

```json
{
  "dependencies": {
    "@grafana/data": "latest",
    "@grafana/runtime": "latest",
    "@grafana/ui": "latest",
    // No additional dependencies needed - using native fetch API
  },
  "devDependencies": {
    "@types/node": "^20.0.0",
    "jest": "^29.0.0",
    "@testing-library/react": "^14.0.0"
  }
}
```

**Note**: Modern browsers and Node.js (18+) have native `fetch()` support, so no additional HTTP client library is needed.

## Deployment Strategy

### Version 2.0.0: Add OAuth Support (Backward Compatible)

**Goal**: Add OAuth 2.0 as an option while keeping API keys fully supported

1. **Plugin Update**:
   - Add OAuth 2.0 client credentials support
   - Keep API key support (no deprecation)
   - Update configuration UI to show both options as equal choices
   - Add setup guides for OAuth

2. **Features**:
   - Both authentication methods work indefinitely
   - Users choose based on their needs:
     - **API Keys**: Simple, quick setup, single credential
     - **OAuth 2.0**: Managed by identity provider, better for enterprise
   - No breaking changes
   - No migration pressure

3. **Communication**:
   - Release notes highlighting new OAuth support
   - Setup guides for both methods
   - No deprecation warnings

### Future: Long-term Support for Both Methods

**Goal**: Continue supporting both authentication methods

- API keys remain a first-class authentication method
- OAuth 2.0 available for users who prefer identity provider management
- Both methods maintained and supported equally
- User choice based on use case and organizational requirements

## Testing Strategy

### 1. Unit Tests

```typescript
// src/auth/OAuthTokenManager.test.ts

import { OAuthTokenManager } from './OAuthTokenManager';

describe('OAuthTokenManager', () => {
  let fetchMock: jest.Mock;

  beforeEach(() => {
    fetchMock = jest.fn();
    global.fetch = fetchMock;
  });

  it('should discover token endpoint', async () => {
    // Mock discovery response
    fetchMock.mockResolvedValueOnce({
      ok: true,
      json: async () => ({
        token_endpoint: 'https://accounts.google.com/token',
      }),
    });

    // Mock token response
    fetchMock.mockResolvedValueOnce({
      ok: true,
      json: async () => ({
        access_token: 'test-token',
        expires_in: 3600,
      }),
    });

    const manager = new OAuthTokenManager({
      issuer: 'https://accounts.google.com',
      clientId: 'test-client',
      clientSecret: 'test-secret',
    });

    const token = await manager.getToken();
    expect(token).toBe('test-token');
    expect(fetchMock).toHaveBeenCalledTimes(2); // Discovery + token
  });

  it('should cache tokens until expiration', async () => {
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          token_endpoint: 'https://accounts.google.com/token',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          access_token: 'cached-token',
          expires_in: 3600,
        }),
      });

    const manager = new OAuthTokenManager({
      issuer: 'https://accounts.google.com',
      clientId: 'test-client',
      clientSecret: 'test-secret',
    });

    const token1 = await manager.getToken();
    const token2 = await manager.getToken();

    expect(token1).toBe(token2);
    expect(fetchMock).toHaveBeenCalledTimes(2); // Only discovery + token, no second token fetch
  });

  it('should refresh expired tokens', async () => {
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          token_endpoint: 'https://accounts.google.com/token',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          access_token: 'token-1',
          expires_in: 1, // Expires in 1 second
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          access_token: 'token-2',
          expires_in: 3600,
        }),
      });

    const manager = new OAuthTokenManager({
      issuer: 'https://accounts.google.com',
      clientId: 'test-client',
      clientSecret: 'test-secret',
    });

    const token1 = await manager.getToken();

    // Wait for token to expire
    await new Promise(resolve => setTimeout(resolve, 6000)); // 6 seconds (includes 5 min buffer)

    const token2 = await manager.getToken();

    expect(token1).toBe('token-1');
    expect(token2).toBe('token-2');
  });

  it('should handle token fetch errors', async () => {
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          token_endpoint: 'https://accounts.google.com/token',
        }),
      })
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
        text: async () => 'Invalid client',
      });

    const manager = new OAuthTokenManager({
      issuer: 'https://accounts.google.com',
      clientId: 'bad-client',
      clientSecret: 'bad-secret',
    });

    await expect(manager.getToken()).rejects.toThrow('Token fetch failed');
  });

  it('should handle concurrent token requests', async () => {
    fetchMock
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          token_endpoint: 'https://accounts.google.com/token',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({
          access_token: 'shared-token',
          expires_in: 3600,
        }),
      });

    const manager = new OAuthTokenManager({
      issuer: 'https://accounts.google.com',
      clientId: 'test-client',
      clientSecret: 'test-secret',
    });

    // Make 10 concurrent requests
    const tokens = await Promise.all(
      Array(10).fill(null).map(() => manager.getToken())
    );

    // All should return same token
    expect(tokens.every(t => t === 'shared-token')).toBe(true);

    // Only one token fetch should have occurred
    expect(fetchMock).toHaveBeenCalledTimes(2); // Discovery + token
  });
});
```

### 2. Integration Tests

**Test with real OIDC providers:**

1. **Google OAuth**:
   - Create test service account
   - Configure OAuth client credentials
   - Test token fetch and query execution

2. **Auth0**:
   - Create test M2M application
   - Configure API and audience
   - Test token fetch and query execution

3. **Azure AD** (future):
   - Create test app registration
   - Test token fetch and query execution

### 3. Manual Testing Checklist

**OAuth 2.0 Tests:**
- [ ] OAuth 2.0 configuration saves correctly
- [ ] Client secret is encrypted and not visible after save
- [ ] Test connection succeeds with valid credentials
- [ ] Test connection fails with invalid credentials (clear error message)
- [ ] Queries execute successfully with OAuth token
- [ ] Token is cached and reused for multiple queries
- [ ] Token refreshes automatically after expiration
- [ ] Invalid issuer URL shows helpful error message
- [ ] Invalid client credentials show helpful error message
- [ ] Network errors are handled gracefully

**API Key Tests:**
- [ ] API key configuration saves correctly
- [ ] API key is encrypted and not visible after save
- [ ] Test connection succeeds with valid API key
- [ ] Test connection fails with invalid API key (clear error message)
- [ ] Queries execute successfully with API key
- [ ] API key authentication works without any warnings

**Switching Tests:**
- [ ] Can switch from API key to OAuth and back
- [ ] Existing API key datasources continue to work after plugin update
- [ ] Can configure new datasources with either method

## Documentation

### 1. OAuth Setup Guide

**Create `OAUTH_SETUP.md` in plugin repo:**

```markdown
# OAuth 2.0 Client Credentials Setup

This guide shows you how to configure OAuth 2.0 client credentials authentication for the Micromegas Grafana datasource.

## When to Use OAuth 2.0

OAuth 2.0 client credentials is recommended when:
- Your organization uses an identity provider (Google, Auth0, Azure AD, Okta)
- You want centralized service account management
- You need credential rotation managed by identity provider
- You want built-in audit trails in identity provider

**Note**: API keys remain fully supported and are simpler for small deployments or quick setup.

## Benefits of OAuth 2.0

- **Centralized Management**: Service accounts managed in your identity provider
- **Automatic Rotation**: Credentials can be rotated in identity provider
- **Audit Trail**: Built-in audit logging in identity provider
- **Industry Standard**: OAuth 2.0 is the de facto standard for service authentication

## Prerequisites

You need access to one of these identity providers:
- Google Cloud (recommended)
- Auth0
- Azure AD
- Okta
- Any OIDC-compliant provider

## Step 1: Create Service Account

### Google Cloud

```bash
# Create service account
gcloud iam service-accounts create grafana-prod \
  --display-name="Grafana Micromegas Datasource"

# Create OAuth client credentials
gcloud iam service-accounts keys create credentials.json \
  --iam-account=grafana-prod@YOUR-PROJECT.iam.gserviceaccount.com

# Note the client_id (service account email) and client_secret (from credentials.json)
```

### Auth0

1. Go to Auth0 Dashboard → Applications → Create Application
2. Choose "Machine to Machine"
3. Name it "Grafana Micromegas Datasource"
4. Select your Micromegas API
5. Note the Client ID and Client Secret
6. Your API must have an audience configured

### Azure AD

```bash
# Create app registration
az ad app create --display-name "grafana-micromegas-datasource"

# Create client secret
az ad app credential reset --id <app-id>

# Note the client_id (application ID) and client_secret
```

## Step 2: Update Grafana Datasource

1. Go to Grafana → Configuration → Data Sources
2. Select your Micromegas datasource
3. Change Authentication Method to "OAuth 2.0 Client Credentials"
4. Enter configuration:
   - **OIDC Issuer**: Your provider URL
     - Google: `https://accounts.google.com`
     - Auth0: `https://YOUR-TENANT.us.auth0.com/`
     - Azure AD: `https://login.microsoftonline.com/YOUR-TENANT/v2.0`
   - **Client ID**: From step 1
   - **Client Secret**: From step 1
   - **Audience**: (Only for Auth0/Azure AD) Your API identifier
5. Click "Save & Test"

## Step 3: Verify

1. Test connection should succeed
2. Try running a query in a dashboard
3. Check that queries return data
4. Verify in identity provider audit logs that authentication is working

## Optional: Continue Using API Keys

You can continue using API keys if you prefer. Both authentication methods are fully supported:
- **API Keys**: Simpler, managed directly in flight-sql-srv configuration
- **OAuth 2.0**: Managed by identity provider, better for enterprise environments

Choose the method that best fits your deployment and security requirements.

## Troubleshooting

### "Discovery failed" error
- Check OIDC issuer URL is correct and accessible
- Ensure URL has no trailing slash (except for Auth0)

### "Invalid client" error
- Verify client ID and client secret are correct
- Check that service account is enabled in identity provider

### "Unauthorized" error from flight-sql-srv
- Ensure flight-sql-srv has OIDC configured with same issuer
- Check audience configuration matches

## Support

For issues, please file a bug report at:
https://github.com/madesroches/grafana-micromegas-datasource/issues
```

### 2. Update README

Add OAuth 2.0 configuration section to plugin README with examples for each provider.

### 3. Provider Setup Guides

Create detailed guides for each major provider:
- `docs/google-oauth-setup.md`
- `docs/auth0-setup.md`
- `docs/azure-ad-setup.md`

## Implementation Checklist

- [ ] Implement `OAuthTokenManager` class with unit tests
- [ ] Update datasource class with OAuth integration
- [ ] Update configuration UI with both auth method options
- [ ] Error handling and testing
- [ ] Documentation and setup guides
- [ ] Integration testing with real providers
- [ ] Review and release

**Release Plan**:
- **v2.0.0**: Add OAuth 2.0 support (backward compatible, no breaking changes)
- Both API keys and OAuth 2.0 supported long-term

## Security Considerations

1. **Client Secret Storage**:
   - Client secrets are encrypted by Grafana's secure JSON data storage
   - Never log or display client secrets
   - Secrets stored in Grafana database with encryption

2. **Token Caching**:
   - Tokens cached in memory only (not persisted)
   - Cache cleared on datasource reload
   - 5-minute expiration buffer for safety

3. **Network Security**:
   - All OAuth communication over HTTPS
   - Token endpoint URLs validated
   - No token exposure in UI or logs

4. **Error Messages**:
   - Don't leak sensitive information in errors
   - Generic error messages for authentication failures
   - Detailed errors only in browser console (dev mode)

## Success Metrics

1. OAuth 2.0 client credentials working with all major providers
2. Zero breaking changes - existing API key users unaffected
3. API key and OAuth methods work equally well (no preference)
4. Clear error messages for common configuration issues
5. Token fetch completes in < 2 seconds (OAuth)
6. Token refresh happens transparently (OAuth)
7. Complete setup documentation for both methods
8. Unit test coverage > 90%

## Open Questions

1. Should we support multiple authentication methods per datasource?
   - **Decision**: No, one method per datasource for simplicity

2. Should we show token expiration time in UI?
   - **Decision**: No, keep UI simple, refresh happens automatically

3. Should we support custom token endpoint URLs?
   - **Decision**: No, use OIDC discovery for standards compliance

4. Should we support client authentication via JWT assertion?
   - **Decision**: Not in v2.0, evaluate for future versions

5. Will API keys be deprecated in the future?
   - **Decision**: No, both methods supported indefinitely. User choice.

## Related Resources

- Analytics Server Auth Plan: `tasks/auth/analytics_auth_plan.md`
- OIDC Implementation: `tasks/auth/oidc_auth_subplan.md`
- Security Review: `tasks/auth/sectodo.md`
- Grafana Plugin Repository: https://github.com/madesroches/grafana-micromegas-datasource/
