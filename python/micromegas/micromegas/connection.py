"""Profile-aware connect: read `~/.micromegas/config.json` and pick auth.

Kept as a top-level module (not under `cli/`) so `connect_with_profile()` is
discoverable from `help(micromegas)`/`from micromegas import
connect_with_profile` without importing a `cli` submodule -- unlike
`micromegas.connect()`, which never reads the config file, or
`micromegas.oidc_connection.connect()`, which takes OIDC settings as
explicit arguments instead of resolving them from a profile.
"""

from micromegas.cli.config import ProfileError, resolve_connection


def connect_with_profile(
    profile=None, client_entrypoint=None, preserve_dictionary=False
):
    """Create a FlightSQL client from a named profile (or the flat config).

    Reads `~/.micromegas/config.json`, resolving settings with priority
    env vars > active profile (`profile` argument > `MICROMEGAS_PROFILE` >
    `default_profile`) > the flat config > defaults, then connects using
    exactly one of three auth mechanisms, checked in this order:

    1. A static API key, when the resolved config names `api_key_file`.
    2. OIDC, when a complete issuer + client_id pair resolves.
    3. No auth, otherwise.

    A profile that resolves both a static key and a complete OIDC pair
    raises `ProfileError` before either branch runs, so a profile naming a
    key never falls through to a browser login.

    Args:
        profile (str, optional): Named profile to use, i.e. the CLI's
            `--profile` flag. Falls back to `MICROMEGAS_PROFILE`, then
            `default_profile` in the config file, then the flat config
            (no `profiles` map) if neither is set.
        client_entrypoint (str, optional): Explicit label for how this
            client was invoked (e.g. "cli-query"), forwarded to the
            underlying client on all three branches, including the
            static-key one. When omitted, the entrypoint is auto-detected.
            See `FlightSQLClient`'s docstring.
        preserve_dictionary (bool, optional): When True, preserve dictionary
            encoding in Arrow arrays for memory efficiency. Forwarded to
            the underlying client on all three branches. Defaults to False.

    Returns:
        FlightSQLClient: Configured client ready for queries.

    Raises:
        ProfileError: On a profile-selection problem (unknown profile, none
            selected, ...), a profile naming both a static key and OIDC, or
            an unreadable/empty `api_key_file`.

    Example (named profile, static API key):
        >>> # ~/.micromegas/config.json:
        >>> # {"profiles": {"prod": {"uri": "grpc+tls://prod:50051",
        >>> #                        "api_key_file": "~/.micromegas/prod.key"}}}
        >>> client = connect_with_profile("prod")  # doctest: +SKIP

    Example (named profile, OIDC):
        >>> # ~/.micromegas/config.json:
        >>> # {"profiles": {"prod": {"uri": "grpc+tls://prod:50051",
        >>> #                        "client_id": "...",
        >>> #                        "issuers": [{"issuer": "https://idp.example.com"}]}}}
        >>> client = connect_with_profile("prod")  # doctest: +SKIP

    See also:
        `micromegas.connect()` for a plain URI, no config file, no auth.
        `micromegas.oidc_connection.connect()` for explicit-args OIDC with
        no config file.
    """
    cfg = resolve_connection(profile=profile)

    if cfg.api_key_file:
        from micromegas.auth import StaticTokenAuthProvider
        from micromegas.flightsql.client import FlightSQLClient

        try:
            auth_provider = StaticTokenAuthProvider.from_file(cfg.api_key_file)
        except (OSError, ValueError) as e:
            raise ProfileError(
                f"profile key 'api_key_file' ('{cfg.api_key_file}'): {e}"
            ) from e

        return FlightSQLClient(
            cfg.uri,
            auth_provider=auth_provider,
            client_entrypoint=client_entrypoint,
            preserve_dictionary=preserve_dictionary,
        )

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
            preserve_dictionary=preserve_dictionary,
        )

    from micromegas.flightsql.client import FlightSQLClient

    return FlightSQLClient(
        cfg.uri,
        client_entrypoint=client_entrypoint,
        preserve_dictionary=preserve_dictionary,
    )
