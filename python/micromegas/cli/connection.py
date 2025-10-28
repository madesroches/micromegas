import importlib
import os
from pathlib import Path


def connect():
    """Create FlightSQL client with authentication support.

    Uses MICROMEGAS_PYTHON_MODULE_WRAPPER if set (corporate auth),
    otherwise uses OIDC if configured, or falls back to simple connect().
    """
    # Corporate wrapper takes precedence
    micromegas_module_name = os.environ.get("MICROMEGAS_PYTHON_MODULE_WRAPPER")
    if micromegas_module_name:
        micromegas_module = importlib.import_module(micromegas_module_name)
        return micromegas_module.connect()

    # Try OIDC authentication
    oidc_issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    oidc_client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")

    if oidc_issuer and oidc_client_id:
        from micromegas.auth import OidcAuthProvider
        from micromegas.flightsql.client import FlightSQLClient

        oidc_client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
        token_file = os.environ.get(
            "MICROMEGAS_TOKEN_FILE", str(Path.home() / ".micromegas" / "tokens.json")
        )

        # Try to load existing tokens
        if Path(token_file).exists():
            try:
                auth = OidcAuthProvider.from_file(
                    token_file, client_secret=oidc_client_secret
                )
            except Exception as e:
                # Token file corrupted or refresh failed - re-authenticate
                print(f"Token refresh failed: {e}")
                print("Re-authenticating...")
                auth = OidcAuthProvider.login(
                    issuer=oidc_issuer,
                    client_id=oidc_client_id,
                    client_secret=oidc_client_secret,
                    token_file=token_file,
                )
        else:
            # First time - login with browser
            print("No saved tokens found. Opening browser for authentication...")
            auth = OidcAuthProvider.login(
                issuer=oidc_issuer,
                client_id=oidc_client_id,
                client_secret=oidc_client_secret,
                token_file=token_file,
            )

        uri = os.environ.get("MICROMEGAS_ANALYTICS_URI", "grpc://localhost:50051")
        return FlightSQLClient(uri, auth_provider=auth)

    # Fall back to simple connect (no auth)
    import micromegas

    return micromegas.connect()
