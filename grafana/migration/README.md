# Grafana Datasource Migration Tool

Tool for migrating Grafana datasources from the old Micromegas plugin (`micromegas-datasource`) to the new plugin (`micromegas-micromegas-datasource`).

## Migration Script

### migrate_datasources.py

Migrates datasource configurations to use the new plugin.

```bash
# Migrate datasources on Grafana server (requires Admin role)
python3 migrate_datasources.py --server http://localhost:3000 --token YOUR_ADMIN_TOKEN

# Preview changes without modifying server
python3 migrate_datasources.py --server http://localhost:3000 --token YOUR_ADMIN_TOKEN --dry-run
```

## Quick Start

### Step 1: Create Admin Token

```bash
# Create admin service account
curl -X POST http://localhost:3000/api/serviceaccounts \
  -u admin:admin \
  -H "Content-Type: application/json" \
  -d '{"name": "datasource-migration", "role": "Admin"}'

# Create token for the service account (note the ID from above response)
curl -X POST http://localhost:3000/api/serviceaccounts/ID/tokens \
  -u admin:admin \
  -H "Content-Type: application/json" \
  -d '{"name": "migration-token"}'
```

Or use the helper script:

```bash
python3 create_test_token.py
```

### Step 2: Run Migration

```bash
# Preview changes first
python3 migrate_datasources.py \
  --server http://localhost:3000 \
  --token "YOUR_ADMIN_TOKEN" \
  --dry-run

# Apply migration
python3 migrate_datasources.py \
  --server http://localhost:3000 \
  --token "YOUR_ADMIN_TOKEN"
```

## What Gets Migrated

The script updates:
- ✅ Datasource type from `micromegas-datasource` to `micromegas-micromegas-datasource`
- ✅ All configuration settings are preserved
- ✅ Datasource UID remains unchanged

Dashboards automatically work after datasource migration because they reference datasources by UID.

## Requirements

- Python 3.6 or later
- Admin access to Grafana instance
- No external dependencies (uses only Python standard library)

## Troubleshooting

### Authentication failed
- Verify API token has Admin role
- Check token hasn't expired
- Ensure service account is enabled

### Connection refused
- Verify Grafana is running: `curl http://localhost:3000/api/health`
- Check firewall settings
- Confirm correct server URL

### No datasources found
- Verify you have Micromegas datasources configured
- Check datasources use `micromegas-datasource` type

## Example Output

```
Connecting to Grafana server: http://localhost:3000
Found 1 datasource(s) on server

Processing: local (UID: ef3xl8te12rcwe, Type: micromegas-datasource)
  FOUND: Datasource uses old plugin
    - Changing type from micromegas-datasource to micromegas-micromegas-datasource
  SUCCESS: Datasource updated

============================================================
Summary:
  Datasources migrated: 1
  Errors: 0
```

## Verification

After migration, verify the datasource type:

```bash
curl -H "Authorization: Bearer YOUR_TOKEN" \
  http://localhost:3000/api/datasources | python3 -m json.tool
```

Look for `"type": "micromegas-micromegas-datasource"` in the output.

## Security Notes

- Never commit API tokens to version control
- Delete tokens when no longer needed
- Use service accounts with minimal required permissions (Admin for datasource migration)

## Helper Scripts

### create_test_token.py

Creates a Grafana API token for testing.

```bash
# Create token for local Grafana (default: http://localhost:3000)
python3 create_test_token.py

# Create token for custom Grafana instance
python3 create_test_token.py --url http://grafana.example.com --user admin --password secret
```
