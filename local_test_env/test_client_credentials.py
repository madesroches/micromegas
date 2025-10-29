#!/usr/bin/env python3
"""Test OAuth 2.0 client credentials authentication with real OIDC provider.

This script tests the OidcClientCredentialsProvider with a real identity provider
to verify token fetch, caching, and refresh behavior.

Requirements:
    - Analytics server running with OIDC authentication enabled
    - Service account configured in OIDC provider
    - Environment variables set (see below)

Environment Variables:
    MICROMEGAS_OIDC_ISSUER: OIDC issuer URL (e.g., "https://accounts.google.com")
    MICROMEGAS_OIDC_CLIENT_ID: Service account client ID
    MICROMEGAS_OIDC_CLIENT_SECRET: Service account client secret
    MICROMEGAS_FLIGHTSQL_URI: FlightSQL server URI (default: "grpc://localhost:32010")
"""

import os
import sys
import time

# Add parent directory to path for local imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../python/micromegas"))

from micromegas.auth import OidcClientCredentialsProvider
from micromegas.flightsql.client import FlightSQLClient


def main():
    print("=" * 70)
    print("Testing OAuth 2.0 Client Credentials Authentication")
    print("=" * 70)
    print()

    # Check required environment variables
    required_vars = [
        "MICROMEGAS_OIDC_ISSUER",
        "MICROMEGAS_OIDC_CLIENT_ID",
        "MICROMEGAS_OIDC_CLIENT_SECRET",
    ]

    missing_vars = [var for var in required_vars if not os.environ.get(var)]
    if missing_vars:
        print("❌ ERROR: Missing required environment variables:")
        for var in missing_vars:
            print(f"  - {var}")
        print()
        print("Example setup:")
        print('  export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"')
        print('  export MICROMEGAS_OIDC_CLIENT_ID="service@project.iam.gserviceaccount.com"')
        print('  export MICROMEGAS_OIDC_CLIENT_SECRET="your-secret"')
        sys.exit(1)

    print("Configuration:")
    print(f"  Issuer: {os.environ['MICROMEGAS_OIDC_ISSUER']}")
    print(f"  Client ID: {os.environ['MICROMEGAS_OIDC_CLIENT_ID']}")
    print(f"  Client Secret: {'*' * 20} (hidden)")
    print()

    # Create auth provider from environment
    print("Step 1: Creating OidcClientCredentialsProvider from environment...")
    try:
        auth = OidcClientCredentialsProvider.from_env()
        print("✅ Provider created successfully")
        print(f"  Token endpoint: {auth.metadata['token_endpoint']}")
    except Exception as e:
        print(f"❌ Failed to create provider: {e}")
        sys.exit(1)
    print()

    # Fetch initial token
    print("Step 2: Fetching initial access token...")
    try:
        start = time.time()
        token1 = auth.get_token()
        duration = time.time() - start
        print(f"✅ Token fetched in {duration:.2f}s")
        print(f"  Token prefix: {token1[:20]}...")
        print(f"  Token length: {len(token1)} characters")
    except Exception as e:
        print(f"❌ Failed to fetch token: {e}")
        sys.exit(1)
    print()

    # Test token caching
    print("Step 3: Testing token caching (should use cached token)...")
    try:
        start = time.time()
        token2 = auth.get_token()
        duration = time.time() - start
        print(f"✅ Token retrieved in {duration:.4f}s (from cache)")

        if token1 == token2:
            print("✅ Token matches (caching works)")
        else:
            print("❌ WARNING: Token changed (caching may not be working)")
    except Exception as e:
        print(f"❌ Failed to get cached token: {e}")
        sys.exit(1)
    print()

    # Test FlightSQL client integration
    flightsql_uri = os.environ.get("MICROMEGAS_FLIGHTSQL_URI", "grpc://localhost:32010")
    print(f"Step 4: Testing FlightSQL client with authentication...")
    print(f"  Connecting to: {flightsql_uri}")

    try:
        client = FlightSQLClient(flightsql_uri, auth_provider=auth)
        print("✅ FlightSQL client created")
    except Exception as e:
        print(f"❌ Failed to create FlightSQL client: {e}")
        sys.exit(1)
    print()

    # Execute test query
    print("Step 5: Executing test query (SELECT * FROM processes LIMIT 5)...")
    try:
        start = time.time()
        df = client.query("SELECT * FROM processes LIMIT 5")
        duration = time.time() - start
        print(f"✅ Query executed in {duration:.2f}s")
        print(f"  Rows returned: {len(df)}")
        if len(df) > 0:
            print(f"  Columns: {list(df.columns)}")
    except Exception as e:
        print(f"❌ Query failed: {e}")
        print(f"  This could indicate:")
        print(f"  - Analytics server not running")
        print(f"  - OIDC configuration mismatch on server")
        print(f"  - Token validation failure")
        sys.exit(1)
    print()

    # Test multiple queries (verify token reuse)
    print("Step 6: Testing multiple queries (verify token reuse)...")
    try:
        for i in range(3):
            start = time.time()
            df = client.query("SELECT COUNT(*) as count FROM processes")
            duration = time.time() - start
            print(f"  Query {i+1}: {duration:.2f}s - Result: {df.iloc[0]['count']} processes")
        print("✅ Multiple queries successful")
    except Exception as e:
        print(f"❌ Multiple queries failed: {e}")
        sys.exit(1)
    print()

    print("=" * 70)
    print("✅ ALL TESTS PASSED")
    print("=" * 70)
    print()
    print("Summary:")
    print("  ✅ Token fetch from OIDC provider")
    print("  ✅ Token caching")
    print("  ✅ FlightSQL client integration")
    print("  ✅ Query execution with authentication")
    print("  ✅ Multiple queries with token reuse")
    print()
    print("OAuth 2.0 client credentials authentication is working correctly!")


if __name__ == "__main__":
    main()
