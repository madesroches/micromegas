# Installation

This guide covers installing the Micromegas Grafana datasource plugin.

## Prerequisites

- **Grafana**: Version 9.0 or later
- **Micromegas Analytics Server**: flight-sql-srv running and accessible
- **Authentication Credentials**: API key or OAuth 2.0 client credentials

## Installation Methods

### Option 1: From Release (Recommended)

1. Download the latest plugin release from GitHub:
   ```bash
   wget https://github.com/madesroches/micromegas/releases/download/grafana-vX.Y.Z/micromegas-datasource-X.Y.Z.zip
   ```

2. Extract to your Grafana plugins directory:
   ```bash
   # Default plugin directory
   unzip micromegas-datasource-X.Y.Z.zip -d /var/lib/grafana/plugins/

   # Or custom plugin directory
   unzip micromegas-datasource-X.Y.Z.zip -d /path/to/grafana/plugins/
   ```

3. Set proper permissions:
   ```bash
   chown -R grafana:grafana /var/lib/grafana/plugins/micromegas-datasource
   ```

4. Restart Grafana:
   ```bash
   # Systemd
   sudo systemctl restart grafana-server

   # Docker
   docker restart grafana
   ```

5. Verify installation:
   - Navigate to **Configuration** → **Plugins** in Grafana
   - Search for "Micromegas"
   - Plugin should appear in the list

### Option 2: Build from Source

!!! info "Development Setup"
    This method is recommended for development or if you need to customize the plugin.

1. Clone the repository:
   ```bash
   git clone https://github.com/madesroches/micromegas.git
   cd micromegas/grafana
   ```

2. Install dependencies:
   ```bash
   npm install
   # or
   yarn install
   ```

3. Build the plugin:
   ```bash
   # Production build
   npm run build

   # Development build with watch mode
   npm run dev
   ```

4. Build backend binaries (Go):
   ```bash
   mage -v build
   ```

5. Link plugin to Grafana:
   ```bash
   # Create symlink in Grafana plugins directory
   ln -s $(pwd) /var/lib/grafana/plugins/micromegas-datasource
   ```

6. Restart Grafana (see commands above)

For detailed development instructions, see [DEVELOPMENT.md](https://github.com/madesroches/micromegas/blob/main/grafana/DEVELOPMENT.md).

### Option 3: Docker with Plugin

If running Grafana in Docker, mount the plugin directory:

```yaml
# docker-compose.yml
version: '3'
services:
  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    volumes:
      - ./micromegas-datasource:/var/lib/grafana/plugins/micromegas-datasource
    environment:
      - GF_PLUGINS_ALLOW_LOADING_UNSIGNED_PLUGINS=micromegas-datasource
```

!!! warning "Unsigned Plugin"
    During development, you may need to allow unsigned plugins with:
    ```bash
    GF_PLUGINS_ALLOW_LOADING_UNSIGNED_PLUGINS=micromegas-datasource
    ```

## Verify Installation

1. Open Grafana (default: http://localhost:3000)
2. Navigate to **Configuration** → **Plugins**
3. Search for "Micromegas"
4. Plugin should be listed as installed

## Next Steps

Once installed:

1. [Configure the data source](configuration.md)
2. [Set up authentication](authentication.md)
3. [Start querying data](usage.md)
