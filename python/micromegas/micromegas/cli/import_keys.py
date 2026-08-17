#!/usr/bin/env python3
"""CLI tool for importing legacy env-keyring API keys into the DB-backed
key store (#1411, revised by #1458).

Carries a pre-existing key string forward via `analytics-web-srv`'s
`.../import` HTTP routes -- no `psql`, no direct Postgres network access, an
HTTP-reachable workstation is enough. Both `--table` values go through
`analytics-web-srv`: ingestion itself exposes no key-management HTTP surface
at all (#1458).
"""

import argparse
import json
import os
import sys
from pathlib import Path

import requests

from micromegas.cli import config
from micromegas.cli.version import add_version_argument
from micromegas.web_client import WebClient

# `--var`'s default when omitted: each table's own prefixed legacy var --
# the same names the monolith's per-role `ProviderBuilder` reads
# (`ProviderBuilder::new("MICROMEGAS_INGESTION")` /
# `ProviderBuilder::new("MICROMEGAS_ANALYTICS")`, `rust/monolith/src/main.rs`).
DEFAULT_VAR = {
    "ingestion": "MICROMEGAS_INGESTION_API_KEYS",
    "analytics": "MICROMEGAS_ANALYTICS_API_KEYS",
}

# When the default var above isn't set, `read_keyring` falls back to this
# unprefixed name -- mirroring `ProviderBuilder`'s `{PREFIX}_API_KEYS`-falls-
# back-to-`MICROMEGAS_API_KEYS` convention (`rust/auth/src/default_provider.rs`).
# This is exactly what a split deployment populates: `telemetry-ingestion-srv`
# and `flight-sql-srv` both build with `ProviderBuilder::new("")`
# (`rust/telemetry-ingestion-srv/src/main.rs`,
# `rust/public/src/servers/flight_sql_server.rs`), so the unprefixed var is
# the *only* one they ever read. The fallback applies only to the default --
# an explicit `--var` is used as-is, with no fallback.
FALLBACK_VAR = "MICROMEGAS_API_KEYS"


def read_keyring(args, parser):
    """Parse the legacy keyring's real shape -- a JSON array of
    `{"name": ..., "key": ...}` objects, exactly what `parse_key_ring` reads
    (`rust/auth/src/api_key.rs`'s `KeyRingEntry`), plus an optional per-entry
    `"audience"` field (#1372, AbAC Stage 4) -- from the named env var or a
    file. Returns a list of `(name, key, audience)` triples, in source order;
    `audience` is `None` when the entry carries none.

    A per-entry `"audience"` combined with `--table analytics` is a
    `parser.error`, raised here (not deferred to the import call) so a
    keyring built for ingestion is rejected up front, before any HTTP
    request, rather than partway through a series of live imports.

    For `--source env` with no explicit `--var`, tries the table's prefixed
    default first, then falls back to the unprefixed `MICROMEGAS_API_KEYS`
    (see `DEFAULT_VAR`/`FALLBACK_VAR` above) -- matching `ProviderBuilder`'s
    fallback convention so this recipe works unmodified on both monolith and
    split deployments. An explicit `--var` is used as-is.
    """
    if args.source == "env":
        if args.var:
            var = args.var
            raw = os.environ.get(var)
            if raw is None:
                parser.error(f"environment variable '{var}' is not set")
        else:
            default_var = DEFAULT_VAR[args.table]
            raw = os.environ.get(default_var)
            if raw is None:
                raw = os.environ.get(FALLBACK_VAR)
            if raw is None:
                parser.error(
                    f"neither '{default_var}' nor '{FALLBACK_VAR}' "
                    "environment variable is set"
                )
    else:
        try:
            raw = Path(args.path).read_text(encoding="utf-8")
        except OSError as e:
            parser.error(f"cannot read file '{args.path}': {e}")

    try:
        entries = json.loads(raw)
    except json.JSONDecodeError as e:
        parser.error(f"invalid JSON keyring: {e}")

    if not isinstance(entries, list):
        parser.error('keyring must be a JSON array of {"name", "key"} objects')

    result = []
    for i, entry in enumerate(entries):
        if (
            not isinstance(entry, dict)
            or not isinstance(entry.get("name"), str)
            or not isinstance(entry.get("key"), str)
        ):
            parser.error(
                f"keyring entry {i} must be an object with string 'name' and 'key' fields"
            )
        audience = entry.get("audience")
        if audience is not None and not isinstance(audience, str):
            parser.error(f"keyring entry {i}: 'audience' must be a string")
        if audience is not None and args.table != "ingestion":
            parser.error(
                f"keyring entry {i}: a per-entry 'audience' is only valid with "
                "--table ingestion (analytics_api_keys has no such column)"
            )
        result.append((entry["name"], entry["key"], audience))
    return result


def select_entries(entries, args, parser):
    """Apply `--only`/`--exclude` (already enforced mutually exclusive by
    argparse) to the parsed keyring, in source order."""
    if args.only:
        selected = set(args.only)
        known = {name for name, _, _ in entries}
        missing = selected - known
        if missing:
            parser.error(
                f"--only names not found in keyring: {', '.join(sorted(missing))}"
            )
        return [
            (name, key, audience)
            for name, key, audience in entries
            if name in selected
        ]
    if args.exclude:
        excluded = set(args.exclude)
        known = {name for name, _, _ in entries}
        missing = excluded - known
        if missing:
            parser.error(
                f"--exclude names not found in keyring: {', '.join(sorted(missing))}"
            )
        return [
            (name, key, audience)
            for name, key, audience in entries
            if name not in excluded
        ]
    return entries


