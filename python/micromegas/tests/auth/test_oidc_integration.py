"""Integration tests for OIDC authentication with Google provider.

These tests require:
1. Analytics server running with OIDC enabled
2. GOOGLE_CLIENT_ID environment variable set
3. Valid Google OAuth credentials

To run manually:
    export GOOGLE_CLIENT_ID="your-client-id.apps.googleusercontent.com"
    python3 local_test_env/ai_scripts/start_services_with_oidc.py
    pytest tests/auth/test_oidc_integration.py -v

Note: These tests will open a browser for authentication on first run.
Tokens are cached in ~/.micromegas/tokens.json for subsequent runs.
"""

import os
import sys
import time
from pathlib import Path
from datetime import datetime, timezone

import pytest


@pytest.fixture(scope="module")
def google_client_id():
    """Get Google Client ID from environment."""
    client_id = os.environ.get("GOOGLE_CLIENT_ID")
    if not client_id:
        pytest.skip("GOOGLE_CLIENT_ID not set - skipping Google OIDC integration tests")
    return client_id


@pytest.fixture(scope="module")
def token_file():
    """Get token file path."""
    return str(Path.home() / ".micromegas" / "tokens.json")


@pytest.fixture(scope="module")
def oidc_auth_provider(google_client_id, token_file):
    """Create or load OIDC auth provider with Google.

    This fixture will:
    1. Use existing tokens if available
    2. Open browser for authentication if no tokens exist
    3. Cache tokens for subsequent test runs
    """
    from micromegas.auth import OidcAuthProvider

    issuer = "https://accounts.google.com"
    token_path = Path(token_file)

    # Try to load existing tokens
    if token_path.exists():
        try:
            print(f"\nℹ️  Loading existing tokens from {token_file}")
            auth = OidcAuthProvider.from_file(token_file)

            # Verify token is still valid
            try:
                token = auth.get_token()
                print("✅ Existing tokens are valid")
                return auth
            except Exception as e:
                print(f"⚠️  Token refresh failed: {e}")
                print("   Deleting and re-authenticating...")
                token_path.unlink()
        except Exception as e:
            print(f"⚠️  Failed to load tokens: {e}")

    # Need to authenticate
    print("\n" + "=" * 70)
    print("🌐 Browser-based authentication required")
    print("=" * 70)
    print()
    print("A browser window will open for Google authentication.")
    print("Please sign in with your Google account.")
    print()
    print("This only happens once - tokens will be cached for future test runs.")
    print("To clear tokens: rm ~/.micromegas/tokens.json")
    print()
    print("=" * 70)

    auth = OidcAuthProvider.login(
        issuer=issuer,
        client_id=google_client_id,
        token_file=token_file,
    )

    print(f"✅ Tokens saved to {token_file}")
    return auth


@pytest.fixture(scope="module")
def authenticated_client(oidc_auth_provider):
    """Create FlightSQL client with OIDC authentication."""
    from micromegas.flightsql.client import FlightSQLClient

    # Check if server is running
    import socket
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    result = sock.connect_ex(("127.0.0.1", 50051))
    sock.close()

    if result != 0:
        pytest.skip(
            "Analytics server not running on port 50051. "
            "Start it with: python3 local_test_env/ai_scripts/start_services_with_oidc.py"
        )

    client = FlightSQLClient("grpc://127.0.0.1:50051", auth_provider=oidc_auth_provider)
    return client


def test_google_oidc_authentication(oidc_auth_provider, google_client_id):
    """Test Google OIDC authentication flow."""
    # Verify provider configuration
    assert oidc_auth_provider.issuer == "https://accounts.google.com"
    assert oidc_auth_provider.client_id == google_client_id

    # Get token
    token = oidc_auth_provider.get_token()
    assert token is not None
    assert isinstance(token, str)
    assert len(token) > 0


def test_token_persistence(oidc_auth_provider, token_file):
    """Test that tokens are persisted to file with correct permissions."""
    token_path = Path(token_file)
    assert token_path.exists(), "Token file should exist after authentication"

    # Check file permissions (should be 0600)
    import stat
    mode = token_path.stat().st_mode
    permissions = stat.filemode(mode)
    # Should be -rw------- (owner read/write only)
    assert permissions == "-rw-------", f"Token file permissions should be 0600, got {permissions}"

    # Verify file contents
    import json
    with open(token_file) as f:
        data = json.load(f)

    assert "issuer" in data
    assert "client_id" in data
    assert "token" in data
    assert data["issuer"] == "https://accounts.google.com"

    # Verify token structure
    token_data = data["token"]
    assert "access_token" in token_data
    assert "id_token" in token_data
    assert "expires_at" in token_data


def test_authenticated_query(authenticated_client):
    """Test FlightSQL query with OIDC authentication."""
    now = datetime.now(timezone.utc)

    # Simple test query
    result = authenticated_client.query("SELECT 1 as test", begin=now, end=now)

    assert result is not None
    assert hasattr(result, "shape") or hasattr(result, "__len__")


def test_token_refresh_logic(oidc_auth_provider):
    """Test token refresh behavior."""
    import json

    token_file = oidc_auth_provider.token_file
    with open(token_file) as f:
        data = json.load(f)

    token_info = data["token"]
    expires_at = token_info.get("expires_at", 0)

    # Calculate time until expiration
    time_until_expiry = expires_at - time.time()

    print(f"\nℹ️  Token expires in {int(time_until_expiry)} seconds")

    # Token should have a refresh token for automatic refresh
    assert "refresh_token" in token_info, "Token should have refresh_token for automatic renewal"

    # Get token - should not refresh if more than 5 minutes until expiry
    if time_until_expiry > 300:  # 5 minutes
        token = oidc_auth_provider.get_token()
        assert token is not None
        print("✅ Token is valid and not expiring soon")
    else:
        print("⚠️  Token is expiring soon - refresh will be tested")
        token = oidc_auth_provider.get_token()
        assert token is not None
        print("✅ Token refresh successful")


def test_concurrent_queries(authenticated_client):
    """Test that concurrent queries handle authentication correctly."""
    import concurrent.futures
    from datetime import datetime, timezone

    now = datetime.now(timezone.utc)

    def run_query(i):
        """Run a test query."""
        result = authenticated_client.query(f"SELECT {i} as test", begin=now, end=now)
        return result

    # Run multiple queries concurrently
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
        futures = [executor.submit(run_query, i) for i in range(5)]
        results = [f.result() for f in concurrent.futures.as_completed(futures)]

    # All queries should succeed
    assert len(results) == 5
    for result in results:
        assert result is not None


def test_token_reuse_across_instances(google_client_id, token_file):
    """Test that tokens can be loaded by a new OidcAuthProvider instance."""
    from micromegas.auth import OidcAuthProvider

    # Create a new provider instance from saved tokens
    new_provider = OidcAuthProvider.from_file(token_file)

    # Should be able to get token without authentication
    token = new_provider.get_token()
    assert token is not None
    assert isinstance(token, str)

    # Verify configuration matches
    assert new_provider.issuer == "https://accounts.google.com"
    assert new_provider.client_id == google_client_id


if __name__ == "__main__":
    # Allow running tests directly
    pytest.main([__file__, "-v", "-s"])
