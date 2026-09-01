#!/usr/bin/env python3
"""CLI tool that mints a personal ingestion API key and prints the OTLP
exporter env vars needed to point a user's own telemetry at a micromegas
deployment.

Named `micromegas-setup-telemetry`, not `micromegas-mint-key` or similar --
from the user's point of view this script sets up telemetry transmission
("send my data"), so the server-side term "ingestion" stays out of the
user-facing name.

`--audience NAME` and `--claim NAME` are mutually exclusive and mean two
different things: `--audience` mints under an audience this caller already
holds a grant for (or, for an admin, any valid name), and errors otherwise;
`--claim` claims a brand-new audience for this caller, verbatim -- neither
flag ever silently rewrites the name it is given.

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


def _claim_suggestion(args, mint_prefix):
    """Renders a concrete `--claim` suggestion: the caller's own namespaced name when a
    prefix is available, or a placeholder when it isn't (no email, or an email whose
    local part sanitizes to empty -- the two differ, see `mint_prefix_for`).
    """
    if mint_prefix is not None:
        return f"--claim {mint_prefix}{args.audience}"
    return "--claim <new-name>"


def _cannot_mint_hint(args, mint_prefix, email, audiences):
    """The error text for `--audience X` where X is outside this non-admin caller's
    mintable set -- every way forward, concretely: the caller's own mintable
    audiences (if any), a `--claim` suggestion for a fresh audience of their own, and
    the exact `micromegas-grants` commands an admin would run to grant this one. This is
    the answer to the issue's discoverability complaint, rendered where the caller hits
    the error rather than left for them to find in the docs.
    """
    lines = [
        f"cannot mint audience {args.audience!r}: it is not in this caller's mintable set",
        "  mintable audiences: "
        + (", ".join(sorted(audiences)) if audiences else "(none)"),
        f"  to claim a fresh audience of your own: {_claim_suggestion(args, mint_prefix)}",
        "  otherwise, ask an admin to grant it:",
    ]
    if email is not None:
        lines.append(
            f"      micromegas-grants --url {args.url} create {args.audience} "
            f"mint 'user:{email}'"
        )
        lines.append("    or, to open it to every authenticated caller:")
    lines.append(
        f"      micromegas-grants --url {args.url} create {args.audience} mint '*'"
    )
    return "\n".join(lines)


def resolve_audience(args, parser, my_audiences):
    """Resolves the audience to mint under:

    - `--audience` and `--claim` together: an error -- each flag means one thing.
    - `--claim NAME`: claims `NAME` verbatim, with no prefix applied -- the name
      passed is the name claimed. Requires a non-admin caller with an email (the
      lazy claim the mint route performs needs an identity to write a
      `user:<email>` grant row under); errors otherwise.
    - `--audience X` already in `my_audiences["audiences"]` (the caller has a real
      grant for it, or is admin): used verbatim.
    - `--audience X` otherwise, non-admin: an error. This name is outside the
      caller's mintable set, so minting it is refused rather than silently
      redirected to a different name the caller didn't ask for -- see
      `_cannot_mint_hint` for what the error suggests instead.
    - `--audience X`, admin caller: used verbatim even when not already in
      `audiences` -- deliberate operational naming; the mint route claims a
      brand-new audience for an admin caller as part of the same request.
    - Both omitted, non-admin: resolved from the caller's *personally held* mint
      audiences only (`my_audiences["held_pairs"]`), filtering out audiences the
      caller can merely see via a `"*"` grant (e.g. the seeded `public` row) --
      exactly one match is used silently; more than one is an error naming the
      choices; none is an error pointing at the visible-but-unheld audiences (if
      any), claiming a fresh name, or asking an admin.
    - Both omitted, admin: an error asking for one explicitly -- `audiences` is
      not a reliable "nothing mintable yet" signal for an admin.

    Returns the resolved audience name. The admin branch does not decide or
    report whether the name is brand-new: the mint route runs that ownership
    check server-side and claims a brand-new audience for an admin caller in
    the same request (`MintResponse`'s `claimed` field says so), so this
    helper never needs to page through
    `list_ingestion_api_keys`/`list_audience_grants` to decide it client-side.
    """
    if args.audience is not None and args.claim is not None:
        parser.error("--audience and --claim are mutually exclusive; pick one")

    is_admin = my_audiences["is_admin"]
    audiences = my_audiences["audiences"]
    mint_prefix = my_audiences.get("mint_prefix")
    email = my_audiences.get("email")

    if args.claim is not None:
        if is_admin:
            parser.error(
                "--claim is for a non-admin's own fresh claim; an admin's "
                "brand-new audience is claimed server-side, use --audience "
                f"{args.claim!r} instead"
            )
        if email is None:
            parser.error(
                f"cannot claim {args.claim!r}: this caller has no email to claim with"
            )
        return args.claim

    if args.audience is not None:
        if args.audience in audiences or is_admin:
            return args.audience
        parser.error(_cannot_mint_hint(args, mint_prefix, email, audiences))

    if is_admin:
        parser.error(
            "--audience is required for an admin caller (pick an audience name "
            "explicitly; an empty mintable-audience list means nothing for an admin)"
        )

    # Both flags omitted: filter to the audiences this caller personally holds a
    # mint grant on, so a seeded wildcard row (e.g. the default `public` mint
    # grant) that puts an audience in every caller's `audiences` list doesn't
    # silently redirect a caller who holds nothing of their own into that shared
    # pool merely because they omitted a flag.
    held = set(my_audiences["held_pairs"])
    personal = [a for a in audiences if f"{a}:mint" in held]
    if len(personal) == 1:
        return personal[0]
    if len(personal) > 1:
        parser.error(
            "multiple mintable audiences found ("
            + ", ".join(sorted(personal))
            + "); pick one with --audience"
        )
    visible = sorted(a for a in audiences if a not in personal)
    if visible:
        parser.error(
            "no mintable audience held personally by this caller; visible but not "
            "personally held (pass one explicitly with --audience): "
            + ", ".join(visible)
            + "; or claim a fresh one of your own with --claim <new-name>; or ask "
            "an admin for a personal grant"
        )
    parser.error(
        "no mintable audience found for this caller; claim a fresh one with "
        "--claim <new-name>, or ask an admin for a grant"
    )


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
            "Write audience to mint the key under: an audience you already have a "
            "grant for (used verbatim; an admin may pass any valid name), or omitted "
            "entirely to resolve one via GET .../audience-grants/my-audiences. To "
            "claim a brand-new audience of your own, use --claim instead. "
            "Mutually exclusive with --claim."
        ),
    )
    parser.add_argument(
        "--claim",
        help=(
            "Claim NAME as a fresh audience, verbatim -- the name passed is the name "
            "claimed, with no prefix applied. Fails if NAME already exists and this "
            "caller holds no grant for it. Mutually exclusive with --audience."
        ),
        metavar="NAME",
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

    # Called unconditionally, even when --audience/--claim is passed explicitly:
    # resolving either flag needs `mint_prefix`, `email`, and the caller's own
    # `audiences`/`held_pairs` from this one response. A useful side effect: a
    # knob-off caller gets a clear 403 up front, instead of a confusing denial only
    # once the mint itself is attempted.
    my_audiences = client.my_audiences()

    audience = resolve_audience(args, parser, my_audiences)

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
    # The server claims a brand-new audience for the caller (admin or non-admin
    # alike) as part of the mint request itself -- `claimed` says so, rather
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
