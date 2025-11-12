# Grafana Plugin Migration Guide

## Overview

This guide explains how to migrate your Grafana dashboards from the old Micromegas plugin (`micromegas-datasource`) to the new plugin (`micromegas-micromegas-datasource`).

## Why Migration is Needed

The plugin was renamed to follow Grafana's official naming guidelines. When you change a panel's datasource from the old plugin to the new one through the Grafana UI, all query settings are lost because Grafana treats them as completely different plugins.

## Migration Options

### Option 1: Automated Migration Script (Recommended)

Use the provided Python script to automatically migrate all dashboard files.

#### Prerequisites

- Python 3.6 or later
- Dashboard JSON files (exported from Grafana or from provisioning directory)

#### Usage

```bash
# View help
python3 migrate_dashboards.py --help

# Preview changes without saving (dry-run)
python3 migrate_dashboards.py --dry-run dashboard.json

# Migrate a single dashboard file
python3 migrate_dashboards.py dashboard.json

# Migrate all dashboards in a directory
python3 migrate_dashboards.py /path/to/dashboards/

# Recursively migrate all dashboards in subdirectories
python3 migrate_dashboards.py --recursive /path/to/dashboards/

# Specify the UID of your new datasource instance
python3 migrate_dashboards.py --new-uid "your-new-datasource-uid" dashboard.json
```

#### What the Script Does

1. **Scans** dashboard JSON files for references to the old plugin
2. **Updates** all datasource references from `micromegas-datasource` to `micromegas-micromegas-datasource`
3. **Migrates** panel-level datasources, query targets, and template variables
4. **Creates backups** with timestamps before making changes
5. **Preserves** all query settings (SQL, format, timeFilter, autoLimit, etc.)

#### Finding Your New Datasource UID

If you want to update the UID to point to a specific datasource instance:

1. In Grafana, go to Configuration → Data Sources
2. Click on your Micromegas datasource
3. Look at the URL: `https://your-grafana/datasources/edit/<UID>`
4. Use this UID with the `--new-uid` parameter

If you don't specify `--new-uid`, the script will keep the existing UIDs (which may need manual updating in Grafana).

#### Example Output

```
Found 1 dashboard file(s)
New datasource UID: new-micromegas-uid

Processing: my_dashboard.json
  FOUND: 4 datasource references to migrate
    - 2 panels
    - 1 template variables
  BACKUP: Created my_dashboard.backup_20251112_141623.json
  SUCCESS: Dashboard migrated and saved

============================================================
Summary:
  Total files checked: 1
  Files migrated: 1
  Errors: 0
```

### Option 2: Manual Migration via Grafana UI

For individual dashboards, you can manually update the JSON:

1. Open the dashboard in Grafana
2. Click the dashboard settings icon (gear) → JSON Model
3. Use your browser's find/replace (Ctrl+F):
   - Find: `"type": "micromegas-datasource"`
   - Replace: `"type": "micromegas-micromegas-datasource"`
4. Update the `uid` fields if needed to match your new datasource instance
5. Click "Save changes"

### Option 3: Export, Migrate, Re-import

If you don't have direct file access:

1. **Export** dashboards from Grafana (Share → Export → Save to file)
2. **Run** the migration script on exported files
3. **Delete** the old dashboards in Grafana (optional, but recommended to avoid confusion)
4. **Import** the migrated dashboard files back into Grafana

## Migration Workflow

### For File-Based Dashboards (Provisioning)

If your dashboards are provisioned from files:

```bash
# 1. Navigate to your provisioning directory
cd /etc/grafana/provisioning/dashboards/

# 2. Preview changes
python3 /path/to/migrate_dashboards.py --dry-run --recursive .

# 3. Run migration
python3 /path/to/migrate_dashboards.py --recursive .

# 4. Restart Grafana to reload provisioned dashboards
sudo systemctl restart grafana-server
```

### For Database-Stored Dashboards

If your dashboards are stored in Grafana's database:

```bash
# 1. Export dashboards using Grafana API or UI
# (Manual export or use grafana-backup tools)

# 2. Migrate exported files
python3 migrate_dashboards.py exported_dashboards/

# 3. Re-import to Grafana
# (Manual import or use Grafana API)
```

## Post-Migration Steps

1. **Verify** a few migrated dashboards in Grafana
2. **Check** that queries execute correctly
3. **Test** template variables if you use them
4. **Remove** the old datasource once all dashboards are migrated
5. **Delete** backup files once you're confident in the migration

## Rollback

If something goes wrong:

1. The script creates timestamped backups before any changes
2. Find the backup: `dashboard.backup_YYYYMMDD_HHMMSS.json`
3. Copy the backup over the migrated file:
   ```bash
   cp dashboard.backup_20251112_141623.json dashboard.json
   ```

## Troubleshooting

### Script reports "No changes needed"

The dashboard already uses the new plugin or doesn't use Micromegas datasource.

### Error: "Invalid JSON"

The dashboard file is corrupted or not valid JSON. Try exporting again from Grafana.

### Dashboards not updating after migration

If using provisioned dashboards:
- Ensure Grafana has restarted or reloaded
- Check file permissions (Grafana needs read access)
- Check Grafana logs for provisioning errors

### UID mismatches

If you migrated without `--new-uid` and dashboards show "(not found)":
1. Note the UID in the dashboard JSON
2. Either:
   - Update your datasource in Grafana to use that UID, OR
   - Re-run migration with correct `--new-uid`

## Support

For issues with:
- **The migration script**: Check the script output and error messages
- **The plugin itself**: See the main README.md
- **Grafana issues**: Consult Grafana documentation

## Safety Features

The migration script includes several safety features:

- ✅ **Dry-run mode** to preview changes
- ✅ **Automatic backups** before modification
- ✅ **Non-destructive** - only modifies datasource references
- ✅ **Validates JSON** before and after migration
- ✅ **Detailed logging** of all changes
- ✅ **Rollback support** via backup files
