#!/bin/bash
set -e

# Script to download the old plugin version for backward compatibility testing

OLD_VERSION="0.1.1"
OLD_PLUGIN_DIR="./old-plugin-v${OLD_VERSION}"
RELEASE_URL="https://github.com/madesroches/grafana-micromegas-datasource/releases/download/grafana-micromegas-datasource-${OLD_VERSION}/micromegas-datasource.zip"

echo "Setting up dual plugin testing environment..."

# Create directory for old plugin if it doesn't exist
if [ ! -d "$OLD_PLUGIN_DIR" ]; then
    echo "Creating directory for old plugin: $OLD_PLUGIN_DIR"
    mkdir -p "$OLD_PLUGIN_DIR"
fi

# Download old plugin if not already downloaded
if [ ! -f "$OLD_PLUGIN_DIR/plugin.json" ]; then
    echo "Downloading old plugin version ${OLD_VERSION}..."

    # Download the release zip
    curl -L -o "/tmp/old-plugin.zip" "$RELEASE_URL"

    # Extract to old plugin directory
    echo "Extracting old plugin..."
    unzip -o "/tmp/old-plugin.zip" -d "$OLD_PLUGIN_DIR"

    # Clean up
    rm "/tmp/old-plugin.zip"

    # Modify plugin name to make it distinguishable in Grafana UI
    echo "Updating plugin name to make it distinguishable..."
    sed -i 's/"name": "Micromegas",/"name": "Micromegas (OLD v0.1.1)",/' "$OLD_PLUGIN_DIR/micromegas-datasource/plugin.json"

    echo "Old plugin v${OLD_VERSION} downloaded successfully"
else
    echo "Old plugin v${OLD_VERSION} already exists at $OLD_PLUGIN_DIR"
fi

echo ""
echo "Setup complete!"
echo "Old plugin (v${OLD_VERSION}) location: $OLD_PLUGIN_DIR"
echo "Current plugin location: ./dist"
echo ""
echo "Run 'yarn server' to start Grafana with both plugin versions"
