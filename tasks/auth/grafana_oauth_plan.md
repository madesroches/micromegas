# Grafana Plugin OAuth 2.0 Authentication Plan

## Overview

Update the Micromegas Grafana datasource plugin to support OAuth 2.0 client credentials authentication while maintaining backward compatibility with existing authentication methods.

**Plugin Location**: `/home/mad/micromegas/grafana/` (monorepo)

**Plugin Type**: Backend datasource plugin
- **Frontend**: React/TypeScript (`grafana/src/`)
- **Backend**: Go (`grafana/pkg/flightsql/`)

## Current State

### Architecture

The plugin is a **backend datasource plugin** where:
- **Frontend (React)**: Provides configuration UI, delegates queries to backend
- **Backend (Go)**: Handles FlightSQL communication and authentication
- **Grafana**: Encrypts sensitive fields, proxies requests to backend

### Current Authentication Methods

Defined in `grafana/src/types.ts`:
```typescript
export const authTypeOptions = [
  {key: 0, label: 'none', value: 'none'},
  {key: 1, label: 'username/password', value: 'username/password'},
  {key: 2, label: 'token', value: 'token'},
]
```

**Implementation in Go** (`grafana/pkg/flightsql/flightsql.go` lines 97-108):
- `username/password` → FlightSQL BasicToken authentication
- `token` → Bearer token in Authorization header: `Bearer {token}`
- Metadata key-value pairs → gRPC headers

## Goals

1. **Add OAuth 2.0 client credentials** as 4th authentication method
2. **Maintain backward compatibility** with existing auth methods
3. **Automatic token fetching and caching** in Go backend
4. **Transparent token refresh** before each query
5. **Secure credential storage** using Grafana's encrypted secureJsonData
6. **User choice** via dropdown selector in config UI

## Security Model

### What Gets Encrypted

**Stored in `jsonData` (NOT encrypted - visible in Grafana DB):**
- `host` - FlightSQL server address
- `selectedAuthType` - 'none', 'username/password', 'token', 'oauth2'
- `username` - Username for basic auth
- `oauthIssuer` - OIDC provider URL (e.g., "https://accounts.google.com")
- `oauthClientId` - OAuth client ID (public identifier)
- `oauthAudience` - Optional audience for Auth0/Azure AD
- `metadata` - Key-value pairs for gRPC headers

**Stored in `secureJsonData` (ENCRYPTED by Grafana):**
- `password` - Password for basic auth
- `token` - API key for Bearer token auth
- `oauthClientSecret` - OAuth client secret ⚠️ SENSITIVE

### Backend Access Pattern

```go
// Read unencrypted config
var cfg config
json.Unmarshal(settings.JSONData, &cfg)  // Gets host, selectedAuthType, oauthIssuer, etc.

// Read encrypted secrets (Grafana decrypts before passing to plugin)
if secret, exists := settings.DecryptedSecureJSONData["oauthClientSecret"]; exists {
    cfg.OAuthClientSecret = secret  // Already decrypted by Grafana
}
```

This is the same pattern used for `token` and `password` fields.

## Implementation Plan

### Phase 1: Frontend Configuration (TypeScript/React)

#### 1.1 Update Type Definitions

**File**: `grafana/src/types.ts`

```typescript
// Add OAuth to auth options
export const authTypeOptions = [
  {key: 0, label: 'none', value: 'none'},
  {key: 1, label: 'username/password', value: 'username/password'},
  {key: 2, label: 'token', value: 'token'},
  {key: 3, label: 'oauth2-client-credentials', value: 'oauth2'},  // NEW
]

// Add OAuth fields to datasource options
export interface FlightSQLDataSourceOptions extends DataSourceJsonData {
  host?: string
  token?: string
  secure?: boolean
  username?: string
  password?: string
  selectedAuthType?: string
  metadata?: any

  // OAuth 2.0 Client Credentials (NEW - stored unencrypted)
  oauthIssuer?: string           // e.g., "https://accounts.google.com"
  oauthClientId?: string         // e.g., "grafana@project.iam.gserviceaccount.com"
  oauthAudience?: string         // Optional, for Auth0/Azure AD
}

// Add OAuth secret to secure data
export interface SecureJsonData {
  password?: string
  token?: string
  oauthClientSecret?: string    // NEW - encrypted by Grafana
}
```

