#!/usr/bin/env python3

"""
Manual test script for OIDC authentication with any provider

This is a simple interactive script to manually test OIDC authentication.
For automated integration tests, use: pytest tests/auth/test_oidc_integration.py

This script:
1. Authenticates with your OIDC provider (opens browser on first run)
2. Saves tokens to ~/.micromegas/tokens.json
3. Tests a simple FlightSQL query
4. Shows token information

Prerequisites:
1. Analytics server running with OIDC enabled
   (run start_services_with_oidc.py first)
2. Environment variables set:
   - OIDC_ISSUER: Your OIDC provider URL
   - OIDC_CLIENT_ID: Your client ID
   - OIDC_CLIENT_SECRET: (optional) Only needed for Web apps

Usage:
    # Google example
    export OIDC_ISSUER="https://accounts.google.com"
    export OIDC_CLIENT_ID="your-client-id.apps.googleusercontent.com"

    # Auth0 example
    export OIDC_ISSUER="https://yourname.auth0.com/"
    export OIDC_CLIENT_ID="your-client-id"

    python3 test_oidc_auth.py

First run:  Opens browser for authentication
Second run: Uses saved tokens (no browser)
"""

import os
import sys
from pathlib import Path
import time

# Add micromegas to path
micromegas_dir = Path(__file__).parent.parent.parent / "python" / "micromegas"
sys.path.insert(0, str(micromegas_dir))


def check_env():
    """Check environment and prerequisites"""
    client_id = os.environ.get("OIDC_CLIENT_ID")
    if not client_id:
        print("❌ Error: OIDC_CLIENT_ID environment variable not set")
        print()
        print('Set it with: export OIDC_CLIENT_ID="your-client-id"')
        print()
        print("Examples:")
        print('  Google: export OIDC_CLIENT_ID="123-abc.apps.googleusercontent.com"')
        print('  Azure:  export OIDC_CLIENT_ID="<your-app-id>"')
        print('  Okta:   export OIDC_CLIENT_ID="<your-client-id>"')
        print()
        print("See tasks/auth/GOOGLE_OIDC_SETUP.md for setup instructions")
        sys.exit(1)

    # Client secret is OPTIONAL - only needed for Web Application clients
    # Desktop/CLI apps use PKCE (Proof Key for Code Exchange) without a secret
    client_secret = os.environ.get("OIDC_CLIENT_SECRET")

    if client_secret:
        print("🔐 Using PKCE + client_secret (Web Application mode)")
        print()
    else:
        print("🔐 Using PKCE without client_secret (Desktop/CLI mode)")
        print("   This is secure and doesn't require secrets on user's machine")
        print()

    # Check if analytics server is running
    try:
        import socket

        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        result = sock.connect_ex(("127.0.0.1", 50051))
        sock.close()
        if result != 0:
            print("❌ Error: Analytics server not running on port 50051")
            print()
            print("Start it with: python3 start_services_with_oidc.py")
            sys.exit(1)
    except Exception as e:
        print(f"⚠️  Warning: Could not check if analytics server is running: {e}")


