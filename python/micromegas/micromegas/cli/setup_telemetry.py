#!/usr/bin/env python3
"""CLI tool that mints a personal ingestion API key and prints the OTLP
exporter env vars needed to point a user's own telemetry at a micromegas
deployment (AbAC Stage 6, #1374).

Named `micromegas-setup-telemetry`, not `micromegas-mint-key` or similar --
from the user's point of view this script sets up telemetry transmission
("send my data"), so the server-side term "ingestion" stays out of the
user-facing name.

Auth reuses `import_keys.py::build_auth_provider`/`make_client`'s exact shape
verbatim: client-credentials env vars first, else `config.resolve_connection`
-> `oidc_connection.load_or_login` (the interactive loopback-redirect browser
login on first run, a cached token after that). No new OIDC code here.
"""

import argparse
import os
import stat
import sys
from pathlib import Path

import requests

from micromegas.cli import config
from micromegas.cli.import_keys import make_client
from micromegas.cli.version import add_version_argument

# Re-exported so tests can call `setup_telemetry.make_client` directly.
__all__ = ["make_client", "main"]


def resolve_otlp_endpoint(args, parser):
    """`--otlp-endpoint`, or derived from `MICROMEGAS_TELEMETRY_URL` when that
    env var is set (mirroring `local_test_env/claude_code_otel.py`'s own
    derivation verbatim: `f"{base}/ingestion/otlp"`) -- a required flag only
    when neither is available.

    `MICROMEGAS_TELEMETRY_URL` is the repo's established ingestion-endpoint
    convention; it is a different service/port from `--url`
    (`analytics-web-srv`'s own base URL), so `--otlp-endpoint` is never
    derived from `--url`.
    """
    if args.otlp_endpoint:
        return args.otlp_endpoint
    base = os.environ.get("MICROMEGAS_TELEMETRY_URL")
    if not base:
        parser.error(
            "--otlp-endpoint is required (or set MICROMEGAS_TELEMETRY_URL, "
            "from which it is derived as '{base}/ingestion/otlp')"
        )
    return f"{base.rstrip('/')}/ingestion/otlp"


def resolve_audience(client, args, parser, my_audiences):
    """Applies the three-way `--audience` prefixing rule (§6):

    - `--audience X` already in `my_audiences["audiences"]` (the caller has
      a real grant for it): used verbatim, never prefixed.
    - `--audience X` not in that list, non-admin caller: a fresh claim (§4a)
      -- minted as `f"{mint_prefix}{X}"` instead of `X`, printing the
      resolved full name to stderr so the caller sees what was actually
      claimed.
    - `--audience X`, admin caller: never prefixed -- deliberate operational
      naming. `is_admin` (not an empty `audiences` list, which is not
      reliably `[]` for an admin) is what tells the two cases apart.
    - `--audience` omitted, non-admin: exactly one match in `audiences` is
      used silently; more than one is an error naming the choices; none is
      an error pointing at claiming a fresh name or asking an admin.
    - `--audience` omitted, admin: an error asking for one explicitly --
      `audiences` is not a reliable "nothing mintable yet" signal for an
      admin.

    Returns the resolved audience name. The admin branch no longer decides
    or reports whether the name is brand-new (#1510): the mint route itself
    now runs that same ownership check server-side and claims a brand-new
    audience for an admin caller in the same request (`MintResponse`'s new
    `claimed` field says so), so this helper no longer needs to page through
    `list_ingestion_api_keys`/`list_audience_grants` to decide it client-side.
    """
    is_admin = my_audiences["is_admin"]
    audiences = my_audiences["audiences"]
    mint_prefix = my_audiences.get("mint_prefix")

    if args.audience is None:
        if is_admin:
            parser.error(
                "--audience is required for an admin caller (pick an audience name "
                "explicitly; an empty mintable-audience list means nothing for an admin)"
            )
        if len(audiences) == 1:
            return audiences[0]
        if len(audiences) > 1:
            parser.error(
                "multiple mintable audiences found ("
                + ", ".join(sorted(audiences))
                + "); pick one with --audience"
            )
        parser.error(
            "no mintable audience found for this caller; claim a fresh one with "
            "--audience <new-name>, or ask an admin for a grant"
        )

    if args.audience in audiences:
        return args.audience

    if is_admin:
        # No list calls: the server now decides (and claims) a brand-new audience for an
        # admin caller as part of the mint request itself (§4).
        return args.audience

    if mint_prefix is None:
        parser.error(
            f"cannot claim a fresh audience {args.audience!r}: this caller has no "
            "email to claim with (pass --audience for an audience you already have "
            "a grant for)"
        )
    resolved = f"{mint_prefix}{args.audience}"
    print(f"claiming fresh audience: {resolved}", file=sys.stderr)
    return resolved


