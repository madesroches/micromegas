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

**Host**: The address of your FlightSQL server, as `hostname:port` (e.g. `localhost:50051`). The default FlightSQL port is 50051.

### TLS/SSL Settings

**Require TLS/SSL**: Enable if your server uses TLS encryption. Recommended for any deployment beyond localhost.

## Authentication

Select an authentication type from the **Auth Type** dropdown:

- **none**: No authentication
- **username/password**: Basic authentication with a username and password
- **token**: Static bearer token
- **oauth2-client-credentials**: Enterprise authentication with an identity provider

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
Auth Type: token
Token: dev-key-12345
```

### Production Setup

```
Host: analytics.example.com:50051
TLS/SSL: Enabled
Auth Type: oauth2-client-credentials
OIDC Issuer: https://accounts.google.com
Client ID: grafana-prod@project.iam.gserviceaccount.com
Client Secret: ********
```

### With Metadata

```
Host: analytics.example.com:50051
TLS/SSL: Enabled
Auth Type: token
Token: prod-key-67890
Metadata:
  environment: production
  region: us-east-1
```

## Testing Configuration

Click **Save & Test** to verify. On error, check host/port (connection), credentials (authentication), or TLS settings, depending on the message shown.

## Updating Configuration

To update an existing data source, go to **Configuration** → **Data Sources**, select it, update settings, and click **Save & Test**. Grafana may require you to re-enter secure fields (API keys, client secrets) when updating credentials.

## Next Steps

- [Set up authentication](authentication.md)
- [Start querying data](usage.md)