def test_oidc_login():
    """Test OIDC login flow"""
    print("=" * 70)
    print("Testing OIDC Authentication")
    print("=" * 70)
    print()

    check_env()

    from micromegas.auth import OidcAuthProvider
    from micromegas.flightsql.client import FlightSQLClient

    # Get OIDC configuration from environment
    issuer = os.environ.get("OIDC_ISSUER")
    if not issuer:
        print("❌ Error: OIDC_ISSUER environment variable not set")
        print()
        print("Set it with: export OIDC_ISSUER=\"<your-provider-issuer-url>\"")
        print()
        print("Examples:")
        print('  Google: export OIDC_ISSUER="https://accounts.google.com"')
        print('  Azure:  export OIDC_ISSUER="https://login.microsoftonline.com/<tenant-id>/v2.0"')
        print('  Okta:   export OIDC_ISSUER="https://<your-domain>.okta.com"')
        sys.exit(1)

    client_id = os.environ["OIDC_CLIENT_ID"]
    client_secret = os.environ.get("OIDC_CLIENT_SECRET")  # Optional
    audience = os.environ.get("OIDC_AUDIENCE") or os.environ.get("MICROMEGAS_OIDC_AUDIENCE")  # Optional but important for Auth0
    scope = os.environ.get("MICROMEGAS_OIDC_SCOPE")  # Optional custom scopes (e.g., for Azure custom API)
    token_file = str(Path.home() / ".micromegas" / "tokens.json")

    print(f"🔐 Configuration:")
    print(f"   Issuer: {issuer}")
    print(f"   Client ID: {client_id}")
    if client_secret:
        print(f"   Client Secret: {'*' * min(20, len(client_secret))} (Web Application mode)")
    else:
        print(f"   Client Secret: (not required - using PKCE)")
    if audience:
        print(f"   Audience: {audience}")
    if scope:
        print(f"   Scope: {scope}")
    print(f"   Token file: {token_file}")
    print()

    # Check if we have saved tokens
    token_file_path = Path(token_file)
    if token_file_path.exists():
        print("✅ Found saved tokens")
        print(f"   File: {token_file}")
        print(f"   Permissions: {oct(token_file_path.stat().st_mode)[-3:]}")
        print()

        # Check if saved token has matching audience
        import json as json_module
        try:
            with open(token_file) as f:
                saved_data = json_module.load(f)
            saved_audience = saved_data.get("audience")

            if audience and saved_audience != audience:
                print(f"⚠️  Saved token has different audience:")
                print(f"   Saved:    {saved_audience}")
                print(f"   Expected: {audience}")
                print("   Deleting and re-authenticating with correct audience...")
                token_file_path.unlink()
                auth = None
            else:
                print("📝 Loading tokens from file...")
                try:
                    auth = OidcAuthProvider.from_file(token_file, client_secret=client_secret)
                    print("✅ Tokens loaded successfully")
                    print()
                except Exception as e:
                    print(f"❌ Failed to load tokens: {e}")
                    print("   Tokens may be expired or corrupted")
                    print("   Deleting and re-authenticating...")
                    token_file_path.unlink()
                    auth = None
        except Exception as e:
            print(f"❌ Failed to read token file: {e}")
            print("   Deleting and re-authenticating...")
            token_file_path.unlink()
            auth = None

        if auth:
            # Try to get token (may trigger refresh)
            print("🔄 Getting current token (may refresh if needed)...")
            try:
                token = auth.get_token()
                print("✅ Token is valid")
                print()
            except Exception as e:
                print(f"❌ Token refresh failed: {e}")
                print("   Re-authenticating...")
                auth = None

    else:
        print("📝 No saved tokens found")
        auth = None

    # If no valid auth, do browser login
    if auth is None:
        print()
        print("🌐 Browser-based authentication required")
        print(f"   Provider: {issuer}")
        print()
        print("To authenticate, you will need to:")
        print("  1. Open the authorization URL in your browser")
        print("  2. Sign in and authorize the application")
        print("  3. The callback will be handled automatically")
        print()
        input("Press Enter when ready to start authentication...")
        print()

        try:
            auth = OidcAuthProvider.login(
                issuer=issuer,
                client_id=client_id,
                client_secret=client_secret,
                token_file=token_file,
                audience=audience,
                scope=scope,
            )
            print()
            print("✅ Authentication successful!")
            print(f"✅ Tokens saved to {token_file}")
            print()
        except Exception as e:
            print(f"❌ Authentication failed: {e}")
            sys.exit(1)

    # Test FlightSQL connection
    print("=" * 70)
    print("Testing FlightSQL Connection with OIDC Auth")
    print("=" * 70)
    print()

    try:
        print("📊 Creating FlightSQL client...")
        client = FlightSQLClient("grpc://127.0.0.1:50051", auth_provider=auth)
        print("✅ Client created")
        print()

        # Try a simple query
        print("🔍 Testing query: SELECT 1 as test...")
        from datetime import datetime, timezone

        # Use a time range (required by the API)
        now = datetime.now(timezone.utc)
        result = client.query("SELECT 1 as test", begin=now, end=now)

        print("✅ Query successful!")
        print(f"   Result type: {type(result)}")
        if hasattr(result, "shape"):
            print(f"   Shape: {result.shape}")
        print()

    except Exception as e:
        print(f"❌ Query failed: {e}")
        print()
        print("Check analytics server logs:")
        print("  tail -f /tmp/analytics.log")
        sys.exit(1)

    # Test token refresh behavior
    print("=" * 70)
    print("Testing Token Information")
    print("=" * 70)
    print()

    try:
        import json

        with open(token_file) as f:
            token_data = json.load(f)

        print("📝 Token file contents:")
        print(f"   Issuer: {token_data.get('issuer')}")
        print(f"   Client ID: {token_data.get('client_id')}")

        token_info = token_data.get("token", {})
        print()
        print("🔑 Token information:")
        print(f"   Token type: {token_info.get('token_type')}")
        print(f"   Scope: {token_info.get('scope')}")
        print(f"   Expires at: {token_info.get('expires_at')}")
        print(f"   Has refresh token: {'refresh_token' in token_info}")
        print()

        # Calculate time until expiration
        expires_at = token_info.get("expires_at", 0)
        if expires_at:
            expires_in = expires_at - time.time()
            hours = int(expires_in // 3600)
            minutes = int((expires_in % 3600) // 60)
            print(f"⏰ Token expires in: {hours}h {minutes}m")
            print(
                "   (Will auto-refresh 5 minutes before expiration on next query)"
            )
            print()

    except Exception as e:
        print(f"⚠️  Could not read token information: {e}")
        print()

    # Success summary
    print("=" * 70)
    print("✅ All tests passed!")
    print("=" * 70)
    print()
    print("Summary:")
    print(f"  ✅ OIDC authentication with {issuer}")
    print("  ✅ Token persistence to file")
    print("  ✅ FlightSQL queries with OIDC auth")
    print("  ✅ Automatic token refresh (will happen when needed)")
    print()
    print("Next steps:")
    print("  - Run this script again to test token reuse (no browser)")
    print("  - Check server logs for auth events: tail -f /tmp/analytics.log")
    print("  - Try CLI tools with OIDC (Phase 3)")
    print()


if __name__ == "__main__":
    test_oidc_login()