#### 1.2 Update Configuration UI

**File**: `grafana/src/components/ConfigEditor.tsx`

Add OAuth configuration section after line 109 (after username/password section):

```tsx
{selectedAuthType?.label === 'oauth2' && (
  <>
    <InlineField
      labelWidth={20}
      label="OIDC Issuer"
      tooltip="Identity provider URL (e.g., https://accounts.google.com)"
    >
      <Input
        width={40}
        name="oauthIssuer"
        type="text"
        value={jsonData.oauthIssuer || ''}
        placeholder="https://accounts.google.com"
        onChange={(e) => onOAuthIssuerChange(e, options, onOptionsChange)}
      />
    </InlineField>

    <InlineField labelWidth={20} label="Client ID">
      <Input
        width={40}
        name="oauthClientId"
        type="text"
        value={jsonData.oauthClientId || ''}
        placeholder="service@project.iam.gserviceaccount.com"
        onChange={(e) => onOAuthClientIdChange(e, options, onOptionsChange)}
      />
    </InlineField>

    <InlineField labelWidth={20} label="Client Secret">
      <SecretInput
        width={40}
        name="oauthClientSecret"
        type="text"
        value={secureJsonData?.oauthClientSecret || ''}
        placeholder="****************"
        onChange={(e) => onOAuthClientSecretChange(e, options, onOptionsChange)}
        onReset={() => onResetOAuthClientSecret(options, onOptionsChange)}
        isConfigured={secureJsonFields?.oauthClientSecret}
      />
    </InlineField>

    <InlineField
      labelWidth={20}
      label="Audience (optional)"
      tooltip="Required for Auth0 and Azure AD"
    >
      <Input
        width={40}
        name="oauthAudience"
        type="text"
        value={jsonData.oauthAudience || ''}
        placeholder="https://api.micromegas.example.com"
        onChange={(e) => onOAuthAudienceChange(e, options, onOptionsChange)}
      />
    </InlineField>

    <InlineFieldRow>
      <InlineField>
        <span className="help-text">
          OAuth 2.0 client credentials flow for service accounts.
          Credentials managed by identity provider (Google, Auth0, Azure AD, Okta).
        </span>
      </InlineField>
    </InlineFieldRow>
  </>
)}
```

#### 1.3 Add Configuration Handlers

**File**: `grafana/src/components/utils.ts`

```typescript
import {DataSourcePluginOptionsEditorProps} from '@grafana/data'
import {FlightSQLDataSourceOptions, SecureJsonData} from '../types'

type EditorProps = DataSourcePluginOptionsEditorProps<FlightSQLDataSourceOptions, SecureJsonData>

export function onOAuthIssuerChange(
  event: React.SyntheticEvent<HTMLInputElement>,
  options: EditorProps['options'],
  onOptionsChange: EditorProps['onOptionsChange']
) {
  const jsonData = {
    ...options.jsonData,
    oauthIssuer: event.currentTarget.value,
  }
  onOptionsChange({...options, jsonData})
}

export function onOAuthClientIdChange(
  event: React.SyntheticEvent<HTMLInputElement>,
  options: EditorProps['options'],
  onOptionsChange: EditorProps['onOptionsChange']
) {
  const jsonData = {
    ...options.jsonData,
    oauthClientId: event.currentTarget.value,
  }
  onOptionsChange({...options, jsonData})
}

export function onOAuthAudienceChange(
  event: React.SyntheticEvent<HTMLInputElement>,
  options: EditorProps['options'],
  onOptionsChange: EditorProps['onOptionsChange']
) {
  const jsonData = {
    ...options.jsonData,
    oauthAudience: event.currentTarget.value,
  }
  onOptionsChange({...options, jsonData})
}

export function onOAuthClientSecretChange(
  event: React.SyntheticEvent<HTMLInputElement>,
  options: EditorProps['options'],
  onOptionsChange: EditorProps['onOptionsChange']
) {
  onOptionsChange({
    ...options,
    secureJsonData: {
      ...options.secureJsonData,
      oauthClientSecret: event.currentTarget.value,
    },
  })
}

export function onResetOAuthClientSecret(
  options: EditorProps['options'],
  onOptionsChange: EditorProps['onOptionsChange']
) {
  onOptionsChange({
    ...options,
    secureJsonFields: {
      ...options.secureJsonFields,
      oauthClientSecret: false,
    },
    secureJsonData: {
      ...options.secureJsonData,
      oauthClientSecret: '',
    },
  })
}
```

