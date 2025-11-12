# Backward Compatibility Testing

This document describes how to test backward compatibility between the current plugin version and older versions.

## Setup

The Grafana test server can load multiple versions of the plugin simultaneously to test backward compatibility.

### Quick Start

1. Run the setup script to download the old plugin version:
   ```bash
   ./setup-dual-plugin-test.sh
   ```

2. Build the current plugin:
   ```bash
   yarn build
   ```

3. Start the Grafana server with both plugins:
   ```bash
   yarn server
   ```

4. Access Grafana at http://localhost:3000

## Plugin Versions Available

When running `yarn server` after setup, you'll have access to two clearly labeled plugins in the Grafana UI:

- **Micromegas (NEW v0.15.0)** - Current version
  - Plugin ID: `micromegas-micromegas-datasource`
  - Location: `./dist`

- **Micromegas (OLD v0.1.1)** - Old version for compatibility testing
  - Plugin ID: `micromegas-datasource`
  - Location: `./old-plugin-v0.1.1/micromegas-datasource`

The plugin names are clearly labeled with "(NEW v0.15.0)" and "(OLD v0.1.1)" to make them easy to distinguish in the Grafana data source selection dropdown and configuration pages.

## Testing Backward Compatibility

### Cross-Datasource Migration (Panel Data Source Switching)

1. Add both data sources to Grafana:
   - Add "Micromegas (OLD v0.1.1)" as a data source
   - Add "Micromegas (NEW v0.15.0)" as a data source
2. Create a dashboard panel using the old plugin (v0.1.1)
3. Add a SQL query in the panel (e.g., `SELECT * FROM logs WHERE ...`)
4. Switch the panel's data source dropdown from old to new plugin
5. Verify that the query is automatically preserved and migrated
6. The plugin will automatically:
   - Preserve the SQL query text
   - Add `timeFilter: true` and `autoLimit: true` if not present
   - Update to v2 schema

### Saved Dashboard Migration

1. Create a dashboard using the old plugin (v0.1.1)
2. Add data sources for both plugin versions
3. Create queries using the old plugin
4. Switch the data source to the new plugin version
5. Verify that queries are automatically migrated and continue to work

### Variable Queries

1. Create dashboard variables using the old plugin
2. Test that variable queries work correctly
3. Switch to the new plugin version
4. Verify variable definitions are preserved and functional

### Expected Behavior

The current plugin should automatically migrate queries from older versions:

- **v1 queries** (old format without version field)
  - Automatically adds `timeFilter: true` and `autoLimit: true`
  - Migrates to v2 format

- **v2 queries** (current format)
  - No migration needed
  - Uses explicit `timeFilter` and `autoLimit` settings

## Implementation Details

The migration logic is implemented in:

- **`src/types.ts`**: The `migrateQuery()` function handles all query schema migrations
  - V1 → V2: Adds `timeFilter` and `autoLimit` defaults, migrates `queryText` to `query`
  - Tests in `src/types.test.ts`

- **`src/components/QueryEditor.tsx`**: Triggers migration when datasource changes
  - `useEffect` hook watches `datasource.uid` to detect datasource switches
  - Automatically migrates queries when switching from old plugin to new plugin
  - Updates component state with migrated values

This ensures queries are preserved when:
1. Loading saved dashboards with old query format
2. Switching data sources in a panel
3. Using variables with old query format

## Cleanup

To remove the old plugin version:

```bash
rm -rf ./old-plugin-v0.1.1
```

The old plugin directory is already in `.gitignore` and won't be committed to the repository.
