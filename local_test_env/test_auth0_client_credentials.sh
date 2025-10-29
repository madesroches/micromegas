#!/bin/bash
# Test Auth0 OAuth 2.0 client credentials flow
#
# Usage: Set environment variables first, then run this script:
#   export MICROMEGAS_OIDC_ISSUER="https://YOUR_TENANT.us.auth0.com/"
#   export MICROMEGAS_OIDC_CLIENT_ID="YOUR_CLIENT_ID"
#   export MICROMEGAS_OIDC_CLIENT_SECRET="YOUR_CLIENT_SECRET"
#   export MICROMEGAS_OIDC_AUDIENCE="https://api.micromegas.example.com"
#   ./test_auth0_client_credentials.sh

set -e

echo "======================================================================"
echo "Auth0 OAuth 2.0 Client Credentials Test"
echo "======================================================================"
echo

# Check required environment variables
if [ -z "$MICROMEGAS_OIDC_ISSUER" ]; then
    echo "❌ ERROR: MICROMEGAS_OIDC_ISSUER not set"
    echo "   Example: export MICROMEGAS_OIDC_ISSUER=\"https://your-tenant.us.auth0.com/\""
    exit 1
fi

if [ -z "$MICROMEGAS_OIDC_CLIENT_ID" ]; then
    echo "❌ ERROR: MICROMEGAS_OIDC_CLIENT_ID not set"
    exit 1
fi

if [ -z "$MICROMEGAS_OIDC_CLIENT_SECRET" ]; then
    echo "❌ ERROR: MICROMEGAS_OIDC_CLIENT_SECRET not set"
    exit 1
fi

if [ -z "$MICROMEGAS_OIDC_AUDIENCE" ]; then
    echo "❌ ERROR: MICROMEGAS_OIDC_AUDIENCE not set"
    echo "   Example: export MICROMEGAS_OIDC_AUDIENCE=\"https://api.micromegas.example.com\""
    exit 1
fi

echo "Configuration:"
echo "  Issuer: $MICROMEGAS_OIDC_ISSUER"
echo "  Client ID: $MICROMEGAS_OIDC_CLIENT_ID"
echo "  Audience: $MICROMEGAS_OIDC_AUDIENCE"
echo

# Test 1: Fetch token using curl
echo "Test 1: Fetching token from Auth0 (curl)..."
TOKEN_ENDPOINT="${MICROMEGAS_OIDC_ISSUER}oauth/token"

RESPONSE=$(curl -s --request POST \
  --url "$TOKEN_ENDPOINT" \
  --header 'content-type: application/json' \
  --data "{
    \"client_id\": \"$MICROMEGAS_OIDC_CLIENT_ID\",
    \"client_secret\": \"$MICROMEGAS_OIDC_CLIENT_SECRET\",
    \"audience\": \"$MICROMEGAS_OIDC_AUDIENCE\",
    \"grant_type\": \"client_credentials\"
  }")

if echo "$RESPONSE" | grep -q "access_token"; then
    echo "✅ Token fetched successfully via curl"
    ACCESS_TOKEN=$(echo "$RESPONSE" | python3 -c "import sys, json; print(json.load(sys.stdin)['access_token'])")
    echo "   Token prefix: ${ACCESS_TOKEN:0:30}..."
else
    echo "❌ Failed to fetch token via curl"
    echo "   Response: $RESPONSE"
    exit 1
fi
echo

# Test 2: Fetch token using Python client
echo "Test 2: Testing Python OidcClientCredentialsProvider..."
cd /home/mad/micromegas/python/micromegas

python3 <<EOF
import sys
import os
from micromegas.auth import OidcClientCredentialsProvider

try:
    auth = OidcClientCredentialsProvider.from_env()
    token = auth.get_token()
    print(f"✅ Python client fetched token successfully")
    print(f"   Token prefix: {token[:30]}...")

    # Test caching
    token2 = auth.get_token()
    if token == token2:
        print("✅ Token caching works")
    else:
        print("❌ Token caching failed")
        sys.exit(1)
except Exception as e:
    print(f"❌ Python client failed: {e}")
    sys.exit(1)
EOF

echo
echo "======================================================================"
echo "✅ ALL TESTS PASSED"
echo "======================================================================"
echo
echo "Next steps:"
echo "1. Start analytics server with OIDC configuration"
echo "2. Run: poetry run python local_test_env/test_client_credentials.py"
echo "3. Test authenticated FlightSQL queries"
