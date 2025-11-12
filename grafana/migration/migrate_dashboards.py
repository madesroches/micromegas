#!/usr/bin/env python3
"""
Grafana Dashboard Migration Script

Migrates dashboards from the old Micromegas plugin (micromegas-datasource)
to the new plugin (micromegas-micromegas-datasource).

Usage:
    # Migrate a single dashboard file
    python3 migrate_dashboards.py dashboard.json

    # Migrate all dashboards in a directory
    python3 migrate_dashboards.py /path/to/dashboards/

    # Preview changes without saving (dry-run)
    python3 migrate_dashboards.py --dry-run dashboard.json

    # Specify new datasource UID (otherwise will prompt or use existing)
    python3 migrate_dashboards.py --new-uid "abc123xyz" dashboard.json
"""

import json
import argparse
import sys
from pathlib import Path
from typing import Dict, Any, List, Tuple
import shutil
from datetime import datetime


OLD_PLUGIN_ID = "micromegas-datasource"
NEW_PLUGIN_ID = "micromegas-micromegas-datasource"


def migrate_datasource_ref(datasource: Any, new_uid: str = None) -> Tuple[Any, bool]:
    """
    Migrate a datasource reference from old plugin to new plugin.

    Args:
        datasource: The datasource reference (can be string, dict, or other)
        new_uid: Optional UID for the new datasource

    Returns:
        Tuple of (migrated_datasource, was_modified)
    """
    if isinstance(datasource, dict):
        if datasource.get("type") == OLD_PLUGIN_ID:
            migrated = datasource.copy()
            migrated["type"] = NEW_PLUGIN_ID
            if new_uid:
                migrated["uid"] = new_uid
            return migrated, True
    elif isinstance(datasource, str):
        # String references like "${DS_MICROMEGAS}" - keep as-is
        # User will need to update their datasource variables separately
        pass

    return datasource, False


def migrate_panel(panel: Dict[str, Any], new_uid: str = None) -> Tuple[Dict[str, Any], int]:
    """
    Migrate a panel's datasource references.

    Args:
        panel: The panel configuration
        new_uid: Optional UID for the new datasource

    Returns:
        Tuple of (migrated_panel, change_count)
    """
    changes = 0
    migrated_panel = panel.copy()

    # Migrate panel-level datasource
    if "datasource" in migrated_panel:
        new_ds, modified = migrate_datasource_ref(migrated_panel["datasource"], new_uid)
        if modified:
            migrated_panel["datasource"] = new_ds
            changes += 1

    # Migrate targets (queries)
    if "targets" in migrated_panel:
        migrated_targets = []
        for target in migrated_panel["targets"]:
            if isinstance(target, dict):
                migrated_target = target.copy()
                if "datasource" in migrated_target:
                    new_ds, modified = migrate_datasource_ref(migrated_target["datasource"], new_uid)
                    if modified:
                        migrated_target["datasource"] = new_ds
                        changes += 1
                migrated_targets.append(migrated_target)
            else:
                migrated_targets.append(target)
        migrated_panel["targets"] = migrated_targets

    # Recursively migrate panels within row panels
    if migrated_panel.get("type") == "row" and "panels" in migrated_panel:
        nested_panels = []
        for nested_panel in migrated_panel["panels"]:
            migrated_nested, nested_changes = migrate_panel(nested_panel, new_uid)
            nested_panels.append(migrated_nested)
            changes += nested_changes
        migrated_panel["panels"] = nested_panels

    return migrated_panel, changes


def migrate_template_variable(variable: Dict[str, Any], new_uid: str = None) -> Tuple[Dict[str, Any], int]:
    """
    Migrate a template variable's datasource reference.

    Args:
        variable: The template variable configuration
        new_uid: Optional UID for the new datasource

    Returns:
        Tuple of (migrated_variable, change_count)
    """
    changes = 0
    migrated_var = variable.copy()

    if "datasource" in migrated_var:
        new_ds, modified = migrate_datasource_ref(migrated_var["datasource"], new_uid)
        if modified:
            migrated_var["datasource"] = new_ds
            changes += 1

    return migrated_var, changes


def migrate_dashboard(dashboard: Dict[str, Any], new_uid: str = None) -> Tuple[Dict[str, Any], Dict[str, int]]:
    """
    Migrate a complete dashboard from old plugin to new plugin.

    Args:
        dashboard: The dashboard JSON object
        new_uid: Optional UID for the new datasource

    Returns:
        Tuple of (migrated_dashboard, stats_dict)
    """
    migrated = dashboard.copy()
    stats = {
        "panels_migrated": 0,
        "targets_migrated": 0,
        "variables_migrated": 0,
        "total_changes": 0
    }

    # Migrate panels
    if "panels" in migrated:
        migrated_panels = []
        for panel in migrated["panels"]:
            migrated_panel, changes = migrate_panel(panel, new_uid)
            migrated_panels.append(migrated_panel)
            if changes > 0:
                stats["panels_migrated"] += 1
                stats["total_changes"] += changes
        migrated["panels"] = migrated_panels

    # Migrate template variables
    if "templating" in migrated and "list" in migrated["templating"]:
        migrated_vars = []
        for variable in migrated["templating"]["list"]:
            migrated_var, changes = migrate_template_variable(variable, new_uid)
            migrated_vars.append(migrated_var)
            if changes > 0:
                stats["variables_migrated"] += 1
                stats["total_changes"] += changes
        migrated["templating"]["list"] = migrated_vars

    # Update dashboard metadata
    if stats["total_changes"] > 0:
        migrated["version"] = migrated.get("version", 0) + 1
        if "id" in migrated:
            # Remove ID so Grafana treats it as new when importing
            # User can choose to keep or remove this
            pass

    return migrated, stats