def write_env_file(path, content):
    """Writes `content` to `path` with mode `0o600` (parent directory `0o700`
    if it doesn't already exist) -- mirroring
    `OidcAuthProvider.save()`'s token-cache permissions. `content` holds a
    standing `Authorization: Bearer` credential, so it must never land at
    the process umask.
    """
    target = Path(path)
    parent = target.parent
    parent_existed = parent.exists()
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not parent_existed:
        # `mkdir(mode=...)` is subject to umask on some platforms, so re-assert
        # explicitly -- but only for a directory this call created; a pre-existing
        # directory's permissions are the caller's own business.
        parent.chmod(0o700)
    fd = os.open(str(target), os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
    try:
        # Belt-and-suspenders: `mkdir(mode=...)`/`os.open(..., 0o600)` are
        # subject to umask on some platforms, so re-assert the permissions
        # explicitly rather than trusting the create call alone.
        os.fchmod(fd, stat.S_IRUSR | stat.S_IWUSR)
        os.write(fd, content.encode("utf-8"))
    finally:
        os.close(fd)


def format_env_exports(key, otlp_endpoint):
    """`OTEL_EXPORTER_OTLP_PROTOCOL`/`_ENDPOINT`/`_HEADERS` shell export
    lines. The protocol export is required because micromegas exposes OTLP
    over HTTP only, so an SDK defaulting to gRPC would otherwise fail to
    reach the endpoint. `Authorization=Bearer <key>`, capitalized with `=`,
    matches the already-documented OTLP header format
    (`mkdocs/docs/otlp/index.md`).
    """
    return (
        "export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf\n"
        f"export OTEL_EXPORTER_OTLP_ENDPOINT={otlp_endpoint}\n"
        f'export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer {key}"\n'
    )


def build_parser():
    parser = argparse.ArgumentParser(
        prog="micromegas-setup-telemetry",
        description=(
            "Mint a personal ingestion API key and print the OTLP exporter "
            "env vars needed to send your own telemetry to a micromegas deployment"
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
    parser.add_argument(
        "--name",
        required=True,
        help="Name for the minted key (e.g. this machine's hostname)",
    )
    parser.add_argument(
        "--audience",
        help=(
            "Write audience to mint the key under: a fresh name to claim (a non-admin "
            "caller's claim is minted under a namespace derived from their own email; "
            "see the docs), an existing audience you already have a grant for, or "
            "omitted entirely to resolve one via GET .../audience-grants/my-audiences"
        ),
    )
    parser.add_argument(
        "--otlp-endpoint",
        help=(
            "OTLP HTTP endpoint to export to. Defaults to "
            "'{MICROMEGAS_TELEMETRY_URL}/ingestion/otlp' when that env var is set."
        ),
    )
    parser.add_argument(
        "--env-file",
        help="Write the OTEL_EXPORTER_OTLP_* exports to this file instead of stdout",
    )
    return parser


def run(args, parser):
    client = make_client(args, parser)

    # Called unconditionally, even when --audience is passed explicitly: applying the
    # three-way prefix rule needs both `mint_prefix` and the caller's own `audiences`
    # list from this one response. A useful side effect: a knob-off caller gets a clear
    # 403 up front, instead of a confusing denial only once the mint itself is attempted.
    my_audiences = client.my_audiences()

    audience = resolve_audience(client, args, parser, my_audiences)

    # Resolved before the mint so a purely local validation error (e.g. missing
    # --otlp-endpoint/MICROMEGAS_TELEMETRY_URL) can never strand an already-minted,
    # never-retrievable-again key.
    otlp_endpoint = resolve_otlp_endpoint(args, parser)

    result = client.mint_ingestion_api_key(args.name, audience)

    print(
        f"minted ingestion api key (key_id={result.get('key_id')}, "
        f"audience={result.get('audience')}, name={result.get('name')})",
        file=sys.stderr,
    )
    # The server now claims a brand-new audience for the caller (admin or non-admin
    # alike) as part of the mint request itself (#1510, §4) -- `claimed` says so, rather
    # than this script inferring it or writing the grant rows itself.
    if result.get("claimed"):
        print(f"claimed audience {result.get('audience')}", file=sys.stderr)

    content = format_env_exports(result["key"], otlp_endpoint)
    if args.env_file:
        try:
            write_env_file(args.env_file, content)
        except OSError as e:
            # The key was already minted above and is never retrievable again -- a
            # write failure here (permission denied, read-only/full filesystem, bad
            # path) must never discard it. Fall back to emitting it on stdout, with a
            # clear warning on stderr, then re-raise.
            print(
                f"warning: failed to write --env-file {args.env_file!r} ({e}); "
                "printing the exports below instead so the key is not lost",
                file=sys.stderr,
            )
            sys.stdout.write(content)
            raise
        else:
            print(args.env_file)
    else:
        sys.stdout.write(content)


def main():
    parser = build_parser()
    args = parser.parse_args()
    try:
        run(args, parser)
    except (
        RuntimeError,
        requests.exceptions.RequestException,
        config.ProfileError,
        OSError,
    ) as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