### Phase 2: Backend OAuth Implementation (Go)

#### 2.1 Update Config Struct

**File**: `grafana/pkg/flightsql/flightsql.go`

```go
type config struct {
	Addr     string              `json:"host"`
	Metadata []map[string]string `json:"metadata"`
	Secure   bool                `json:"secure"`
	Username string              `json:"username"`
	Password string              `json:"password"`
	Token    string              `json:"token"`

	// OAuth 2.0 Client Credentials (NEW)
	OAuthIssuer       string `json:"oauthIssuer"`
	OAuthClientId     string `json:"oauthClientId"`
	OAuthClientSecret string `json:"oauthClientSecret"`  // Populated from DecryptedSecureJSONData
	OAuthAudience     string `json:"oauthAudience"`
}

func (cfg config) validate() error {
	if strings.Count(cfg.Addr, ":") == 0 {
		return fmt.Errorf(`server address must be in the form "host:port"`)
	}

	noToken := len(cfg.Token) == 0
	noUserPass := len(cfg.Username) == 0 || len(cfg.Password) == 0
	noOAuth := len(cfg.OAuthIssuer) == 0 || len(cfg.OAuthClientId) == 0 || len(cfg.OAuthClientSecret) == 0

	// if secure, require some form of auth
	if noToken && noUserPass && noOAuth && cfg.Secure {
		return fmt.Errorf("token, username/password, or OAuth credentials are required")
	}

	return nil
}
```

#### 2.2 Add OAuth2 Library Dependency

First, add the official Go OAuth2 library:

```bash
cd /home/mad/micromegas/grafana
go get golang.org/x/oauth2
```

This adds to `go.mod`:
```
require (
    golang.org/x/oauth2 v0.15.0
)
```

**Why use `golang.org/x/oauth2`?**
- Official Go extended library (maintained by Go team)
- Automatic token caching and refresh (no manual mutex/expiry logic needed)
- Thread-safe by design
- Battle-tested in thousands of production systems
- Reduces implementation from ~150 lines to ~50 lines

#### 2.3 Create OAuth Token Manager

**File**: `grafana/pkg/flightsql/oauth.go` (NEW FILE)

