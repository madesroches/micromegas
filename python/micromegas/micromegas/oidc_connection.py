"""OIDC connection helpers for FlightSQL client.

This module provides convenient functions for connecting to Micromegas analytics
services using OIDC authentication with explicit arguments (no environment variables).
"""

from pathlib import Path
from typing import Optional

from micromegas.auth import OidcAuthProvider
from micromegas.flightsql.client import FlightSQLClient


def load_or_login(
    issuer: str,
    client_id: str,
    client_secret: Optional[str] = None,
    token_file: Optional[str] = None,
    audience: Optional[str] = None,
    scope: Optional[str] = None,
) -> OidcAuthProvider:
    """Load existing OIDC tokens or perform browser login.

    Attempts to load saved tokens from file first. If no tokens exist or refresh
    fails, opens browser for user authentication.

    Args:
        issuer: OIDC issuer URL (e.g., "https://accounts.google.com")
        client_id: Client ID from identity provider
        client_secret: Client secret (optional, only for Web application clients)
        token_file: Path to save/load tokens. If None, uses default location
            (~/.micromegas/tokens.json)
        audience: API audience/identifier (optional, provider-specific)
        scope: OAuth scopes to request (optional). If None, defaults to
            "openid email profile offline_access". For Azure custom API, use
            "api://{client_id}/.default" to request access tokens for your API.

    Returns:
        OidcAuthProvider: Configured auth provider with valid tokens

    Example:
        >>> auth = load_or_login(
        ...     issuer="https://accounts.google.com",
        ...     client_id="my-client-id.apps.googleusercontent.com",
        ...     token_file="~/.micromegas/tokens.json"
        ... )
        >>>
        >>> # Use with FlightSQL client
        >>> from micromegas.flightsql.client import FlightSQLClient
        >>> client = FlightSQLClient(
        ...     "grpc+tls://analytics.example.com:50051",
        ...     auth_provider=auth
        ... )

    Example (Auth0):
        >>> auth = load_or_login(
        ...     issuer="https://your-tenant.auth0.com",
        ...     client_id="your-auth0-client-id",
        ...     audience="https://your-api.example.com"
        ... )

    Example (Azure AD custom API):
        >>> auth = load_or_login(
        ...     issuer="https://login.microsoftonline.com/{tenant}/v2.0",
        ...     client_id="your-azure-client-id",
        ...     audience="your-azure-client-id",
        ...     scope="api://your-azure-client-id/.default"
        ... )
    """
    if token_file is None:
        token_file = str(Path.home() / ".micromegas" / "tokens.json")

    if Path(token_file).exists():
        try:
            return OidcAuthProvider.from_file(token_file, client_secret=client_secret)
        except Exception as e:
            print(f"Token refresh failed: {e}")
            print("Re-authenticating...")

    print("No saved tokens found. Opening browser for authentication...")
    return OidcAuthProvider.login(
        issuer=issuer,
        client_id=client_id,
        client_secret=client_secret,
        token_file=token_file,
        audience=audience,
        scope=scope,
    )


def connect(
    uri: str,
    issuer: str,
    client_id: str,
    client_secret: Optional[str] = None,
    token_file: Optional[str] = None,
    preserve_dictionary: bool = False,
    audience: Optional[str] = None,
    scope: Optional[str] = None,
) -> FlightSQLClient:
    """Create FlightSQL client with OIDC authentication.

    Convenience function that combines token loading/login and client creation.
    Uses explicit arguments instead of environment variables for better flexibility.

    Args:
        uri: FlightSQL server URI (e.g., "grpc+tls://analytics.example.com:50051")
        issuer: OIDC issuer URL (e.g., "https://accounts.google.com")
        client_id: Client ID from identity provider
        client_secret: Client secret (optional, only for Web application clients)
        token_file: Path to save/load tokens. If None, uses default location
            (~/.micromegas/tokens.json)
        preserve_dictionary: When True, preserve dictionary encoding in Arrow arrays
            for memory efficiency. Defaults to False.
        audience: API audience/identifier (optional, provider-specific)
        scope: OAuth scopes to request (optional). If None, defaults to
            "openid email profile offline_access". For Azure custom API, use
            "api://{client_id}/.default" to request access tokens for your API.

    Returns:
        FlightSQLClient: Configured client ready for queries

    Example:
        >>> # Connect with OIDC authentication
        >>> client = connect(
        ...     uri="grpc+tls://analytics.example.com:50051",
        ...     issuer="https://accounts.google.com",
        ...     client_id="my-client-id.apps.googleusercontent.com",
        ...     client_secret="optional-secret"
        ... )
        >>>
        >>> # Query data
        >>> df = client.query("SELECT * FROM logs")

    Example (Desktop app without client_secret):
        >>> # For CLI tools and desktop apps
        >>> client = connect(
        ...     uri="grpc+tls://analytics.example.com:50051",
        ...     issuer="https://accounts.google.com",
        ...     client_id="desktop-app-id.apps.googleusercontent.com"
        ... )

    Example (Auth0):
        >>> # For Auth0 authentication
        >>> client = connect(
        ...     uri="grpc+tls://analytics.example.com:50051",
        ...     issuer="https://your-tenant.auth0.com",
        ...     client_id="your-auth0-client-id",
        ...     audience="https://your-api.example.com"
        ... )

    Example (Azure AD custom API):
        >>> # For Azure AD custom API with access tokens
        >>> client = connect(
        ...     uri="grpc+tls://analytics.example.com:50051",
        ...     issuer="https://login.microsoftonline.com/{tenant}/v2.0",
        ...     client_id="your-azure-client-id",
        ...     audience="your-azure-client-id",
        ...     scope="api://your-azure-client-id/.default"
        ... )
    """
    auth = load_or_login(issuer, client_id, client_secret, token_file, audience, scope)
    return FlightSQLClient(
        uri, auth_provider=auth, preserve_dictionary=preserve_dictionary
    )
