#!/usr/bin/env python3
"""CLI tool for managing local groups.

Talks to `analytics-web-srv`'s `/api/groups` routes over HTTP via `WebClient`
-- never direct Postgres access, the same convention `grants.py` follows.
Modeled directly on `grants.py`'s `--url`/`--profile` argument shape.

No `bootstrap` convenience command: taking over from a wildcard-seeded
`admins` group is the two-command sequence
`micromegas-groups add admins user:<you>` then
`micromegas-groups remove admins '*'`, documented as-is rather than folded
into one command.
"""

import argparse
import os
import sys

import requests

from micromegas.cli import config
from micromegas.cli.version import add_version_argument
from micromegas.web_client import WebClient


def build_auth_provider(args):
    """Same resolution `grants.py::build_auth_provider` uses -- see that
    function's doc comment.
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


def cmd_list(args):
    """List every group with its member count."""
    client = make_client(args)
    groups = client.list_groups()
    if not groups:
        print("No groups.")
        return
    for g in groups:
        description = f" -- {g['description']}" if g.get("description") else ""
        print(
            f"{g['name']} ({g['member_count']} members){description} "
            f"(created_by={g['created_by']}, created_at={g['created_at']})"
        )


def cmd_create(args):
    """Create a new, empty group."""
    client = make_client(args)
    result = client.create_group(args.name, description=args.description)
    print(
        f"{result['name']} "
        f"(created_by={result['created_by']}, created_at={result['created_at']})"
    )


def cmd_delete(args):
    """Delete a group. Fails (409) on `admins` or while still referenced."""
    client = make_client(args)
    client.delete_group(args.name)
    print(f"Deleted: {args.name}")


def cmd_members(args):
    """List a group's direct members."""
    client = make_client(args)
    members = client.list_group_members(args.name)
    if not members:
        print("No members.")
        return
    for m in members:
        print(
            f"{m['member']} (created_by={m['created_by']}, created_at={m['created_at']})"
        )


def cmd_add(args):
    """Add a member ('*', 'user:<id>', or 'group:<id>') to a group."""
    client = make_client(args)
    result = client.add_group_member(args.name, args.member)
    print(f"{result['group_name']} += {result['member']}")


def cmd_remove(args):
    """Remove a member from a group."""
    client = make_client(args)
    client.remove_group_member(args.name, args.member)
    print(f"{args.name} -= {args.member}")


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas-groups",
        description="Manage local group membership (analytics-web-srv /api/groups)",
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

    p_list = subparsers.add_parser("list", help="List every group")
    p_list.set_defaults(func=cmd_list)

    p_create = subparsers.add_parser("create", help="Create a new, empty group")
    p_create.add_argument("name", help="Group name ([A-Za-z0-9_-]{1,255})")
    p_create.add_argument("--description", help="Optional description")
    p_create.set_defaults(func=cmd_create)

    p_delete = subparsers.add_parser("delete", help="Delete a group")
    p_delete.add_argument("name", help="Group name")
    p_delete.set_defaults(func=cmd_delete)

    p_members = subparsers.add_parser("members", help="List a group's members")
    p_members.add_argument("name", help="Group name")
    p_members.set_defaults(func=cmd_members)

    p_add = subparsers.add_parser("add", help="Add a member to a group")
    p_add.add_argument("name", help="Group name")
    p_add.add_argument("member", help="'*', 'user:<id>', or 'group:<id>'")
    p_add.set_defaults(func=cmd_add)

    p_remove = subparsers.add_parser("remove", help="Remove a member from a group")
    p_remove.add_argument("name", help="Group name")
    p_remove.add_argument("member", help="'*', 'user:<id>', or 'group:<id>'")
    p_remove.set_defaults(func=cmd_remove)

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