def backup_file(file_path: Path) -> Path:
    """Create a timestamped backup of the file."""
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    backup_path = file_path.with_suffix(f".backup_{timestamp}{file_path.suffix}")
    shutil.copy2(file_path, backup_path)
    return backup_path


def process_dashboard_file(file_path: Path, new_uid: str = None, dry_run: bool = False) -> bool:
    """
    Process a single dashboard file.

    Args:
        file_path: Path to the dashboard JSON file
        new_uid: Optional UID for the new datasource
        dry_run: If True, don't save changes

    Returns:
        True if changes were made, False otherwise
    """
    print(f"\nProcessing: {file_path}")

    try:
        with open(file_path, 'r') as f:
            dashboard = json.load(f)
    except json.JSONDecodeError as e:
        print(f"  ERROR: Invalid JSON - {e}")
        return False
    except Exception as e:
        print(f"  ERROR: Failed to read file - {e}")
        return False

    # Check if this dashboard uses the old plugin
    dashboard_str = json.dumps(dashboard)
    if OLD_PLUGIN_ID not in dashboard_str:
        print(f"  SKIP: Dashboard doesn't use {OLD_PLUGIN_ID}")
        return False

    # Migrate the dashboard
    migrated, stats = migrate_dashboard(dashboard, new_uid)

    if stats["total_changes"] == 0:
        print(f"  SKIP: No changes needed")
        return False

    # Print statistics
    print(f"  FOUND: {stats['total_changes']} datasource references to migrate")
    if stats["panels_migrated"] > 0:
        print(f"    - {stats['panels_migrated']} panels")
    if stats["variables_migrated"] > 0:
        print(f"    - {stats['variables_migrated']} template variables")

    if dry_run:
        print(f"  DRY-RUN: Would save changes (use without --dry-run to save)")
        return True

    # Create backup
    try:
        backup_path = backup_file(file_path)
        print(f"  BACKUP: Created {backup_path.name}")
    except Exception as e:
        print(f"  ERROR: Failed to create backup - {e}")
        return False

    # Save migrated dashboard
    try:
        with open(file_path, 'w') as f:
            json.dump(migrated, f, indent=2)
        print(f"  SUCCESS: Dashboard migrated and saved")
        return True
    except Exception as e:
        print(f"  ERROR: Failed to save file - {e}")
        # Restore from backup
        shutil.copy2(backup_path, file_path)
        print(f"  RESTORED: File restored from backup")
        return False


def main():
    parser = argparse.ArgumentParser(
        description=f"Migrate Grafana dashboards from {OLD_PLUGIN_ID} to {NEW_PLUGIN_ID}",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )
    parser.add_argument(
        "path",
        help="Dashboard file or directory containing dashboard files"
    )
    parser.add_argument(
        "--new-uid",
        help="UID of the new datasource instance (optional)"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview changes without saving"
    )
    parser.add_argument(
        "--recursive",
        action="store_true",
        help="Recursively process directories"
    )

    args = parser.parse_args()

    path = Path(args.path)

    if not path.exists():
        print(f"ERROR: Path does not exist: {path}")
        sys.exit(1)

    # Collect dashboard files
    dashboard_files: List[Path] = []

    if path.is_file():
        if path.suffix.lower() == '.json':
            dashboard_files.append(path)
        else:
            print(f"ERROR: File must be a .json file: {path}")
            sys.exit(1)
    elif path.is_dir():
        pattern = "**/*.json" if args.recursive else "*.json"
        dashboard_files = list(path.glob(pattern))
        if not dashboard_files:
            print(f"ERROR: No .json files found in: {path}")
            sys.exit(1)

    print(f"Found {len(dashboard_files)} dashboard file(s)")

    if args.dry_run:
        print("\n*** DRY-RUN MODE - No files will be modified ***")

    if args.new_uid:
        print(f"New datasource UID: {args.new_uid}")
    else:
        print("New datasource UID: Not specified (will keep existing UIDs)")

    # Process files
    processed_count = 0
    error_count = 0

    for file_path in dashboard_files:
        try:
            if process_dashboard_file(file_path, args.new_uid, args.dry_run):
                processed_count += 1
        except Exception as e:
            print(f"\nERROR processing {file_path}: {e}")
            error_count += 1

    # Summary
    print(f"\n{'='*60}")
    print(f"Summary:")
    print(f"  Total files checked: {len(dashboard_files)}")
    print(f"  Files migrated: {processed_count}")
    print(f"  Errors: {error_count}")

    if args.dry_run and processed_count > 0:
        print(f"\nRun without --dry-run to apply changes")

    sys.exit(0 if error_count == 0 else 1)


if __name__ == "__main__":
    main()
