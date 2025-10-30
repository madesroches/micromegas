# Grafana Micromegas Datasource

A Grafana datasource plugin for querying [Micromegas](https://github.com/madesroches/micromegas) telemetry data using Apache Arrow FlightSQL protocol.

!!! note
    This plugin was forked from InfluxDB's FlightSQL Grafana plugin and adapted for Micromegas while maintaining FlightSQL protocol compatibility.

## Quick Start

```bash
cd grafana
npm install
npm run build
```

## Documentation

For complete documentation, see:

**[📚 Grafana Plugin Documentation](https://madesroches.github.io/micromegas/docs/grafana/)**

- [Installation Guide](https://madesroches.github.io/micromegas/docs/grafana/installation/) - Install and set up the plugin
- [Configuration Guide](https://madesroches.github.io/micromegas/docs/grafana/configuration/) - Configure connection settings
- [Authentication Guide](https://madesroches.github.io/micromegas/docs/grafana/authentication/) - Set up API keys or OAuth 2.0
- [Usage Guide](https://madesroches.github.io/micromegas/docs/grafana/usage/) - Query builder and SQL examples
- [Troubleshooting Guide](https://madesroches.github.io/micromegas/docs/grafana/troubleshooting/) - Common issues and solutions

## Development

For development instructions, see [DEVELOPMENT.md](DEVELOPMENT.md).

## Support

For issues or questions:

- GitHub Issues: [madesroches/micromegas/issues](https://github.com/madesroches/micromegas/issues)
- Add label: `grafana-plugin`