```go
package flightsql

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	"golang.org/x/oauth2"
	"golang.org/x/oauth2/clientcredentials"
)

// OAuthTokenManager handles OAuth 2.0 client credentials flow
// Uses golang.org/x/oauth2 for automatic token caching and refresh
type OAuthTokenManager struct {
	tokenSource oauth2.TokenSource
	config      *clientcredentials.Config
}

// NewOAuthTokenManager creates a new OAuth token manager
// The oauth2 library handles caching and automatic token refresh
func NewOAuthTokenManager(issuer, clientId, clientSecret, audience string) (*OAuthTokenManager, error) {
	// Discover token endpoint from OIDC provider
	tokenEndpoint, err := discoverTokenEndpoint(issuer)
	if err != nil {
		return nil, fmt.Errorf("OIDC discovery failed: %w", err)
	}

	// Configure client credentials flow
	config := &clientcredentials.Config{
		ClientID:     clientId,
		ClientSecret: clientSecret,
		TokenURL:     tokenEndpoint,
	}

	// Add audience if provided (required for Auth0/Azure AD)
	if audience != "" {
		config.EndpointParams = map[string][]string{
			"audience": {audience},
		}
	}

	logInfof("OAuth token manager initialized: issuer=%s, endpoint=%s", issuer, tokenEndpoint)

	// Create token source - handles all caching and refresh automatically!
	tokenSource := config.TokenSource(context.Background())

	return &OAuthTokenManager{
		tokenSource: tokenSource,
		config:      config,
	}, nil
}

// GetToken returns a valid access token
// The oauth2 library automatically handles caching and refresh
func (m *OAuthTokenManager) GetToken(ctx context.Context) (string, error) {
	token, err := m.tokenSource.Token()
	if err != nil {
		return "", fmt.Errorf("failed to get OAuth token: %w", err)
	}

	logInfof("OAuth token retrieved, expires at: %s", token.Expiry.Format("2006-01-02 15:04:05"))

	return token.AccessToken, nil
}

// discoverTokenEndpoint fetches OIDC discovery document to find token endpoint
func discoverTokenEndpoint(issuer string) (string, error) {
	discoveryURL := strings.TrimSuffix(issuer, "/") + "/.well-known/openid-configuration"

	resp, err := http.Get(discoveryURL)
	if err != nil {
		return "", fmt.Errorf("discovery request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("discovery failed with status: %d", resp.StatusCode)
	}

	var discovery struct {
		TokenEndpoint string `json:"token_endpoint"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&discovery); err != nil {
		return "", fmt.Errorf("failed to parse discovery response: %w", err)
	}

	if discovery.TokenEndpoint == "" {
		return "", fmt.Errorf("token_endpoint not found in discovery document")
	}

	return discovery.TokenEndpoint, nil
}
```

**Key Benefits of Using `oauth2` Library:**
- **Automatic Caching**: Tokens cached in memory, reused until expiration
- **Automatic Refresh**: Expired tokens refreshed transparently
- **Thread Safety**: Built-in mutex protection, safe for concurrent use
- **Expiration Handling**: Checks token expiry with safety buffer
- **Standard Compliance**: Implements OAuth 2.0 spec correctly
- **Minimal Code**: ~50 lines vs ~150 lines manual implementation

#### 2.4 Integrate OAuth into NewDatasource

**File**: `grafana/pkg/flightsql/flightsql.go`

Update the `NewDatasource` function (lines 60-127):

```go
func NewDatasource(ctx context.Context, settings backend.DataSourceInstanceSettings) (instancemgmt.Instance, error) {
	var cfg config

	err := json.Unmarshal(settings.JSONData, &cfg)
	if err != nil {
		return nil, fmt.Errorf("config: %s", err)
	}

	// Read encrypted secrets from Grafana
	if token, exists := settings.DecryptedSecureJSONData["token"]; exists {
		cfg.Token = token
	}

	if password, exists := settings.DecryptedSecureJSONData["password"]; exists {
		cfg.Password = password
	}

	// NEW: Read OAuth client secret from encrypted storage
	if oauthSecret, exists := settings.DecryptedSecureJSONData["oauthClientSecret"]; exists {
		cfg.OAuthClientSecret = oauthSecret
	}

	if err := cfg.validate(); err != nil {
		return nil, fmt.Errorf("config validation: %v", err)
	}

	client, err := newFlightSQLClient(cfg)
	if err != nil {
		return nil, fmt.Errorf("flightsql: %s", err)
	}

	md := metadata.MD{}

	// Handle custom metadata
	for _, m := range cfg.Metadata {
		for k, v := range m {
			if _, ok := md[k]; ok {
				return nil, fmt.Errorf("metadata: duplicate key: %s", k)
			}
			if k != "" {
				md.Set(k, v)
			}
		}
	}

	// Handle username/password authentication
	if len(cfg.Username) > 0 || len(cfg.Password) > 0 {
		ctx, err = client.FlightClient().AuthenticateBasicToken(ctx, cfg.Username, cfg.Password)
		if err != nil {
			return nil, fmt.Errorf("flightsql: %s", err)
		}
		authMD, _ := metadata.FromOutgoingContext(ctx)
		md = metadata.Join(md, authMD)
	}

	// Handle token authentication (existing API key)
	if cfg.Token != "" {
		md.Set("Authorization", fmt.Sprintf("Bearer %s", cfg.Token))
	}

	// NEW: Handle OAuth 2.0 client credentials
	var oauthMgr *OAuthTokenManager
	if cfg.OAuthIssuer != "" && cfg.OAuthClientId != "" && cfg.OAuthClientSecret != "" {
		oauthMgr, err = NewOAuthTokenManager(
			cfg.OAuthIssuer,
			cfg.OAuthClientId,
			cfg.OAuthClientSecret,
			cfg.OAuthAudience,
		)
		if err != nil {
			return nil, fmt.Errorf("oauth initialization: %v", err)
		}

		// Fetch initial token to validate configuration
		token, err := oauthMgr.GetToken(ctx)
		if err != nil {
			return nil, fmt.Errorf("oauth token fetch: %v", err)
		}

		// Set initial token in metadata
		md.Set("Authorization", fmt.Sprintf("Bearer %s", token))

		logInfof("OAuth authentication initialized successfully")
	}

	ds := &FlightSQLDatasource{
		client:   client,
		md:       md,
		oauthMgr: oauthMgr,  // NEW: Store for token refresh
	}

	r := chi.NewRouter()
	r.Use(recoverer)
	r.Route("/plugin", func(r chi.Router) {
		r.Get("/macros", ds.getMacros)
	})
	r.Route("/flightsql", func(r chi.Router) {
		r.Get("/sql-info", ds.getSQLInfo)
		r.Get("/tables", ds.getTables)
		r.Get("/columns", ds.getColumns)
	})
	ds.resourceHandler = httpadapter.New(r)

	return ds, nil
}
```

#### 2.5 Add OAuth Field to FlightSQLDatasource Struct

**File**: `grafana/pkg/flightsql/flightsql.go`

Update the struct definition (around line 52):

```go
// FlightSQLDatasource is a Grafana datasource plugin for Flight SQL.
type FlightSQLDatasource struct {
	client          *client
	resourceHandler backend.CallResourceHandler
	md              metadata.MD
	oauthMgr        *OAuthTokenManager  // NEW: OAuth token manager
}
```

#### 2.6 Implement Token Refresh on Each Query

**File**: `grafana/pkg/flightsql/query_data.go`

Update the `QueryData` method to refresh OAuth token before each query:

```go
// QueryData handles multiple queries and returns multiple responses.
func (d *FlightSQLDatasource) QueryData(ctx context.Context, req *backend.QueryDataRequest) (*backend.QueryDataResponse, error) {
	// NEW: Refresh OAuth token if using OAuth authentication
	if d.oauthMgr != nil {
		token, err := d.oauthMgr.GetToken(ctx)
		if err != nil {
			logErrorf("OAuth token refresh failed: %v", err)
			// Return error for all queries
			response := backend.NewQueryDataResponse()
			for _, q := range req.Queries {
				response.Responses[q.RefID] = backend.DataResponse{
					Error: fmt.Errorf("OAuth token refresh failed: %v", err),
				}
			}
			return response, nil
		}

		// Update metadata with fresh token
		d.md.Set("Authorization", fmt.Sprintf("Bearer %s", token))
	}

	// Continue with existing query logic...
	response := backend.NewQueryDataResponse()
	for _, q := range req.Queries {
		res := d.query(ctx, parseQuery(q))
		response.Responses[q.RefID] = res
	}
	return response, nil
}
```

#### 2.7 Add User Attribution (Identity Logging)

**Goal**: Log which end user is running queries on the FlightSQL server

**Problem**: The FlightSQL server authenticates the client (via OAuth/API key), but doesn't know which end user is making the request (e.g., Grafana user viewing dashboard, Python service user, etc.).

**Solution**: Pass user information as gRPC metadata headers using generic header names

**File**: `grafana/pkg/flightsql/query_data.go`

Add user context extraction and header injection:

```go
// QueryData handles multiple queries and returns multiple responses.
func (d *FlightSQLDatasource) QueryData(ctx context.Context, req *backend.QueryDataRequest) (*backend.QueryDataResponse, error) {
	// NEW: Extract user information from plugin context and pass to FlightSQL server
	// Uses generic header names that work for any client (Grafana, Python services, etc.)
	if req.PluginContext.User != nil {
		user := req.PluginContext.User

		// Add end-user identity to gRPC metadata
		// FlightSQL server can log these headers for attribution
		if user.Login != "" {
			d.md.Set("x-user-id", user.Login)  // Generic: works for any client
		}
		if user.Email != "" {
			d.md.Set("x-user-email", user.Email)  // Generic: works for any client
		}
		if user.Name != "" {
			d.md.Set("x-user-name", user.Name)  // Generic: works for any client
		}

		// Add organization/tenant context
		if req.PluginContext.OrgID > 0 {
			d.md.Set("x-org-id", fmt.Sprintf("%d", req.PluginContext.OrgID))  // Generic: tenant ID
		}

		// Indicate the client type (useful when multiple client types exist)
		d.md.Set("x-client-type", "grafana")

		logInfof("Query from user: %s (%s) via Grafana", user.Login, user.Email)
	}

	// Refresh OAuth token if using OAuth authentication
	if d.oauthMgr != nil {
		token, err := d.oauthMgr.GetToken(ctx)
		if err != nil {
			logErrorf("OAuth token refresh failed: %v", err)
			response := backend.NewQueryDataResponse()
			for _, q := range req.Queries {
				response.Responses[q.RefID] = backend.DataResponse{
					Error: fmt.Errorf("OAuth token refresh failed: %v", err),
				}
			}
			return response, nil
		}
		d.md.Set("Authorization", fmt.Sprintf("Bearer %s", token))
	}

	// Continue with existing query logic...
	response := backend.NewQueryDataResponse()
	for _, q := range req.Queries {
		res := d.query(ctx, parseQuery(q))
		response.Responses[q.RefID] = res
	}
	return response, nil
}
```

**What Gets Sent to FlightSQL Server:**

gRPC metadata headers (generic, work for any client):
- `x-user-id: alice` - User ID/login (Grafana: username, Python: service user)
- `x-user-email: alice@company.com` - User's email
- `x-user-name: Alice Smith` - Display name
- `x-org-id: 1` - Organization/tenant ID
- `x-client-type: grafana` - Client type (grafana, python, etc.)
- `authorization: Bearer <token>` - OAuth/API key for authentication

**FlightSQL Server Changes** (in Micromegas flight-sql-srv):

**File**: `rust/flight-sql-srv/src/flight_sql_srv.rs`

Add logging of user attribution headers in the request handler:

```rust
// In do_get or do_action methods, extract user identity from metadata
// Uses generic headers that work for any client (Grafana, Python, etc.)
fn log_user_attribution(metadata: &MetadataMap) {
    let user_id = metadata
        .get("x-user-id")
        .and_then(|v| v.to_str().ok());
    let user_email = metadata
        .get("x-user-email")
        .and_then(|v| v.to_str().ok());
    let user_name = metadata
        .get("x-user-name")
        .and_then(|v| v.to_str().ok());
    let org_id = metadata
        .get("x-org-id")
        .and_then(|v| v.to_str().ok());
    let client_type = metadata
        .get("x-client-type")
        .and_then(|v| v.to_str().ok());

    if let Some(user) = user_id.or(user_email) {
        info!(
            "Query from user: id={} email={} name={} org={} client={}",
            user_id.unwrap_or("unknown"),
            user_email.unwrap_or("unknown"),
            user_name.unwrap_or("unknown"),
            org_id.unwrap_or("unknown"),
            client_type.unwrap_or("unknown")
        );
    }
}
```

**Benefits:**

1. **Audit Trail**: Know who ran which queries from any client
2. **Usage Tracking**: Understand which users query the data
3. **Debugging**: Correlate slow queries with specific users
4. **Compliance**: Required for some regulatory environments (SOC2, HIPAA, etc.)
5. **Separate from Auth**: Works with any auth method (OAuth, API key, none)
6. **Multi-Client Support**: Same headers work for Grafana, Python services, etc.

**Example Log Output (FlightSQL Server):**

**From Grafana:**
```
INFO Query from user: id=alice email=alice@company.com name="Alice Smith" org=1 client=grafana
INFO Query executed: SELECT * FROM logs WHERE time > now() - interval '1 hour'
INFO authenticated: subject=grafana-datasource@project.iam.gserviceaccount.com issuer=https://accounts.google.com
```

**From Python Service:**
```
INFO Query from user: id=data-pipeline email=pipeline@company.com name="Data Pipeline" org=5 client=python
INFO Query executed: SELECT * FROM metrics WHERE service='api'
INFO authenticated: subject=pipeline-service@project.iam.gserviceaccount.com issuer=https://accounts.google.com
```

This shows:
- **Authentication**: OAuth service account (client identity)
- **Attribution**: End user who initiated the request (Alice, pipeline, etc.)
- **Client Type**: Where the request came from (Grafana, Python, etc.)

**Important Notes:**

1. **Not for Authentication**: These headers are informational only
   - FlightSQL server still authenticates via OAuth/API key
   - User headers are for logging/attribution only
   - Don't trust headers for access control

2. **Privacy Consideration**:
   - User email/name is sent to FlightSQL server
   - Ensure compliance with privacy policies
   - Can be disabled via configuration if needed

3. **Multi-Client Pattern**:
   - Single client (OAuth service account) used by all end users
   - User attribution headers distinguish individual users
   - Works for Grafana datasources, Python services, etc.
   - Common pattern in enterprise deployments

**For Python Services** (Future Implementation):

Similar pattern can be added to Python micromegas client:

```python
# In Python telemetry client
import os
from micromegas import TelemetryClient

