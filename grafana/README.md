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

## Release Process

To create a release package of the Grafana plugin:

1. **Build the plugin package:**
   ```bash
   cd grafana
   ./build-plugin.sh
   ```

   This script will:
   - Install dependencies
   - Build the frontend (TypeScript/React)
   - Build the backend (Go binaries for multiple platforms)
   - Generate the plugin manifest
   - Create a properly structured zip file: `micromegas-micromegas-datasource.zip`

2. **Attach to GitHub release:**
   - Upload the generated `micromegas-micromegas-datasource.zip` to the GitHub release
   - Ensure the zip contains files at the root level (not nested in a `dist/` folder)

3. **Important notes:**
   - The script is idempotent and cleans up old artifacts before building
   - The zip structure must have plugin files at the root for Grafana compatibility
   - Build artifacts are git-ignored and won't be committed

## Support

For issues or questions:

- GitHub Issues: [madesroches/micromegas/issues](https://github.com/madesroches/micromegas/issues)
- Add label: `grafana-plugin`
