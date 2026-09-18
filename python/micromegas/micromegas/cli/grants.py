#!/usr/bin/env python3
"""CLI tool for managing DB-backed audience grants.

Talks to `analytics-web-srv`'s `/api/audience-grants` routes over HTTP via
`WebClient` -- never direct Postgres access, the same convention every CLI in
this codebase follows (`screens.py`, `setup_telemetry.py`). Modeled on
`setup_telemetry.py`'s `--url`/`--profile` argument shape rather than
`screens.py`'s local-config-file shape: there is no local state to track here,
just a thin wrapper over two HTTP calls (`create`/`delete`).

**No `list` subcommand.** Listing goes through
`micromegas-query --all "SELECT * FROM list_audience_grants()"` instead,
which as a bonus gives a non-admin caller their own scoped view and an admin
`WHERE`/`ORDER BY` to work with.
"""

import argparse
import sys

import requests

from micromegas.cli import config
from micromegas.cli.version import add_version_argument
from micromegas.cli.web_auth import resolve_web_auth
from micromegas.web_client import WebClient


def build_auth_provider(args):
    """Delegates to `web_auth.resolve_web_auth`, discarding its diagnostic --
    see that function's doc comment for the resolution ladder. Returns `None`
    when no auth mechanism resolves at all (e.g. `--disable-auth` targets),
    matching `WebClient`'s own "no auth provider" support. Raises
    `config.ProfileError` on an unresolvable `--profile`, caught in `main()`
    alongside `RuntimeError`.
    """
    auth_provider, _diagnostic = resolve_web_auth(profile=args.profile)
    return auth_provider


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
