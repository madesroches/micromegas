"""Shared auth resolution for every `WebClient`-based CLI (`screens.py`,
`grants.py`, `groups.py`, `import_keys.py`, and -- via `import_keys.make_client`
-- `setup_telemetry.py`).

`analytics-web-srv` validates OIDC tokens only (no `ProviderBuilder`, no
`DbApiKeyAuthProvider`, no `ApiKeyTable` in that crate) -- it never validates
a static API key, so `resolve_web_auth` never builds a `StaticTokenAuthProvider`
from `conn.api_key_file`. That is `connect_with_profile`'s job, for FlightSQL.
"""

import os
from typing import Optional, Tuple

from micromegas.cli import config


def _no_auth_diagnostic(conn: config.ConnectionConfig) -> str:
    """Diagnostic for a resolved connection that names no complete OIDC pair
    and no `api_key_file` -- nothing configured at all."""
    if conn.profile is not None:
        subject = f"profile '{conn.profile}'"
        hint = (
            "set 'client_id' and 'issuers[0].issuer' in the profile, or "
            "MICROMEGAS_OIDC_ISSUER and MICROMEGAS_OIDC_CLIENT_ID"
        )
    else:
        subject = "~/.micromegas/config.json"
        hint = (
            "set 'client_id' and 'issuers[0].issuer', or add a 'profiles' "
            "map and pass --profile to select a named profile"
        )
    return f"{subject} resolves no auth mechanism: no OIDC issuer/client_id ({hint})"


def _api_key_only_diagnostic(conn: config.ConnectionConfig) -> str:
    """Diagnostic for a resolved connection whose only auth mechanism is a
    static `api_key_file` -- the analytics web API can't validate it."""
    subject = f"profile '{conn.profile}'" if conn.profile is not None else "config file"
    return (
        f"{subject} configures 'api_key_file', but the analytics web API validates "
        "OIDC tokens only -- a static analytics API key works with micromegas-query "
        "(FlightSQL), not with this tool. Set 'client_id' and 'issuers[0].issuer' "
        "on a profile for this server."
    )


def resolve_web_auth(
    profile: Optional[str] = None, config_path=None
) -> Tuple[Optional[object], Optional[str]]:
    """Return `(auth_provider, diagnostic)` for a WebClient.

    `auth_provider` is None exactly when no mechanism resolved, and
    `diagnostic` is then a sentence naming what was missing; when a provider
    resolves, `diagnostic` is None. Deciding whether an unauthenticated
    client is acceptable is the caller's policy, not this function's.

    Resolution, in order:

    1. All three of `MICROMEGAS_OIDC_ISSUER`/`_CLIENT_ID`/`_CLIENT_SECRET` set
       in the environment -> `OidcClientCredentialsProvider.from_env()`, the
       non-interactive CI branch. Checked *before* `resolve_connection`, so a
       complete env credential never fails on profile selection.
    2. `config.resolve_connection(config_path=config_path, profile=profile)`
       -- raises `ProfileError` for an unknown/unselected profile, a
       malformed entry, or a config naming two auth mechanisms. Callers
       surface it; this function never swallows it.
    3. `conn.oidc_issuer and conn.oidc_client_id` ->
       `oidc_connection.load_or_login(...)`, an interactive/cached browser
       login.
    4. Otherwise `(None, diagnostic)` -- naming `api_key_file` specifically
       when that's the only mechanism the connection resolved, since that is
       the case a user configured for `micromegas-query` is most likely to
       hit here.
    """
    issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
    client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
    if issuer and client_id and client_secret:
        from micromegas.auth.oidc import OidcClientCredentialsProvider

        return OidcClientCredentialsProvider.from_env(), None

    conn = config.resolve_connection(config_path=config_path, profile=profile)

    if conn.oidc_issuer and conn.oidc_client_id:
        from micromegas.oidc_connection import load_or_login

        return (
            load_or_login(
                issuer=conn.oidc_issuer,
                client_id=conn.oidc_client_id,
                client_secret=conn.oidc_client_secret,
                token_file=conn.token_file,
                audience=conn.oidc_audience,
                scope=conn.oidc_scope,
            ),
            None,
        )

    if conn.api_key_file:
        return None, _api_key_only_diagnostic(conn)

    return None, _no_auth_diagnostic(conn)
