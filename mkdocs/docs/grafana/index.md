# Grafana Datasource Plugin

The Micromegas Grafana datasource plugin enables you to query your telemetry data directly in Grafana dashboards using SQL queries via Apache Arrow FlightSQL protocol.

!!! note "Plugin Origin"
    This plugin was forked from InfluxDB's FlightSQL Grafana plugin and adapted for Micromegas while maintaining FlightSQL protocol compatibility.

## Overview

The plugin provides:

- **SQL Query Builder**: Visual interface for building queries with dropdowns for tables and columns
- **Raw SQL Mode**: Full SQL support for complex queries
- **Multiple Authentication Methods**: Choose between API keys or OAuth 2.0 client credentials
- **Time-Series Visualization**: Native support for Grafana time-series panels
- **Table Support**: Display query results in Grafana tables

## Quick Start

### Prerequisites

- Grafana 9.0 or later
- Micromegas analytics server (flight-sql-srv) running
- Authentication credentials (API key or OAuth 2.0 client credentials)

### Installation

1. Download the latest plugin release
2. Extract to your Grafana plugins directory
3. Restart Grafana
4. Add Micromegas as a data source

For detailed installation instructions, see the [Installation Guide](installation.md).

### Adding a Data Source

1. Navigate to **Configuration** → **Data Sources** in Grafana
2. Click **Add data source**
3. Search for and select **Micromegas**
4. Configure connection settings:
   - **Host**: Your flight-sql-srv address (e.g., `localhost:50051`)
   - **Authentication Method**: Choose API Key or OAuth 2.0
   - **TLS/SSL**: Enable if your server uses TLS
5. Click **Save & Test**

For detailed configuration, see the [Configuration Guide](configuration.md).

## Key Features

### Query Builder

The visual query builder helps you construct SQL queries:

- Select tables from dropdown
- Choose columns with autocomplete
- Add WHERE clauses with + button
- Switch to raw SQL for advanced queries

### Authentication Options

Choose the authentication method that fits your environment:

- **API Keys**: Simple, direct authentication (recommended for quick setup)
- **OAuth 2.0 Client Credentials**: Enterprise-grade authentication with identity provider integration

See the [Authentication Guide](authentication.md) for setup instructions.

### FlightSQL Protocol

The plugin uses Apache Arrow FlightSQL protocol for efficient data transfer:

- High-performance binary protocol
- Efficient columnar data format
- Streaming support for large result sets
- Compatible with any FlightSQL server

## Documentation

- [Installation](installation.md) - Install and set up the plugin
- [Configuration](configuration.md) - Configure connection settings
- [Authentication](authentication.md) - Set up API keys or OAuth 2.0
- [Usage](usage.md) - Query builder and SQL examples

## Development

For development instructions, see the [grafana/DEVELOPMENT.md](https://github.com/madesroches/micromegas/blob/main/grafana/DEVELOPMENT.md) file in the repository.

## Support

For issues or questions:

- GitHub Issues: [madesroches/micromegas/issues](https://github.com/madesroches/micromegas/issues)
- Add label: `grafana-plugin`

## Related Documentation

- [Query Guide](../query-guide/index.md) - SQL query examples and patterns
- [Schema Reference](../query-guide/schema-reference.md) - Database schema documentation
- [Authentication](../admin/authentication.md) - Server-side authentication setup
