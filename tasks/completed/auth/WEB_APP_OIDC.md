# OIDC Authentication for Web Applications

This guide shows how to use OIDC authentication in web applications with the Micromegas Python client.

## Desktop App vs Web Application

| Feature | Desktop App | Web Application |
|---------|-------------|-----------------|
| Client Secret | ❌ Not required | ✅ Required (stored on server) |
| Use Case | CLI, local scripts | Web apps, server-side |
| PKCE | ✅ Required | ✅ Recommended |
| Redirect URI | Local only | Configurable (including HTTPS) |
| Security | PKCE protects against code interception | Client secret + PKCE |

## Setup for Web Applications

### 1. Create Web Application OAuth Client

1. Go to [Google Cloud Console](https://console.cloud.google.com/)
2. Navigate to "APIs & Services" → "Credentials"
3. Click "+ CREATE CREDENTIALS" → "OAuth client ID"
4. Select **"Web application"**
5. Configure:
   - **Name**: "Micromegas Web App"
   - **Authorized redirect URIs**:
     - Development: `http://localhost:48080/callback`
     - Production: `https://your-domain.com/auth/callback`
6. Click "Create"
7. **Save both credentials**:
   - Client ID
   - Client Secret (store securely!)

### 2. Store Client Secret Securely

**DO NOT** commit client_secret to version control!

```bash
# Option 1: Environment variable
export OIDC_CLIENT_ID="your-id.apps.googleusercontent.com"
export OIDC_CLIENT_SECRET="your-client-secret-here"
export OIDC_ISSUER="https://accounts.google.com"

# Option 2: Configuration file (add to .gitignore!)
cat > config/oauth_secrets.json << EOF
{
  "google": {
    "client_id": "your-id.apps.googleusercontent.com",
    "client_secret": "your-client-secret-here"
  }
}
EOF

# Option 3: Secret manager (production)
# Use AWS Secrets Manager, Google Secret Manager, etc.
```

### 3. Use in Your Web Application

#### Flask Example

```python
from flask import Flask, redirect, request, session, url_for
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient
import os

app = Flask(__name__)
app.secret_key = os.urandom(24)

# Load credentials from environment (or secret manager)
OIDC_CLIENT_ID = os.environ["OIDC_CLIENT_ID"]
OIDC_CLIENT_SECRET = os.environ["OIDC_CLIENT_SECRET"]
OIDC_ISSUER = os.environ["OIDC_ISSUER"]


@app.route("/login")
def login():
    """Initiate OAuth login flow."""
    # Use your web app's callback URL
    redirect_uri = url_for("callback", _external=True)

    auth = OidcAuthProvider.login(
        issuer=OIDC_ISSUER,
        client_id=OIDC_CLIENT_ID,
        client_secret=OIDC_CLIENT_SECRET,  # Web app includes secret
        token_file=f"/tmp/tokens_{session['user_id']}.json",  # Per-user tokens
        redirect_uri=redirect_uri,
    )

    # Save auth provider to session
    session["authenticated"] = True
    session["token_file"] = f"/tmp/tokens_{session['user_id']}.json"

    return redirect(url_for("dashboard"))


@app.route("/auth/callback")
def callback():
    """OAuth callback handler."""
    # The OidcAuthProvider.login() handles the callback automatically
    # This route just needs to exist in your redirect_uri configuration
    return redirect(url_for("dashboard"))


@app.route("/dashboard")
def dashboard():
    """Protected route - requires authentication."""
    if not session.get("authenticated"):
        return redirect(url_for("login"))

    # Load auth from saved tokens
    client_secret = os.environ["OIDC_CLIENT_SECRET"]
    auth = OidcAuthProvider.from_file(
        session["token_file"],
        client_secret=client_secret,  # Provide secret for refresh
    )

    # Create FlightSQL client
    client = FlightSQLClient(
        "grpc://analytics.example.com:50051",
        auth_provider=auth
    )

    # Use client for queries
    from datetime import datetime, timezone
    now = datetime.now(timezone.utc)
    result = client.query("SELECT * FROM processes LIMIT 10", begin=now, end=now)

    return f"Found {len(result)} processes"


if __name__ == "__main__":
    app.run(port=48080)  # Match your redirect URI port
```

#### Django Example

```python
# views.py
from django.shortcuts import redirect
from django.http import HttpResponse
from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient
import os


def login_view(request):
    """Initiate OAuth login."""
    client_id = os.environ["OIDC_CLIENT_ID"]
    client_secret = os.environ["OIDC_CLIENT_SECRET"]
    issuer = os.environ["OIDC_ISSUER"]

    # Store tokens per user
    token_file = f"/var/app/tokens/{request.user.id}.json"

    auth = OidcAuthProvider.login(
        issuer=issuer,
        client_id=client_id,
        client_secret=client_secret,
        token_file=token_file,
        redirect_uri="http://localhost:8000/auth/callback",
    )

    request.session["token_file"] = token_file
    return redirect("dashboard")


def dashboard_view(request):
    """Protected view."""
    token_file = request.session.get("token_file")
    if not token_file:
        return redirect("login")

    # Load auth with client_secret
    client_secret = os.environ["OIDC_CLIENT_SECRET"]
    auth = OidcAuthProvider.from_file(token_file, client_secret=client_secret)

    # Create client and query
    client = FlightSQLClient(
        "grpc://analytics.example.com:50051",
        auth_provider=auth
    )

    from datetime import datetime, timezone
    now = datetime.now(timezone.utc)
    result = client.query("SELECT * FROM processes LIMIT 10", begin=now, end=now)

    return HttpResponse(f"Found {len(result)} processes")
```

## Key Differences from Desktop App

### Authentication Flow

```python
# Desktop App (CLI) - no client_secret
auth = OidcAuthProvider.login(
    issuer="https://accounts.google.com",
    client_id="desktop-app-id.apps.googleusercontent.com",
    # No client_secret - uses PKCE only
)

# Web Application - with client_secret
auth = OidcAuthProvider.login(
    issuer="https://accounts.google.com",
    client_id="web-app-id.apps.googleusercontent.com",
    client_secret=os.environ["OIDC_CLIENT_SECRET"],  # From server env
    redirect_uri="https://your-domain.com/auth/callback",
)
```

### Loading from File

```python
# Desktop App (CLI)
auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")

# Web Application - must provide client_secret for refresh
client_secret = os.environ["OIDC_CLIENT_SECRET"]
auth = OidcAuthProvider.from_file(
    "/var/app/tokens/user123.json",
    client_secret=client_secret,  # Required for token refresh
)
```

## Security Best Practices

### 1. Never Commit Secrets

```bash
# .gitignore
config/oauth_secrets.json
*.secret
.env
```

### 2. Use Environment Variables

```python
import os

# Good - from environment
client_secret = os.environ["OIDC_CLIENT_SECRET"]

# Bad - hardcoded
client_secret = "GOCSPX-abc123..."  # NEVER DO THIS
```

### 3. Secure Token Storage

```python
# Store tokens per user, not globally
token_file = f"/var/app/tokens/{user_id}.json"

# Set restrictive permissions
import os
os.chmod(token_file, 0o600)  # Owner read/write only
```

### 4. Use HTTPS in Production

```python
# Development
redirect_uri = "http://localhost:48080/callback"

# Production - ALWAYS use HTTPS
redirect_uri = "https://your-domain.com/auth/callback"
```

### 5. Validate Tokens

```python
# The library handles token validation automatically
# But you should also check expiration in your app
auth = OidcAuthProvider.from_file(token_file, client_secret=secret)
try:
    token = auth.get_token()  # Raises if invalid/expired
except Exception as e:
    # Token invalid - redirect to login
    return redirect("login")
```

## Production Deployment

### Using Secret Manager (AWS)

```python
import boto3
import json

def get_client_secret():
    """Fetch client secret from AWS Secrets Manager."""
    client = boto3.client("secretsmanager", region_name="us-east-1")
    response = client.get_secret_value(SecretId="micromegas/google_oauth")
    secret = json.loads(response["SecretString"])
    return secret["client_secret"]

# Use in your app
client_secret = get_client_secret()
auth = OidcAuthProvider.login(
    issuer=os.environ["OIDC_ISSUER"],
    client_id=os.environ["OIDC_CLIENT_ID"],
    client_secret=client_secret,
    # ...
)
```

### Using Google Secret Manager

```python
from google.cloud import secretmanager

def get_client_secret():
    """Fetch client secret from Google Secret Manager."""
    client = secretmanager.SecretManagerServiceClient()
    name = "projects/PROJECT_ID/secrets/google-oauth-secret/versions/latest"
    response = client.access_secret_version(request={"name": name})
    return response.payload.data.decode("UTF-8")
```

## Testing

### Local Testing

```bash
# Set environment variables
export OIDC_CLIENT_ID="web-app-id.apps.googleusercontent.com"
export OIDC_CLIENT_SECRET="your-secret-here"
export OIDC_ISSUER="https://accounts.google.com"

# Run your web app
python app.py
```

### Integration Tests

```python
import pytest
import os

@pytest.fixture
def auth_provider():
    """Create auth provider for testing."""
    return OidcAuthProvider.login(
        issuer=os.environ["OIDC_ISSUER"],
        client_id=os.environ["OIDC_CLIENT_ID"],
        client_secret=os.environ["OIDC_CLIENT_SECRET"],
        token_file="/tmp/test_tokens.json",
    )

def test_authenticated_query(auth_provider):
    """Test query with authentication."""
    from micromegas.flightsql.client import FlightSQLClient
    from datetime import datetime, timezone

    client = FlightSQLClient("grpc://localhost:50051", auth_provider=auth_provider)
    now = datetime.now(timezone.utc)
    result = client.query("SELECT 1 as test", begin=now, end=now)

    assert result is not None
```

## Troubleshooting

### "Invalid client_secret"

- Check that you're using the correct secret for your Web application client
- Verify the secret hasn't been regenerated in Google Console
- Ensure no whitespace/newlines in the secret value

### "Redirect URI mismatch"

- Check that redirect_uri in code matches Google Console exactly
- Include protocol (http/https), port, and path
- Add all redirect URIs to Google Console (dev + production)

### Token refresh fails

- Ensure client_secret is provided when loading from file
- Check that refresh_token is present in token file
- Verify client_secret matches the client_id

## Reference

- [OAuth 2.0 for Web Server Applications](https://developers.google.com/identity/protocols/oauth2/web-server)
- [PKCE RFC 7636](https://tools.ietf.org/html/rfc7636)
- [Google OAuth Client Types](https://developers.google.com/identity/protocols/oauth2#clienttypes)
