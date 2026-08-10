from micromegas.cli.config import resolve_connection


def connect(profile=None, client_entrypoint=None):
    """Create FlightSQL client using resolved configuration.

    Priority: env vars > active profile (or config file) > defaults.
    """
    cfg = resolve_connection(profile=profile)

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
            client_entrypoint=client_entrypoint,
        )

    from micromegas.flightsql.client import FlightSQLClient

    return FlightSQLClient(cfg.uri, client_entrypoint=client_entrypoint)
