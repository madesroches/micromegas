import importlib

from micromegas.cli.config import resolve_connection


def connect():
    """Create FlightSQL client using resolved configuration.

    Priority: env vars > config file (~/.micromegas_oidc_config.json) > defaults.
    """
    cfg = resolve_connection()

    if cfg.python_module_wrapper:
        wrapper_module = importlib.import_module(cfg.python_module_wrapper)
        return wrapper_module.connect()

    if cfg.oidc_issuer and cfg.oidc_client_id:
        from micromegas import oidc_connection

        return oidc_connection.connect(
            uri=cfg.uri,
            issuer=cfg.oidc_issuer,
            client_id=cfg.oidc_client_id,
            client_secret=cfg.oidc_client_secret,
            token_file=cfg.token_file,
            audience=cfg.oidc_audience,
            scope=cfg.oidc_scope,
        )

    from micromegas.flightsql.client import FlightSQLClient

    return FlightSQLClient(cfg.uri)
