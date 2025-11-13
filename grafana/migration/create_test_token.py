#!/usr/bin/env python3
"""
Script to create a Grafana API token for testing the migration script.

Usage:
    python3 create_test_token.py
    python3 create_test_token.py --url http://localhost:3000
    python3 create_test_token.py --url http://localhost:3000 --user admin --password admin
"""

import argparse
import sys
import json
import urllib.request
import urllib.error
import base64


def create_service_account_token(grafana_url: str, admin_user: str, admin_password: str) -> str:
    """
    Create a service account and token for dashboard migration.

    Args:
        grafana_url: Grafana server URL
        admin_user: Admin username
        admin_password: Admin password

    Returns:
        API token string
    """
    grafana_url = grafana_url.rstrip('/')

    # Create basic auth header
    credentials = f"{admin_user}:{admin_password}"
    encoded_credentials = base64.b64encode(credentials.encode('utf-8')).decode('utf-8')
    auth_header = f"Basic {encoded_credentials}"

    print(f"Creating service account for dashboard migration...")
    print(f"Grafana URL: {grafana_url}")

    # Create service account
    sa_data = {
        "name": "dashboard-migration",
        "role": "Editor",
        "isDisabled": False
    }

    sa_request = urllib.request.Request(
        f"{grafana_url}/api/serviceaccounts",
        data=json.dumps(sa_data).encode('utf-8'),
        headers={
            'Authorization': auth_header,
            'Content-Type': 'application/json',
            'Accept': 'application/json'
        },
        method='POST'
    )

    try:
        with urllib.request.urlopen(sa_request) as response:
            sa_response = json.loads(response.read().decode('utf-8'))
    except urllib.error.HTTPError as e:
        error_body = e.read().decode('utf-8')
        print(f"ERROR: Failed to create service account - HTTP {e.code}")
        print(f"Response: {error_body}")
        sys.exit(1)
    except urllib.error.URLError as e:
        print(f"ERROR: Failed to connect to Grafana - {e.reason}")
        sys.exit(1)

    sa_id = sa_response.get('id')
    if not sa_id:
        print(f"ERROR: No service account ID in response")
        print(f"Response: {sa_response}")
        sys.exit(1)

    print(f"Service account created with ID: {sa_id}")

    # Create token for the service account
    token_data = {
        "name": "migration-token",
        "role": "Editor"
    }

    token_request = urllib.request.Request(
        f"{grafana_url}/api/serviceaccounts/{sa_id}/tokens",
        data=json.dumps(token_data).encode('utf-8'),
        headers={
            'Authorization': auth_header,
            'Content-Type': 'application/json',
            'Accept': 'application/json'
        },
        method='POST'
    )

    try:
        with urllib.request.urlopen(token_request) as response:
            token_response = json.loads(response.read().decode('utf-8'))
    except urllib.error.HTTPError as e:
        error_body = e.read().decode('utf-8')
        print(f"ERROR: Failed to create token - HTTP {e.code}")
        print(f"Response: {error_body}")
        sys.exit(1)

    token = token_response.get('key')
    if not token:
        print(f"ERROR: No token in response")
        print(f"Response: {token_response}")
        sys.exit(1)

    return token


def main():
    parser = argparse.ArgumentParser(
        description="Create a Grafana API token for dashboard migration testing"
    )
    parser.add_argument(
        "--url",
        default="http://localhost:3000",
        help="Grafana server URL (default: http://localhost:3000)"
    )
    parser.add_argument(
        "--user",
        default="admin",
        help="Admin username (default: admin)"
    )
    parser.add_argument(
        "--password",
        default="admin",
        help="Admin password (default: admin)"
    )

    args = parser.parse_args()

    try:
        token = create_service_account_token(args.url, args.user, args.password)

        print()
        print("=" * 60)
        print("SUCCESS! API token created:")
        print(token)
        print("=" * 60)
        print()
        print("You can now use this token with the migration script:")
        print()
        print(f'python3 migrate_dashboards.py \\')
        print(f'  --server {args.url} \\')
        print(f'  --token "{token}" \\')
        print(f'  --dry-run')
        print()
        print("Save this token as you won't be able to see it again!")

    except Exception as e:
        print(f"ERROR: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