client = TelemetryClient(
    url="http://localhost:9000",
    auth_type="oauth",  # or "api_key"
    # User attribution (optional)
    user_id=os.getenv("USER"),
    user_email=os.getenv("USER_EMAIL"),
    client_type="python"
)
```

This would send the same generic headers: `x-user-id`, `x-user-email`, `x-client-type`

### Phase 3: Testing

#### 3.1 Local Development Testing

**Prerequisites:**
- Auth-enabled flight-sql-srv running locally
- OAuth credentials from Google, Auth0, or Azure AD

**Test Steps:**

1. **Build plugin:**
   ```bash
   cd /home/mad/micromegas/grafana
   npm install
   npm run build
   ```

2. **Start Grafana with plugin:**
   ```bash
   # Symlink plugin to Grafana plugins directory or use docker-compose
   docker-compose up
   ```

3. **Configure datasource in Grafana UI:**
   - Go to Configuration → Data Sources → Add data source
   - Select "Micromegas FlightSQL"
   - Set Host:Port (e.g., `localhost:50051`)
   - Select Auth Type: `oauth2-client-credentials`
   - Enter OIDC Issuer (e.g., `https://accounts.google.com`)
   - Enter Client ID
   - Enter Client Secret (encrypted by Grafana)
   - Enter Audience (if required for Auth0/Azure AD)
   - Enable "Require TLS/SSL" if needed
   - Click "Save & Test"

