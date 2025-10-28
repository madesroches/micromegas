import importlib
import os
from pathlib import Path


def _get_token_file_path():
    """Get token file path from environment or default."""
    return os.environ.get(
        "MICROMEGAS_TOKEN_FILE", str(Path.home() / ".micromegas" / "tokens.json")
    )


def _load_or_login_oidc(issuer, client_id, client_secret, token_file):
    """Load existing tokens or perform browser login."""
    from micromegas.auth import OidcAuthProvider

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
    )


def _connect_with_oidc():
    """Create FlightSQL client with OIDC authentication."""
    from micromegas.flightsql.client import FlightSQLClient

    issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
    client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
    token_file = _get_token_file_path()

    auth = _load_or_login_oidc(issuer, client_id, client_secret, token_file)

    uri = os.environ.get("MICROMEGAS_ANALYTICS_URI", "grpc://localhost:50051")
    return FlightSQLClient(uri, auth_provider=auth)


def _connect_with_wrapper():
    """Create client using corporate authentication wrapper."""
    wrapper_module_name = os.environ.get("MICROMEGAS_PYTHON_MODULE_WRAPPER")
    wrapper_module = importlib.import_module(wrapper_module_name)
    return wrapper_module.connect()


def connect():
    """Create FlightSQL client with authentication support.

    Uses MICROMEGAS_PYTHON_MODULE_WRAPPER if set (corporate auth),
    otherwise uses OIDC if configured, or falls back to simple connect().
    """
    if os.environ.get("MICROMEGAS_PYTHON_MODULE_WRAPPER"):
        return _connect_with_wrapper()

    if os.environ.get("MICROMEGAS_OIDC_ISSUER") and os.environ.get(
        "MICROMEGAS_OIDC_CLIENT_ID"
    ):
        return _connect_with_oidc()

    import micromegas

    return micromegas.connect()
