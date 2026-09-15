#!/usr/bin/env python3
"""CLI tool that mints a personal ingestion API key and prints the OTLP
exporter env vars needed to point a user's own telemetry at a micromegas
deployment.

Named `micromegas-setup-telemetry`, not `micromegas-mint-key` or similar --
from the user's point of view this script sets up telemetry transmission
("send my data"), so the server-side term "ingestion" stays out of the
user-facing name.

`--user-audience SUFFIX` and `--audience NAME` are mutually exclusive and both
lazily create the audience when it doesn't already exist: `--user-audience`
composes `SUFFIX` under a namespace prefix derived server-side from the
caller's own email (identical for an admin and a non-admin caller), while
`--audience` uses the name verbatim, for an org/team/service audience that
isn't namespaced under any one caller. Neither flag ever silently rewrites
the name it is given -- `--user-audience` only ever prepends the caller's own
prefix, nothing more.

Auth reuses `import_keys.py::build_auth_provider`/`make_client` verbatim,
which in turn delegates to `web_auth.resolve_web_auth` -- see that
function's doc comment for the resolution ladder. No new OIDC code here.
"""

import argparse
import os
import shlex
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


def _fresh_audience_suggestion(mint_prefix, email):
    """Renders a concrete suggestion for claiming a fresh audience of the caller's own:
    `--user-audience` when a prefix is available (identical for admin and non-admin), a
    bare `--audience <new-name>` when the caller has an email but no prefix (the local
    part sanitizes to empty), or an ask-an-admin line when the caller has no email at
    all and so cannot claim anything (the two "no prefix" cases differ, see
    `mint_prefix_for`).
    """
    if mint_prefix is not None:
        return f"--user-audience <name> (mints under `{mint_prefix}<name>`)"
    if email is not None:
        return "--audience <new-name>"
    return (
        "this caller has no email, so it cannot claim a fresh audience of its own; "
        "ask an admin for a grant"
    )


# Substrings of the mint route's own 403 messages (see `rust/analytics-web-srv/src/
# ingestion_keys.rs`) that mean the denial is actually about audience grants -- as opposed to
# the per-caller live-key cap, the per-caller claim cap, or the self-service knob being off,
# none of which `_mint_denied_hint`'s remedies (claim a fresh audience, ask an admin for an
# audience grant) actually fix.
_AUDIENCE_DENIAL_MARKERS = (
    "mintable set",
    "cannot be claimed",
    "already exists and the caller has no grant for it",
)


def _is_audience_denial(message):
    return any(marker in message for marker in _AUDIENCE_DENIAL_MARKERS)


def _mint_denied_hint(url, audience, my_audiences):
    """The error text appended after a mint request's `HTTP 403` -- every way forward,
    concretely: the caller's own mintable audiences (if any), a suggestion for claiming
    a fresh audience of their own, and the exact `micromegas-grants` commands an admin
    would run to grant this one. This is the answer to the issue's discoverability
    complaint, rendered where the caller hits the error rather than left for them to
    find in the docs.

    Keyed off the resolved `audience` (not any one flag) and reads `audiences`/
    `mint_prefix`/`email` off `my_audiences`, since the denial can now arrive whichever
    of `--audience`/`--user-audience`/neither produced it.
    """
    audiences = my_audiences["audiences"]
    mint_prefix = my_audiences.get("mint_prefix")
    email = my_audiences.get("email")
    lines = [
        "  mintable audiences: "
        + (", ".join(sorted(audiences)) if audiences else "(none)"),
        f"  to use an audience of your own: {_fresh_audience_suggestion(mint_prefix, email)}",
    ]
    if email is not None:
        # `_fresh_audience_suggestion` already ends in "ask an admin for a grant"
        # when there's no email -- don't repeat that lead-in here.
        lines.append("  otherwise, ask an admin to grant it:")
        lines.append(
            f"      micromegas-grants --url {url} create {audience} "
            f"mint 'user:{email}'"
        )
        lines.append("    or, to open it to every authenticated caller:")
        lines.append(f"      micromegas-grants --url {url} create {audience} mint '*'")
    else:
        # No email to grant a `user:`-scoped command for -- the only remaining
        # remedy is opening the audience to every authenticated caller.
        lines.append(
            "  otherwise, ask an admin to grant it, e.g. to every authenticated caller:"
        )
        lines.append(f"      micromegas-grants --url {url} create {audience} mint '*'")
    return "\n".join(lines)