4. **Verify:**
   - Test connection should succeed
   - Check Grafana logs for OAuth token fetch
   - Create dashboard and execute query
   - Verify query succeeds with OAuth token

#### 3.2 Test Cases

**OAuth Configuration:**
- ✅ Valid credentials → Test connection succeeds
- ✅ Invalid issuer → Clear error message
- ✅ Invalid client ID/secret → Clear error message
- ✅ Missing required fields → Validation error
- ✅ Token cached correctly → Subsequent queries fast
- ✅ Token refresh works → Long-running dashboard updates

**Backward Compatibility:**
- ✅ Existing token (API key) datasources still work
- ✅ Username/password datasources still work
- ✅ Can switch between auth methods
- ✅ Client secret properly encrypted/decrypted

**Integration:**
- ✅ Query execution with OAuth token
- ✅ Multiple concurrent queries
- ✅ Token refresh mid-session
- ✅ Dashboard variables work
- ✅ Alerting works

#### 3.3 Provider-Specific Testing

**Google OAuth:**
- Create service account in GCP
- Generate client credentials
- Test token fetch and query execution

**Auth0:**
- Create M2M application
- Configure API with audience
- Test with audience parameter

**Azure AD:**
- Create app registration
- Generate client secret
- Test with v2.0 endpoint