def build_auth_provider(args, parser):
    """`OidcClientCredentialsProvider.from_env()` for non-interactive
    service-account use, else an interactive/cached `load_or_login` built
    from the resolved `--profile` connection -- same auth-setup precedent as
    `screens.py`/`query.py`. Returns `None` when no OIDC config is available
    at all (e.g. `--disable-auth` targets), matching `WebClient`'s own "no
    auth provider" support.
    """
    issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
    client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
    if issuer and client_id and client_secret:
        from micromegas.auth.oidc import OidcClientCredentialsProvider

        return OidcClientCredentialsProvider.from_env()

    try:
        conn = config.resolve_connection(profile=args.profile)
    except config.ProfileError as e:
        parser.error(str(e))

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


def make_client(args, parser):
    """`--url` always points at `analytics-web-srv`'s base URL, for both
    `--table` values (#1458) -- ingestion no longer exposes any
    key-management HTTP routes of its own, so there is nothing left for a
    `--table ingestion` run to call directly.
    """
    auth_provider = build_auth_provider(args, parser)
    return WebClient(args.url, auth_provider=auth_provider)


def import_one(client, table, name, key, audience):
    """Calls the table-appropriate import method and returns the parsed
    response dict. Raises `RuntimeError` on a 4xx/5xx (from
    `WebClient`'s `_check_response`), or
    `requests.exceptions.RequestException` on a network-level failure
    (connection reset, DNS failure, timeout) from the underlying
    `session.post` call.

    `audience` is passed through only on the ingestion branch --
    `import_analytics_api_key` takes no such parameter, since
    `analytics_api_keys` has no `audience` column. This is only ever reached
    with a non-`None` `audience` on the analytics branch if the up-front
    guards in `read_keyring`/`main` were somehow bypassed; those guards are
    what actually keep this call correct.
    """
    if table == "ingestion":
        return client.import_ingestion_api_key(name, key, audience)
    return client.import_analytics_api_key(name, key)


def run_import(client, table, entries, cli_audience=None):
    """Imports each `(name, key, audience)` triple, printing one line per key
    and continuing past individual failures rather than aborting the batch.
    Returns `True` if every key imported cleanly (freshly imported, or
    already present and not revoked).

    A per-entry `audience` wins over `cli_audience` (`--audience`); neither
    given leaves `audience` `None`, so the request omits the field entirely
    and the server applies its own default.
    """
    all_ok = True
    for name, key, entry_audience in entries:
        audience = entry_audience if entry_audience is not None else cli_audience
        try:
            result = import_one(client, table, name, key, audience)
        except (RuntimeError, requests.exceptions.RequestException) as e:
            print(f"{name}: error: {e}", file=sys.stderr)
            all_ok = False
            continue

        key_id = result.get("key_id")
        # `analytics_api_keys` rows carry no `audience` at all, so this is
        # blank for `--table analytics`.
        suffix = f", audience={result['audience']}" if "audience" in result else ""
        if result.get("imported"):
            print(f"{name}: imported (key_id={key_id}{suffix})")
        elif result.get("revoked_at"):
            print(f"{name}: already present (revoked) (key_id={key_id}{suffix})")
            all_ok = False
        else:
            print(f"{name}: already present (key_id={key_id}{suffix})")
    return all_ok


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas-import-keys",
        description="Import legacy env-keyring API keys into the DB-backed key store",
    )
    add_version_argument(parser)
    parser.add_argument(
        "--table",
        choices=["ingestion", "analytics"],
        required=True,
        help="Which key table to import into",
    )
    parser.add_argument(
        "--url",
        required=True,
        help=(
            "analytics-web-srv's base URL -- used for both --table values, "
            "since ingestion itself exposes no key-management HTTP routes"
        ),
    )
    parser.add_argument(
        "--source",
        choices=["env", "file"],
        default="env",
        help="Where to read the legacy keyring from (default: env)",
    )
    parser.add_argument(
        "--var",
        help="Env var holding the keyring JSON (--source env; default depends on --table)",
    )
    parser.add_argument(
        "--path",
        help="Path to a file holding the keyring JSON (--source file)",
    )
    select_group = parser.add_mutually_exclusive_group()
    select_group.add_argument(
        "--only",
        nargs="+",
        metavar="NAME",
        help="Import only these keyring entry names",
    )
    select_group.add_argument(
        "--exclude",
        nargs="+",
        metavar="NAME",
        help="Import every keyring entry except these names",
    )
    parser.add_argument(
        "--profile",
        help="Named connection profile from ~/.micromegas/config.json (for OIDC auth setup)",
    )
    parser.add_argument(
        "--audience",
        help=(
            "Write audience to stamp newly-imported ingestion keys with (--table ingestion "
            "only; analytics_api_keys has no such column). Not the OIDC token audience "
            "already configured via --profile/MICROMEGAS_OIDC_AUDIENCE -- unrelated setting, "
            "same flag name coincidence. A keyring entry's own \"audience\" field wins over "
            "this flag. Neither given: the server applies MICROMEGAS_DEFAULT_KEY_AUDIENCE, "
            "falling back to 'public'."
        ),
    )
    args = parser.parse_args()

    if args.source == "file" and not args.path:
        parser.error("--source file requires --path")
    if args.table != "ingestion" and args.audience is not None:
        parser.error("--audience is only valid with --table ingestion")

    entries = read_keyring(args, parser)
    selected = select_entries(entries, args, parser)

    if not selected:
        print("No keys selected to import.")
        return

    client = make_client(args, parser)
    ok = run_import(client, args.table, selected, args.audience)
    if not ok:
        sys.exit(1)


if __name__ == "__main__":
    main()