def resolve_audience(args, parser, my_audiences):
    """Resolves the audience to mint under:

    - `--audience` and `--user-audience` together: an error -- both are spellings
      of the same one flag.
    - `--user-audience SUFFIX`: composed as `f"{mint_prefix}{SUFFIX}"`, where
      `mint_prefix` is derived server-side from the caller's own email and is
      identical for an admin and a non-admin caller. Requires a non-empty `SUFFIX`
      and a caller whose email yields a `mint_prefix`; errors otherwise, with
      distinct messages for "no email at all" vs. "email sanitizes to empty".
    - `--audience NAME`: used verbatim, unconditionally -- no client-side check of
      `my_audiences["audiences"]` any more. A genuinely fresh name is lazily
      claimed server-side by the mint route itself; a name someone else already
      holds is refused there with an ordinary `403`, which `run()` enriches with a
      hint (see `_mint_denied_hint`).
    - Both omitted, non-admin: resolved from the caller's *personally held* mint
      audiences only (`my_audiences["held_pairs"]`), filtering out audiences the
      caller can merely see via a `"*"` grant (e.g. the seeded `public` row) --
      exactly one match is used silently; more than one is an error naming the
      choices; none is an error pointing at the visible-but-unheld audiences (if
      any), claiming a fresh name of the caller's own, or asking an admin.
    - Both omitted, admin: an error asking for one explicitly -- `audiences` is
      not a reliable "nothing mintable yet" signal for an admin, but an admin
      resolves `mint_prefix` the same way anyone else does, so `--user-audience`
      is offered too.

    Returns the resolved audience name. Neither this helper nor the mint route's
    caller decides or reports whether the name is brand-new: the mint route runs
    that ownership check server-side and claims a brand-new audience as part of
    the same request (`MintResponse`'s `claimed` field says so), so this helper
    never needs to page through `list_ingestion_api_keys`/`list_audience_grants`
    to decide it client-side.
    """
    if args.audience is not None and args.user_audience is not None:
        parser.error("--audience and --user-audience are mutually exclusive; pick one")

    mint_prefix = my_audiences.get("mint_prefix")
    email = my_audiences.get("email")

    if args.user_audience is not None:
        if not args.user_audience:
            parser.error("--user-audience requires a non-empty name")
        if mint_prefix is None:
            if email is None:
                parser.error(
                    "--user-audience needs a caller-derived prefix and this caller "
                    "has no email to derive one from; a caller with no email cannot "
                    "claim a fresh audience at all, so ask an admin for a grant "
                    "instead"
                )
            parser.error(
                "--user-audience needs a caller-derived prefix and this caller's "
                "email sanitizes to empty; pass the whole name with --audience "
                "<name> instead"
            )
        return f"{mint_prefix}{args.user_audience}"

    if args.audience is not None:
        if not args.audience:
            parser.error("--audience requires a non-empty name")
        return args.audience

    is_admin = my_audiences["is_admin"]
    audiences = my_audiences["audiences"]

    if is_admin:
        parser.error(
            "--audience or --user-audience is required for an admin caller (pick an "
            "audience name explicitly; an empty mintable-audience list means "
            "nothing for an admin)"
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
    fresh = _fresh_audience_suggestion(mint_prefix, email)
    visible = sorted(a for a in audiences if a not in personal)
    if visible:
        prefix = (
            "no mintable audience held personally by this caller; visible but not "
            "personally held (pass one explicitly with --audience): "
            + ", ".join(visible)
        )
    else:
        prefix = "no mintable audience found for this caller"
    if email is None:
        # `fresh` already reads as a full sentence ending in "ask an admin for a
        # grant" -- appending another "ask an admin" clause would just repeat it.
        parser.error(f"{prefix}; {fresh}")
    parser.error(
        f"{prefix}; or claim a fresh one of your own with {fresh}; or ask an "
        "admin for a personal grant"
    )


def write_env_file(path, content):
    """Writes `content` to `path` with mode `0o600` where the platform enforces
    it (parent directory `0o700` if it doesn't already exist) -- mirroring
    `OidcAuthProvider.save()`'s token-cache permissions. `content` holds a
    standing `Authorization: Bearer` credential, so it must never land at
    the process umask. On Windows, POSIX mode bits aren't enforced (restricting
    access there needs an ACL change this CLI doesn't make), so the file lands
    at its parent directory's inherited ACL instead.
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
    # The binary-mode guarantee (no `\n` -> `\r\n` translation on Windows) actually
    # comes from `os.fdopen(fd, "wb")` below, which forces the fd into binary mode.
    # `O_BINARY` here is belt-and-suspenders for a raw `os.write` on the fd, which
    # this function does not do.
    flags = os.O_CREAT | os.O_WRONLY | os.O_TRUNC | getattr(os, "O_BINARY", 0)
    fd = os.open(str(target), flags, 0o600)
    with os.fdopen(fd, "wb") as f:
        # Written before the mode is re-asserted below: `os.open` already
        # create-and-truncated `target`, so a hardening failure after this point
        # costs nothing, while one before it would leave the file empty and the
        # just-minted, never-retrievable key with nowhere to land. Going through
        # the buffered file object (instead of a raw `os.write`) means a short
        # write raises instead of silently truncating the credential.
        f.write(content.encode("utf-8"))
        f.flush()
        # `os.fchmod` is absent on Windows before Python 3.13; where present
        # there, it only toggles the read-only bit, so it cannot deliver `0o600`.
        # `os.open`'s mode argument is masked by the umask, which can only clear
        # bits, so this only ever restores an owner bit the umask stripped -- it
        # can never widen the file past `0o600`.
        if hasattr(os, "fchmod"):
            os.fchmod(fd, stat.S_IRUSR | stat.S_IWUSR)


def _env_var_pairs(key, otlp_endpoint):
    """The `OTEL_EXPORTER_OTLP_*` variables to export, in order. The protocol
    var is required because micromegas exposes OTLP over HTTP only, so an SDK
    defaulting to gRPC would otherwise fail to reach the endpoint.
    `Authorization=Bearer <key>`, capitalized with `=`, matches the
    already-documented OTLP header format (`mkdocs/docs/otlp/index.md`).
    """
    return (
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
        ("OTEL_EXPORTER_OTLP_ENDPOINT", otlp_endpoint),
        ("OTEL_EXPORTER_OTLP_HEADERS", f"Authorization=Bearer {key}"),
    )


def _render_posix(name, value):
    return f"export {name}={shlex.quote(value)}"


def _render_powershell(name, value):
    # PowerShell's tokenizer ends a `'...'` literal on any of these five
    # code points (`CharTraits.IsSingleQuote`), not just the ASCII `'` that
    # opened it, so a curly quote copy-pasted into a value would otherwise
    # break out of the string. Doubling each one is a valid self-escape.
    single_quotes = "'‘’‚‛"
    for q in single_quotes:
        value = value.replace(q, q * 2)
    return f"$env:{name} = '{value}'"


def _render_cmd(name, value):
    # The quoted-`set` form: it is the only one that keeps the quote
    # characters out of the value (`cmd.exe` has no escape for a literal `"`
    # inside it), and the leading `@` suppresses the command echo both in a
    # batch file and at the interactive prompt.
    return f'@set "{name}={value}"'


def _render_dotenv(name, value):
    return f"{name}={value}"


# Insertion order is the order `--help` lists `--format`'s choices, kept as
# the issue lists them (`posix` first, as the default).
_FORMAT_RENDERERS = {
    "posix": _render_posix,
    "powershell": _render_powershell,
    "cmd": _render_cmd,
    "dotenv": _render_dotenv,
}

# Characters each dialect's quoting rule cannot represent -- see
# `_render_*`'s docstrings/comments for why. Empty for `posix`, whose
# `shlex.quote` represents any value.
_FORMAT_UNSAFE_CHARS = {
    "posix": (),
    "powershell": ("\r", "\n"),
    "cmd": ('"', "%", "\r", "\n"),
    "dotenv": ("#", "$", "\r", "\n"),
}


def _unsafe_chars_in(fmt, value):
    """The characters of `value` that `fmt`'s rendering rule cannot
    represent, in `_FORMAT_UNSAFE_CHARS[fmt]` order."""
    return [c for c in _FORMAT_UNSAFE_CHARS[fmt] if c in value]


def check_format_endpoint(fmt, otlp_endpoint, parser):
    """Errors out -- before a key is minted -- when the resolved OTLP
    endpoint carries a character `fmt` cannot represent. Called right after
    `resolve_otlp_endpoint` and before `client.mint_ingestion_api_key`: a
    minted ingestion API key is never retrievable again, so any check that
    can fail locally must run before the mint, never after.
    """
    unsafe = _unsafe_chars_in(fmt, otlp_endpoint)
    if unsafe:
        parser.error(
            f"--otlp-endpoint contains {unsafe[0]!r}, which --format {fmt} "
            "cannot represent; pass a different --format or --otlp-endpoint"
        )
    if fmt == "dotenv" and otlp_endpoint != otlp_endpoint.strip():
        parser.error(
            "--otlp-endpoint has leading/trailing whitespace, which "
            "--format dotenv cannot represent (a dotenv loader would "
            "silently trim it); pass a different --format or --otlp-endpoint"
        )


def format_env_exports(key, otlp_endpoint, fmt="posix"):
    """Renders the `OTEL_EXPORTER_OTLP_PROTOCOL`/`_ENDPOINT`/`_HEADERS`
    variables in `fmt`'s dialect (one of `_FORMAT_RENDERERS`), one line per
    variable, `"\\n"`-joined with a trailing `"\\n"`. Each dialect quotes a
    value differently: `posix` via `shlex.quote` (bare unless the value needs
    quoting), `powershell` as a single-quoted literal (`'` doubled), `cmd` as
    a quoted `@set "NAME=value"`, and `dotenv` unquoted (`NAME=value`, value
    is everything after the first `=`).
    """
    render = _FORMAT_RENDERERS[fmt]
    lines = (render(name, value) for name, value in _env_var_pairs(key, otlp_endpoint))
    return "\n".join(lines) + "\n"


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
        "--user-audience",
        metavar="SUFFIX",
        help=(
            "Mint under f'{mint_prefix}{SUFFIX}', a name namespaced under this "
            "caller's own email-derived prefix (identical composition for an admin "
            "and a non-admin caller). Lazily claims the audience if it doesn't "
            "already exist. Mutually exclusive with --audience."
        ),
    )
    parser.add_argument(
        "--audience",
        help=(
            "Write audience to mint the key under, verbatim -- for an org/team/service "
            "audience that isn't namespaced under any one caller. Lazily claims the "
            "audience if it doesn't already exist; for a non-admin caller, fails with "
            "a 403 if it exists and this caller holds no grant for it (an admin caller "
            "mints into any existing audience verbatim). Omitted entirely resolves one "
            "via GET .../audience-grants/my-audiences. Mutually exclusive with "
            "--user-audience."
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
    parser.add_argument(
        "--format",
        choices=tuple(_FORMAT_RENDERERS),
        default="posix",
        help="Output syntax for the env vars (default: posix). Applies to both "
        "stdout and --env-file; never inferred from the OS.",
    )
    return parser


def run(args, parser):
    client = make_client(args, parser)

    # Called unconditionally, even when --audience/--user-audience is passed
    # explicitly: resolving either flag needs `mint_prefix`, `email`, and the
    # caller's own `audiences`/`held_pairs` from this one response. A useful side
    # effect: a knob-off caller gets a clear 403 up front, instead of a confusing
    # denial only once the mint itself is attempted.
    my_audiences = client.my_audiences()

    audience = resolve_audience(args, parser, my_audiences)

    # Resolved before the mint so a purely local validation error (e.g. missing
    # --otlp-endpoint/MICROMEGAS_TELEMETRY_URL) can never strand an already-minted,
    # never-retrievable-again key.
    otlp_endpoint = resolve_otlp_endpoint(args, parser)
    check_format_endpoint(args.format, otlp_endpoint, parser)

    # The client-side mintable-set guard is gone, so a denial now only ever
    # surfaces here, as the mint route's own 403 -- enrich it with the same
    # discoverability hint the old pre-flight guard used to render, but only when the
    # 403 is actually about audience grants: mint_key also returns 403 for the
    # per-caller live-key cap, the per-caller claim cap, and the self-service knob
    # being off, none of which the hint's remedies fix.
    try:
        result = client.mint_ingestion_api_key(args.name, audience)
    except RuntimeError as e:
        if str(e).startswith("HTTP 403") and _is_audience_denial(str(e)):
            raise RuntimeError(
                f"{e}\n{_mint_denied_hint(args.url, audience, my_audiences)}"
            ) from e
        raise

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

    # Unreachable today (the minted key's alphabet, `mmk_` + base64url-nopad,
    # is safe in every format -- see the design plan), kept so a future change
    # to the server's key alphabet degrades to a warning instead of silently
    # mangled output. A warning, never an error: the key already exists and
    # must not be discarded over a formatting concern.
    header_name, header_value = _env_var_pairs(result["key"], otlp_endpoint)[2]
    unsafe = _unsafe_chars_in(args.format, header_value)
    if unsafe:
        print(
            f"warning: minted key's {header_name} value contains "
            f"{unsafe[0]!r}, which --format {args.format} cannot represent; "
            "output may be malformed",
            file=sys.stderr,
        )

    content = format_env_exports(result["key"], otlp_endpoint, args.format)
    if args.env_file:
        try:
            write_env_file(args.env_file, content)
        except Exception as e:
            # The key was already minted above and is never retrievable again -- a
            # write failure here (permission denied, read-only/full filesystem, bad
            # path, a platform-missing syscall) must never discard it. Fall back to
            # emitting it on stdout, with a clear warning on stderr, then re-raise.
            print(
                f"warning: could not complete --env-file {args.env_file!r} ({e}); "
                "exports printed below so the key is not lost; if the file exists, "
                "treat it as holding a live credential",
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