### Phase 4: Documentation

#### 4.1 Create Setup Guide

**File**: `grafana/docs/oauth-setup.md` (NEW)

Content:
- Prerequisites (service account in identity provider)
- Step-by-step setup for Google, Auth0, Azure AD
- Configuration examples
- Troubleshooting guide
- Security best practices

#### 4.2 Update Plugin README

**File**: `grafana/README.md`

Add OAuth configuration section:
- Brief overview of OAuth support
- Link to detailed setup guide
- When to use OAuth vs API keys
- Migration guide for existing users

#### 4.3 Update Plugin Metadata

**File**: `grafana/src/plugin.json`

```json
{
  "version": "0.2.0",
  "updated": "2025-10-31",
  "info": {
    "description": "FlightSQL datasource with support for OAuth 2.0 client credentials",
    "version": "0.2.0"
  }
}
```

## Implementation Checklist

- [ ] **Frontend (TypeScript/React)**
  - [ ] Update `types.ts` with OAuth fields
  - [ ] Update `ConfigEditor.tsx` with OAuth UI
  - [ ] Add handlers to `utils.ts`
  - [ ] Test configuration saving

- [ ] **Backend (Go) - Grafana Plugin**
  - [ ] Add `golang.org/x/oauth2` dependency (`go get golang.org/x/oauth2`)
  - [ ] Update `config` struct in `flightsql.go`
  - [ ] Create `oauth.go` with token manager using `oauth2` library
  - [ ] Integrate OAuth into `NewDatasource`
  - [ ] Add token refresh in `QueryData`
  - [ ] Add user attribution headers (generic) in `QueryData`
  - [ ] Update `FlightSQLDatasource` struct
  - [ ] Test with local auth-enabled server

