#!/usr/bin/env python3
"""
Grafana Datasource Migration Script

Migrates datasource configurations from the old Micromegas plugin (micromegas-datasource)
to the new plugin (micromegas-micromegas-datasource).

Usage:
    # Migrate datasources on Grafana server
    python3 migrate_datasources.py --server http://localhost:3000 --token YOUR_API_TOKEN

    # Preview changes without saving (dry-run)
    python3 migrate_datasources.py --server http://localhost:3000 --token YOUR_API_TOKEN --dry-run
"""

import json
import argparse
import sys
from typing import Dict, Any, List, Tuple, Optional
import urllib.request
import urllib.error


OLD_PLUGIN_ID = "micromegas-datasource"
NEW_PLUGIN_ID = "micromegas-micromegas-datasource"


class GrafanaAPIClient:
    """Client for interacting with Grafana HTTP API."""

    def __init__(self, base_url: str, api_token: str):
        """
        Initialize Grafana API client.

        Args:
            base_url: Grafana server URL (e.g., http://localhost:3000)
            api_token: API token for authentication
        """
        self.base_url = base_url.rstrip('/')
        self.api_token = api_token

    def _make_request(self, method: str, path: str, data: Optional[Dict] = None) -> Dict[str, Any]:
        """
        Make an HTTP request to Grafana API.

        Args:
            method: HTTP method (GET, POST, PUT, etc.)
            path: API endpoint path
            data: Optional request body data

        Returns:
            Response data as dict
        """
        url = f"{self.base_url}{path}"
        headers = {
            'Authorization': f'Bearer {self.api_token}',
            'Content-Type': 'application/json',
            'Accept': 'application/json'
        }

        req_data = json.dumps(data).encode('utf-8') if data else None
        request = urllib.request.Request(url, data=req_data, headers=headers, method=method)

        try:
            with urllib.request.urlopen(request) as response:
                response_text = response.read().decode('utf-8')
                if response_text:
                    return json.loads(response_text)
                return {}
        except urllib.error.HTTPError as e:
            error_body = e.read().decode('utf-8')
            raise Exception(f"HTTP {e.code} error: {error_body}")
        except urllib.error.URLError as e:
            raise Exception(f"URL error: {e.reason}")

    def get_datasources(self) -> List[Dict[str, Any]]:
        """
        Get all datasources.

        Returns:
            List of datasource configurations
        """
        return self._make_request('GET', '/api/datasources')

    def get_datasource(self, uid: str) -> Dict[str, Any]:
        """
        Get datasource by UID.

        Args:
            uid: Datasource UID

        Returns:
            Datasource configuration
        """
        return self._make_request('GET', f'/api/datasources/uid/{uid}')

    def update_datasource(self, datasource_id: int, datasource: Dict[str, Any]) -> Dict[str, Any]:
        """
        Update a datasource.

        Args:
            datasource_id: Datasource ID
            datasource: Updated datasource configuration

        Returns:
            Response from Grafana API
        """
        return self._make_request('PUT', f'/api/datasources/{datasource_id}', datasource)


def migrate_datasource(datasource: Dict[str, Any]) -> Tuple[Dict[str, Any], bool]:
    """
    Migrate a datasource configuration from old plugin to new plugin.

    Args:
        datasource: The datasource configuration

    Returns:
        Tuple of (migrated_datasource, was_modified)
    """
    if datasource.get('type') != OLD_PLUGIN_ID:
        return datasource, False

    migrated = datasource.copy()
    migrated['type'] = NEW_PLUGIN_ID

    return migrated, True


def process_datasource(client: GrafanaAPIClient, datasource: Dict[str, Any], dry_run: bool = False) -> bool:
    """
    Process a single datasource.

    Args:
        client: Grafana API client
        datasource: Datasource configuration
        dry_run: If True, don't save changes

    Returns:
        True if changes were made, False otherwise
    """
    uid = datasource.get('uid', 'unknown')
    name = datasource.get('name', 'unknown')
    ds_type = datasource.get('type', 'unknown')

    print(f"\nProcessing: {name} (UID: {uid}, Type: {ds_type})")

    if ds_type != OLD_PLUGIN_ID:
        print(f"  SKIP: Datasource doesn't use {OLD_PLUGIN_ID}")
        return False

    migrated, modified = migrate_datasource(datasource)

    if not modified:
        print(f"  SKIP: No changes needed")
        return False

    print(f"  FOUND: Datasource uses old plugin")
    print(f"    - Changing type from {OLD_PLUGIN_ID} to {NEW_PLUGIN_ID}")

    if dry_run:
        print(f"  DRY-RUN: Would update datasource on server")
        return True

    # Update datasource on server
    try:
        datasource_id = datasource['id']
        response = client.update_datasource(datasource_id, migrated)
        print(f"  SUCCESS: Datasource updated")
        return True
    except Exception as e:
        print(f"  ERROR: Failed to update datasource - {e}")
        return False


def migrate_grafana_datasources(base_url: str, api_token: str, dry_run: bool = False) -> Tuple[int, int]:
    """
    Migrate all datasources on a Grafana server.

    Args:
        base_url: Grafana server URL
        api_token: API token for authentication
        dry_run: If True, don't save changes

    Returns:
        Tuple of (processed_count, error_count)
    """
    print(f"Connecting to Grafana server: {base_url}")

    try:
        client = GrafanaAPIClient(base_url, api_token)
        datasources = client.get_datasources()
    except Exception as e:
        print(f"ERROR: Failed to connect to Grafana server - {e}")
        return 0, 1

    print(f"Found {len(datasources)} datasource(s) on server")

    if dry_run:
        print("\n*** DRY-RUN MODE - No datasources will be modified ***")

    processed_count = 0
    error_count = 0

    for datasource in datasources:
        try:
            if process_datasource(client, datasource, dry_run):
                processed_count += 1
        except Exception as e:
            print(f"\nERROR processing {datasource.get('name', 'unknown')}: {e}")
            error_count += 1

    return processed_count, error_count


def main():
    parser = argparse.ArgumentParser(
        description=f"Migrate Grafana datasources from {OLD_PLUGIN_ID} to {NEW_PLUGIN_ID}",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )
    parser.add_argument(
        "--server",
        required=True,
        help="Grafana server URL (e.g., http://localhost:3000)"
    )
    parser.add_argument(
        "--token",
        required=True,
        help="Grafana API token for authentication"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview changes without saving"
    )

    args = parser.parse_args()

    processed_count, error_count = migrate_grafana_datasources(
        args.server, args.token, args.dry_run
    )

    # Summary
    print(f"\n{'='*60}")
    print(f"Summary:")
    print(f"  Datasources migrated: {processed_count}")
    print(f"  Errors: {error_count}")

    if args.dry_run and processed_count > 0:
        print(f"\nRun without --dry-run to apply changes")

    sys.exit(0 if error_count == 0 else 1)


if __name__ == "__main__":
    main()
