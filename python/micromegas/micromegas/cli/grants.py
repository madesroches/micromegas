#!/usr/bin/env python3
"""CLI tool for managing DB-backed audience grants.

Talks to `analytics-web-srv`'s `/api/audience-grants` routes over HTTP via
`WebClient` -- never direct Postgres access, the same convention every CLI in
this codebase follows (`screens.py`, `import_keys.py`). Modeled on
`import_keys.py`'s `--url`/`--profile` argument shape rather than
`screens.py`'s local-config-file shape: there is no local state to track here,
just a thin wrapper over two HTTP calls (`create`/`delete`).

**No `list` subcommand.** Listing goes through
`micromegas-query --all "SELECT * FROM list_audience_grants()"` instead,
which as a bonus gives a non-admin caller their own scoped view and an admin
`WHERE`/`ORDER BY` to work with.
"""

import argparse
import os
import sys

import requests

from micromegas.cli import config
from micromegas.cli.version import add_version_argument
from micromegas.web_client import WebClient


def build_auth_provider(args):
    """`OidcClientCredentialsProvider.from_env()` for non-interactive
    service-account use, else an interactive/cached `load_or_login` built from
    the resolved `--profile` connection -- same auth-setup precedent as
    `import_keys.py`/`screens.py`. Returns `None` when no OIDC config is
    available at all (e.g. `--disable-auth` targets), matching `WebClient`'s
    own "no auth provider" support. Raises `config.ProfileError` on an
    unresolvable `--profile`, caught in `main()` alongside `RuntimeError`.
    """
    issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
    client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
    if issuer and client_id and client_secret:
        from micromegas.auth.oidc import OidcClientCredentialsProvider

        return OidcClientCredentialsProvider.from_env()

    conn = config.resolve_connection(profile=args.profile)
    if not conn.oidc_issuer or not conn.oidc_client_id:
        return None

    from micromegas.oidc_connection import load_or_login

    return load_or_login(
        issuer=conn.oidc_issuer,
        client_id=conn.oidc_client_id,
        client_secret=conn.oidc_client_secret,
        token_file=conn.token_file,
        audience=conn.oidc_audience,
        scope=conn.oidc_scope,
    )


def make_client(args):
    """`--url` points at `analytics-web-srv`'s base URL."""
    auth_provider = build_auth_provider(args)
    return WebClient(args.url, auth_provider=auth_provider)


def cmd_create(args):
    """Create (or report the pre-existing) audience grant row."""
    client = make_client(args)
    result = client.create_audience_grant(args.audience, args.axis, args.selector)
    print(
        f"{result['audience']} {result['axis']} {result['selector']} "
        f"(created_by={result['created_by']}, created_at={result['created_at']})"
    )


def cmd_delete(args):
    """Delete one audience grant row, keyed by its natural triple."""
    client = make_client(args)
    client.delete_audience_grant(args.audience, args.axis, args.selector)
    print(f"Deleted: {args.audience} {args.axis} {args.selector}")


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas-grants",
        description="Manage DB-backed audience grants (analytics-web-srv /api/audience-grants)",
        epilog=(
            "To list grants, query the SQL function instead: "
            'micromegas-query --all "SELECT * FROM list_audience_grants()" '
            "-- a non-admin caller gets their own scoped view; an admin gets every row, "
            "filterable with WHERE and ORDER BY."
        ),
    )
    add_version_argument(parser)
    parser.add_argument(
        "--url",
        required=True,
        help="analytics-web-srv's base URL",
    )
    parser.add_argument(
        "--profile",
        help="Named connection profile from ~/.micromegas/config.json (for OIDC auth setup)",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # create
    p_create = subparsers.add_parser("create", help="Create an audience grant")
    p_create.add_argument("audience", help="Audience name ([A-Za-z0-9_-]{1,255})")
    p_create.add_argument("axis", choices=["read", "mint"], help="Grant axis")
    p_create.add_argument("selector", help="'*', 'user:<id>', or 'group:<id>'")
    p_create.set_defaults(func=cmd_create)

    # delete
    p_delete = subparsers.add_parser("delete", help="Delete an audience grant")
    p_delete.add_argument("audience", help="Audience name")
    p_delete.add_argument("axis", choices=["read", "mint"], help="Grant axis")
    p_delete.add_argument("selector", help="'*', 'user:<id>', or 'group:<id>'")
    p_delete.set_defaults(func=cmd_delete)

    args = parser.parse_args()
    try:
        args.func(args)
    except (
        RuntimeError,
        requests.exceptions.RequestException,
        config.ProfileError,
    ) as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