- [ ] **Backend (Rust) - FlightSQL Server**
  - [ ] Add user attribution header extraction in `flight_sql_srv.rs`
  - [ ] Add logging of user identity (works for all clients)
  - [ ] Test user attribution logging with Grafana
  - [ ] Document headers for Python client implementation

- [ ] **Testing**
  - [ ] Unit tests for OAuth token manager (Go)
  - [ ] Integration tests with mock OIDC
  - [ ] Manual testing with Google OAuth
  - [ ] Manual testing with Auth0
  - [ ] Backward compatibility testing
  - [ ] Performance testing (token caching)
  - [ ] User attribution testing (verify user headers logged on server for any client)

- [ ] **Documentation**
  - [ ] OAuth setup guide for major providers
  - [ ] Update plugin README
  - [ ] Security documentation
  - [ ] Migration guide
  - [ ] Troubleshooting guide
  - [ ] Document user attribution feature (privacy implications)

## Success Metrics

1. OAuth 2.0 client credentials working with Google, Auth0, Azure AD
2. Zero breaking changes - existing auth methods work unchanged
3. Token fetch completes in <2 seconds
4. Token caching reduces subsequent queries to <10ms overhead
5. Clear error messages for configuration issues
6. Complete setup documentation
7. Backward compatible with all existing datasources
8. **User attribution**: FlightSQL server logs show username/email of end users from any client (Grafana, Python, etc.)

## Security Considerations

1. **Client Secret Storage**:
   - ✅ Encrypted by Grafana's secureJsonData
   - ✅ Never logged or displayed after save
   - ✅ Only decrypted in backend plugin process

2. **Token Caching**:
   - ✅ In-memory only (not persisted)
   - ✅ Cleared on datasource restart
   - ✅ 5-minute expiration buffer

3. **Network Security**:
   - ✅ All OAuth communication over HTTPS
   - ✅ Token endpoint URLs validated
   - ✅ No tokens in logs or error messages

4. **Error Messages**:
   - ✅ Generic errors for auth failures
   - ✅ No sensitive information leaked
   - ✅ Detailed errors only in backend logs

## Timeline

| Phase | Description | Estimated Time |
|-------|-------------|----------------|
| 1 | Frontend configuration | 2-3 hours |
| 2 | Backend OAuth implementation (with `oauth2` lib) | 3-4 hours |
| 3 | Testing | 2-3 hours |
| 4 | Documentation | 2-3 hours |
| **Total** | | **9-13 hours** |

**Note:** Using `golang.org/x/oauth2` reduces implementation time by ~1 hour (simpler code, battle-tested library).

## Related Documents

- [Analytics Server Auth Plan](analytics_auth_plan.md) - Server-side OIDC implementation (complete)
- [OIDC Auth Subplan](oidc_auth_subplan.md) - Detailed OIDC implementation
- [Ingestion Auth Plan](ingestion_auth_plan.md) - Ingestion service authentication (complete)

## Notes

- Plugin uses Grafana's backend datasource architecture
- Authentication happens in Go backend, not frontend
- Frontend only provides configuration UI
- Grafana encrypts sensitive fields automatically
- Same security model as existing token/password fields
- **Using `golang.org/x/oauth2` library for OAuth implementation**:
  - Official Go extended library (maintained by Go team)
  - Automatic token caching and refresh
  - Thread-safe by design
  - Reduces code from ~150 lines to ~50 lines
  - Battle-tested in production systems
- **User attribution via generic gRPC metadata headers**:
  - Plugin sends `x-user-id`, `x-user-email`, `x-user-name`, `x-client-type` headers
  - FlightSQL server logs which end user is making requests
  - Generic headers work for all clients (Grafana, Python services, etc.)
  - Separate from authentication (client authenticates via OAuth/API key)
  - Provides audit trail: who ran which queries from which client
  - Privacy consideration: user email sent to FlightSQL server
