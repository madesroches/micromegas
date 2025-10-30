# Configuration

This guide covers configuring the Micromegas datasource in Grafana.

## Adding a Data Source

1. Navigate to **Configuration** → **Data Sources** in Grafana
2. Click **Add data source**
3. Search for and select **Micromegas**
4. Configure the settings (see sections below)
5. Click **Save & Test**

## Connection Settings

### Host Configuration

**Host**: The address of your FlightSQL server

- **Format**: `hostname:port`
- **Example**: `localhost:50051`
- **Production**: `analytics.example.com:50051`

!!! tip "Default Port"
    The default FlightSQL port for Micromegas is 50051.

### TLS/SSL Settings

**Require TLS/SSL**: Enable if your server uses TLS encryption

- **Development**: Usually disabled (localhost connections)
- **Production**: Strongly recommended for security

!!! warning "Security Recommendation"
    Always enable TLS/SSL for production deployments to encrypt data in transit.

### Skip TLS Verification

**Skip TLS Verify**: Only for self-signed certificates

!!! danger "Use Only for Development"
    Never skip TLS verification in production. Use valid certificates instead.

## Authentication

Choose between two authentication methods:

- **API Key**: Simple authentication with a single credential
- **OAuth 2.0 Client Credentials**: Enterprise authentication with identity provider

See the [Authentication Guide](authentication.md) for detailed setup instructions.

## Metadata

**Metadata**: Optional key-value pairs sent to the FlightSQL server

Common use cases:
- Environment identifiers (`env: production`)
- Tenant identifiers (`tenant: acme-corp`)
- Custom headers required by your server

**Format**: Key-value pairs

```
key1: value1
key2: value2
```

!!! note "Query Performance Settings"
    Query timeout and caching are configured at the Grafana dashboard or panel level, not in the datasource settings. See the [Usage Guide](usage.md#query-performance-tips) for query optimization tips.

## Example Configurations

### Development Setup

```
Host: localhost:50051
TLS/SSL: Disabled
Auth Method: API Key
API Key: dev-key-12345
```

### Production Setup

```
Host: analytics.example.com:50051
TLS/SSL: Enabled
Auth Method: OAuth 2.0 Client Credentials
OIDC Issuer: https://accounts.google.com
Client ID: grafana-prod@project.iam.gserviceaccount.com
Client Secret: ********
```

### With Metadata

```
Host: analytics.example.com:50051
TLS/SSL: Enabled
Auth Method: API Key
API Key: prod-key-67890
Metadata:
  environment: production
  region: us-east-1
```

## Testing Configuration

After configuration, click **Save & Test** to verify:

✅ **Success**: "Data source is working"

- Connection successful
- Authentication valid
- Server responding

❌ **Error**: Check error message for details:

- Connection errors → Verify host and port
- Authentication errors → Check credentials
- TLS errors → Verify TLS settings

## Updating Configuration

To update an existing data source:

1. Navigate to **Configuration** → **Data Sources**
2. Select your Micromegas data source
3. Update settings
4. Click **Save & Test**

!!! warning "Credential Updates"
    When updating credentials, Grafana may require you to re-enter secure fields (API keys, client secrets).

## Next Steps

- [Set up authentication](authentication.md)
- [Start querying data](usage.md)
