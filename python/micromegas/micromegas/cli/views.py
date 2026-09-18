"""CLI tool for managing DDL-defined materialized view sets as code.

Terraform-shaped workflow (`plan`/`apply`/`pull`/`list`/`show`) over a directory of
`<view_set_name>.sql` files, each holding exactly one `CREATE [OR REPLACE] MATERIALIZED
VIEW` statement. The server is the current state (`list_view_set_definitions()`); the files
are the desired state. Nothing new is stored -- the write path is the existing
`CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` DDL over FlightSQL.
"""

import argparse
import re
import sys
from collections import namedtuple
from pathlib import Path

import pandas
import pyarrow
from pyarrow import flight
from tabulate import tabulate

from micromegas.cli.config import ProfileError
from micromegas.cli.state_sync import (
    add_color_arg,
    confirm_apply,
    unified_diff,
    use_color,
)
from micromegas.cli.version import add_version_argument
from micromegas.connection import connect_with_profile

# ---------------------------------------------------------------------------
# Local file model and parsing
# ---------------------------------------------------------------------------

LocalDefinition = namedtuple("LocalDefinition", "name text path")


class HeaderParseError(ValueError):
    """Raised when a `.sql` file's head does not match `CREATE [OR REPLACE] MATERIALIZED
    VIEW <name>`, after stripping any stacked leading `--`/`/* */` comments. A comment
    interleaved *inside* the keyword run itself (`CREATE /* x */ OR REPLACE MATERIALIZED
    VIEW`) also lands here, even though the server's `sqlparser` accepts it -- an accepted
    limitation in exchange for needing no SQL lexer.
    """


_HEADER_KEYWORDS_RE = re.compile(
    r"CREATE\s+(?:OR\s+REPLACE\s+)?MATERIALIZED\s+VIEW", re.IGNORECASE
)
_HEADER_RE = re.compile(
    r"(?:CREATE\s+(?:OR\s+REPLACE\s+)?MATERIALIZED\s+VIEW)\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
    re.IGNORECASE,
)


def _skip_leading_comments(text):
    """Return the index in `text` where a stacked run of leading `--`/`/* */` comments (any
    mix, in any order) ends, skipping whitespace around and between them too. Nothing but
    this head-anchored region is ever read out of a `.sql` file's text -- the three queries
    and the options are never parsed.
    """
    i = 0
    n = len(text)
    while True:
        while i < n and text[i].isspace():
            i += 1
        if text.startswith("--", i):
            newline = text.find("\n", i)
            i = n if newline == -1 else newline + 1
            continue
        if text.startswith("/*", i):
            end = text.find("*/", i)
            if end == -1:
                # Unterminated block comment: return as-is so the header match below fails
                # with a clear "header does not parse" outcome rather than looping forever.
                return i
            i = end + 2
            continue
        return i


def _rewrite_header(text, replacement):
    """Replace the leading `CREATE [OR REPLACE] MATERIALIZED VIEW` keyword run (after any
    leading comments) with `replacement`. Shared by `canonical_ddl` (collapses to plain
    `CREATE MATERIALIZED VIEW`) and `with_or_replace` (expands to `CREATE OR REPLACE
    MATERIALIZED VIEW`) -- the same head-anchored region `_skip_leading_comments` reads.
    """
    i = _skip_leading_comments(text)
    prefix, rest = text[:i], text[i:]
    return prefix + _HEADER_KEYWORDS_RE.sub(replacement, rest, count=1)


def canonical_ddl(text):
    """The canonical form both sides of the plan comparison are reduced to: CRLF/CR -> LF,
    strip leading/trailing whitespace, drop a single trailing `;` and re-strip, then
    collapse the leading keyword run to exactly `CREATE MATERIALIZED VIEW`. Interior
    whitespace elsewhere is deliberately left untouched.
    """
    text = text.replace("\r\n", "\n").replace("\r", "\n").strip()
    if text.endswith(";"):
        text = text[:-1].strip()
    return _rewrite_header(text, "CREATE MATERIALIZED VIEW")


def with_or_replace(text):
    """Inject `OR REPLACE` into `text`'s header when absent -- idempotent, and the inverse
    of `canonical_ddl`'s keyword-collapsing step. Used to build the statement `apply` sends
    for an update.
    """
    return _rewrite_header(text, "CREATE OR REPLACE MATERIALIZED VIEW")


def parse_local_definition(path):
    """Read `path` (utf-8-sig) into a `LocalDefinition`.

    Raises `UnicodeDecodeError` if the file can't even be decoded, or `HeaderParseError` if
    its head does not match `CREATE [OR REPLACE] MATERIALIZED VIEW <name>`. Does not check
    the declared name against the filename stem -- callers (`list_local_definitions`) do,
    since that is a different kind of failure (decodes and parses fine, but disagrees with
    the file it's in).
    """
    text = Path(path).read_text(encoding="utf-8-sig")
    match = _HEADER_RE.match(text, _skip_leading_comments(text))
    if not match:
        raise HeaderParseError(
            f"{path}: does not start with CREATE [OR REPLACE] MATERIALIZED VIEW <name>"
        )
    return LocalDefinition(name=match.group("name"), text=text, path=Path(path))


def list_local_definitions(directory):
    """Scan `directory` for `*.sql` files -> `(definitions, protected_names)`.

    `definitions` maps view set name to `LocalDefinition`, for every file that decoded,
    whose header parsed, and whose declared name matched its filename stem. Every other
    file is reported (once) and skipped, contributing its filename stem -- and, when the
    file parsed but disagreed with its own filename, the name it declared too -- to
    `protected_names`, the set `--prune` refuses to drop.
    """
    definitions = {}
    protected_names = set()
    for path in sorted(Path(directory).glob("*.sql")):
        try:
            local_def = parse_local_definition(path)
        except UnicodeDecodeError as e:
            print(f"Warning: skipping {path}: encoding error ({e})", file=sys.stderr)
            protected_names.add(path.stem)
            continue
        except HeaderParseError as e:
            print(f"Warning: skipping {path}: {e}", file=sys.stderr)
            protected_names.add(path.stem)
            continue

        if local_def.name != path.stem:
            print(
                f"Warning: skipping {path}: declared name '{local_def.name}' does not "
                f"match filename stem '{path.stem}'",
                file=sys.stderr,
            )
            protected_names.add(path.stem)
            protected_names.add(local_def.name)
            continue

        definitions[local_def.name] = local_def
    return definitions, protected_names


# ---------------------------------------------------------------------------
# Client factory and current-state read
# ---------------------------------------------------------------------------


def make_client(args):
    """Create a FlightSQL client from `--profile`, resolving auth through the standard
    `connect_with_profile` precedence (static API key, then OIDC, then unauthenticated) --
    unlike `micromegas-screens`, no `--no-auth` flag is needed.
    """
    return connect_with_profile(profile=args.profile, client_entrypoint="cli-views")


def read_server_state(client):
    """-> DataFrame(view_set_name, definition_sql, update_group, updated_at, updated_by)"""
    return client.query(
        "SELECT view_set_name, definition_sql, update_group, updated_at, updated_by "
        "FROM list_view_set_definitions()"
    )


# ---------------------------------------------------------------------------
# The plan model
# ---------------------------------------------------------------------------


def compute_plan(server_state, local_scan, names=None):
    """-> (creates, updates, unchanged, server_only)

    `updates` elements are `(name, local_canonical, server_canonical)` triples. `names`
    narrows only the create/update/unchanged classification (iterating the named subset of
    local definitions); `server_only` is always computed against the full local
    `definitions` map, so a locally-managed view outside the named subset is never
    misreported as server-only.
    """
    definitions, _protected_names = local_scan
    server_names = set(server_state["view_set_name"])
    server_sql_by_name = dict(
        zip(server_state["view_set_name"], server_state["definition_sql"])
    )

    local_names = set(definitions)
    considered = (local_names & set(names)) if names else local_names

    creates = []
    updates = []
    unchanged = []
    for name in sorted(considered):
        local_canonical = canonical_ddl(definitions[name].text)
        if name not in server_names:
            creates.append(name)
        else:
            server_canonical = canonical_ddl(server_sql_by_name[name])
            if local_canonical == server_canonical:
                unchanged.append(name)
            else:
                updates.append((name, local_canonical, server_canonical))

    server_only = sorted(server_names - local_names)
    return creates, updates, unchanged, server_only


def format_plan(creates, updates, unchanged, server_only, drops, use_color=False):
    """-> str. `server_only` and `drops` are computed at the call site, not by
    `compute_plan`: `drops` renders as `- drop:` actions, and `server_only` minus `drops`
    renders under the server-only footer.
    """
    lines = []
    if creates or updates or drops:
        lines.append("micromegas-views will perform the following actions:\n")
        for name in creates:
            lines.append(f"  + create: {name}")
        for name, local_canonical, server_canonical in updates:
            lines.append(f"  ~ update: {name}")
            diff = unified_diff(
                server_canonical.splitlines(),
                local_canonical.splitlines(),
                "server",
                "local",
                use_color,
            )
            if diff:
                lines.append(diff)
        for name in drops:
            lines.append(f"  - drop: {name}")
        lines.append(
            f"\nPlan: {len(creates)} to create, {len(updates)} to update, "
            f"{len(drops)} to drop, {len(unchanged)} unchanged."
        )
    else:
        lines.append(f"No changes. {len(unchanged)} unchanged.")

    footer_names = sorted(set(server_only) - set(drops))
    if footer_names:
        if drops:
            lines.append("\nServer-only view sets (use 'pull' to adopt):")
        else:
            lines.append(
                "\nServer-only view sets (use 'pull' to adopt, '--prune' to drop):"
            )
        for name in footer_names:
            lines.append(f"  ? {name}")

    return "\n".join(lines)


def _check_unknown_names(names, definitions, server_names):
    """Print an error for every name in `names` present on neither side. Returns True if
    any were unknown.
    """
    unknown = sorted(n for n in names if n not in definitions and n not in server_names)
    for name in unknown:
        print(f"Error: '{name}' not found locally or on the server.", file=sys.stderr)
    return bool(unknown)


def _compute_drops(server_only, protected_names, names, prune):
    """`names` and `--prune` compose orthogonally: `names` narrows which view sets are
    considered, `prune` alone gates whether drops happen at all.
    """
    if not prune:
        return []
    return [
        n for n in server_only if n not in protected_names and (not names or n in names)
    ]


def _refuse_prune_on_empty_dir(definitions, directory, prune):
    if prune and not definitions:
        print(
            f"Error: --prune refuses to run against zero readable .sql files in {directory}",
            file=sys.stderr,
        )
        sys.exit(1)


# ---------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------


def _gather(args):
    """Shared `plan`/`apply` preamble: client construction, local scan, the
    empty-directory `--prune` guard, the server-state read, `names` normalization, the
    unknown-name check, `compute_plan`, and `_compute_drops`.
    """
    client = make_client(args)
    local_scan = list_local_definitions(args.dir)
    definitions, protected_names = local_scan
    _refuse_prune_on_empty_dir(definitions, args.dir, args.prune)

    server_state = read_server_state(client)
    server_names = set(server_state["view_set_name"])
    names = args.names or None

    unknown = _check_unknown_names(names, definitions, server_names) if names else False

    creates, updates, unchanged, server_only = compute_plan(
        server_state, local_scan, names
    )
    drops = _compute_drops(server_only, protected_names, names, args.prune)

    return (
        client,
        definitions,
        protected_names,
        creates,
        updates,
        unchanged,
        server_only,
        drops,
        unknown,
    )


def cmd_plan(args):
    """Preview what apply would change."""
    (
        _client,
        _definitions,
        protected_names,
        creates,
        updates,
        unchanged,
        server_only,
        drops,
        unknown,
    ) = _gather(args)

    colorize = use_color(args)
    print(format_plan(creates, updates, unchanged, server_only, drops, colorize))

    if protected_names or unknown:
        sys.exit(1)


def cmd_apply(args):
    """Apply local view set definitions to the server."""
    (
        client,
        definitions,
        protected_names,
        creates,
        updates,
        unchanged,
        server_only,
        drops,
        unknown,
    ) = _gather(args)

    if not creates and not updates and not drops:
        print(f"No changes. {len(unchanged)} unchanged.")
        if protected_names or unknown:
            sys.exit(1)
        return

    colorize = use_color(args)
    print(format_plan(creates, updates, unchanged, server_only, drops, colorize))
    print()

    if not confirm_apply(args.auto_approve):
        sys.exit(1)

    print("Applying...\n")

    created = 0
    updated_count = 0
    dropped = 0
    errors = 0

    def run_statement(name, sql):
        nonlocal errors
        try:
            result = client.query(sql)
            print(f"{name}: {result.iloc[0]['status']}")
            return True
        except (flight.FlightError, pyarrow.lib.ArrowException) as e:
            print(f"Error applying '{name}': {e}", file=sys.stderr)
            errors += 1
            return False

    # All creates and updates run first, then all drops, so an update that removes a
    # dependency on a to-be-pruned view lands before the drop.
    for name in sorted(creates):
        if run_statement(name, canonical_ddl(definitions[name].text)):
            created += 1

    for name, _local_canonical, _server_canonical in sorted(updates):
        if run_statement(name, with_or_replace(definitions[name].text)):
            updated_count += 1

    for name in sorted(drops):
        if run_statement(name, f"DROP MATERIALIZED VIEW {name}"):
            dropped += 1

    print(
        f"\nApply complete! {created} created, {updated_count} updated, {dropped} dropped."
    )
    if errors:
        print(f"{errors} error(s) occurred.", file=sys.stderr)
    if errors or protected_names or unknown:
        sys.exit(1)


def cmd_pull(args):
    """Refresh local `.sql` files from the server."""
    client = make_client(args)
    directory = Path(args.dir)
    definitions, _protected_names = list_local_definitions(directory)
    server_state = read_server_state(client)
    server_sql_by_name = dict(
        zip(server_state["view_set_name"], server_state["definition_sql"])
    )

    if args.names:
        names = list(args.names)
        for name in names:
            if name not in definitions and name not in server_sql_by_name:
                print(
                    f"Error: '{name}' not found locally or on the server.",
                    file=sys.stderr,
                )
                sys.exit(1)
    else:
        # Bare pull refreshes only the parseable names already in the local scan -- a
        # skipped file's own warning was already printed by list_local_definitions above.
        names = sorted(definitions)

    updated = 0
    unchanged = 0
    for name in names:
        if name not in server_sql_by_name:
            # The normal state between authoring a new file and running apply.
            print(
                f"Warning: '{name}' not found on the server; skipping.", file=sys.stderr
            )
            continue

        rendered = canonical_ddl(server_sql_by_name[name]) + "\n"
        target = directory / f"{name}.sql"

        if target.exists():
            if name in definitions:
                if definitions[name].text == rendered:
                    unchanged += 1
                    continue
            else:
                # Exists but failed to decode, its header didn't parse, or its declared
                # name disagreed with the filename -- already warned about above by
                # list_local_definitions. The filename stem fixes this view set's identity
                # regardless of what the broken file's contents say, so there is no
                # unknowable-identity case to protect here, unlike screens.py's no-clobber
                # guard: overwrite it.
                print(
                    f"Warning: overwriting {target}, which could not be read as this view "
                    "set's definition.",
                    file=sys.stderr,
                )

        target.write_text(rendered, encoding="utf-8")
        updated += 1

    print(f"Pull complete: {updated} updated, {unchanged} unchanged.")


def cmd_list(args):
    """Show the view set inventory: local/server diff status plus server metadata."""
    client = make_client(args)
    local_scan = list_local_definitions(args.dir)
    server_state = read_server_state(client)
    creates, updates, unchanged, server_only = compute_plan(server_state, local_scan)

    status_by_name = {}
    for name in creates:
        status_by_name[name] = "create"
    for name, _local_canonical, _server_canonical in updates:
        status_by_name[name] = "update"
    for name in unchanged:
        status_by_name[name] = "unchanged"
    for name in server_only:
        status_by_name[name] = "server-only"

    # dtype=object avoids an empty inventory's "name"/"status" columns defaulting to
    # float64, which fails to merge against server_state's object-typed columns below.
    status_df = pandas.DataFrame(
        {
            "name": pandas.Series(list(status_by_name), dtype=object),
            "status": pandas.Series(list(status_by_name.values()), dtype=object),
        }
    )
    # A create row has no server row yet -- the left merge leaves update_group/updated_at/
    # updated_by null for it, rather than guessing.
    merged = status_df.merge(
        server_state[["view_set_name", "update_group", "updated_at", "updated_by"]],
        how="left",
        left_on="name",
        right_on="view_set_name",
    ).drop(columns="view_set_name")
    # Nullable Int64 (rather than the plain int32 the query returns) keeps a create row's
    # missing update_group an integer <NA> instead of upcasting the whole column to
    # float64, which would render an existing row's update_group as e.g. 4000.0.
    merged["update_group"] = merged["update_group"].astype("Int64")
    merged = merged.sort_values("name").reset_index(drop=True)

    if args.format == "json":
        print(merged.to_json(orient="records", indent=2))
    else:
        # For display only: render missing cells as empty rather than "<NA>"/"NaT"/"nan".
        display = merged.astype(object).where(merged.notna(), "")
        print(tabulate(display, headers="keys", showindex=False, tablefmt="simple"))


def cmd_show(args):
    """Print a stored view set's definition verbatim, exactly as the server holds it."""
    client = make_client(args)
    server_state = read_server_state(client)
    row = server_state[server_state["view_set_name"] == args.name]
    if row.empty:
        print(f"Error: '{args.name}' not found on the server.", file=sys.stderr)
        sys.exit(1)
    print(row.iloc[0]["definition_sql"])


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main():
    # Ensure stdout can always print the non-ASCII diff output this tool can produce,
    # regardless of the platform's default encoding.
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")

    parser = argparse.ArgumentParser(
        prog="micromegas-views",
        description="Manage DDL-defined materialized view sets as code",
    )
    add_version_argument(parser)
    subparsers = parser.add_subparsers(dest="command", required=True)

    # Defined in exactly one place each -- see screens.py's client_args for the
    # shared-Namespace trap this dodges.
    client_args = argparse.ArgumentParser(add_help=False)
    client_args.add_argument(
        "--profile", help="Named connection profile from ~/.micromegas/config.json"
    )

    dir_args = argparse.ArgumentParser(add_help=False)
    dir_args.add_argument(
        "--dir", default=".", help="Directory of <view_set_name>.sql files (default: .)"
    )

    # plan
    p_plan = subparsers.add_parser(
        "plan", parents=[client_args, dir_args], help="Preview changes"
    )
    p_plan.add_argument("names", nargs="*", help="View set names (default: all)")
    p_plan.add_argument(
        "--prune", action="store_true", help="Propose dropping server-only view sets"
    )
    add_color_arg(p_plan)
    p_plan.set_defaults(func=cmd_plan)

    # apply
    p_apply = subparsers.add_parser(
        "apply", parents=[client_args, dir_args], help="Apply changes to the server"
    )
    p_apply.add_argument("names", nargs="*", help="View set names (default: all)")
    p_apply.add_argument(
        "--prune", action="store_true", help="Drop server-only view sets"
    )
    p_apply.add_argument(
        "--auto-approve", action="store_true", help="Skip confirmation prompt"
    )
    add_color_arg(p_apply)
    p_apply.set_defaults(func=cmd_apply)

    # pull
    p_pull = subparsers.add_parser(
        "pull", parents=[client_args, dir_args], help="Pull definitions from the server"
    )
    p_pull.add_argument("names", nargs="*", help="View set names (default: all local)")
    p_pull.set_defaults(func=cmd_pull)

    # list
    p_list = subparsers.add_parser(
        "list", parents=[client_args, dir_args], help="List view set inventory"
    )
    p_list.add_argument(
        "--format", choices=["table", "json"], default="table", help="Output format"
    )
    p_list.set_defaults(func=cmd_list)

    # show
    p_show = subparsers.add_parser(
        "show", parents=[client_args], help="Show a stored view set definition"
    )
    p_show.add_argument("name", help="View set name")
    p_show.set_defaults(func=cmd_show)

    args = parser.parse_args()

    if hasattr(args, "dir") and not Path(args.dir).is_dir():
        print(f"Error: --dir '{args.dir}' is not a directory.", file=sys.stderr)
        sys.exit(1)

    try:
        args.func(args)
    except (flight.FlightError, pyarrow.lib.ArrowException) as e:
        print(
            f"Error: {e}\n"
            "Every subcommand needs an admin identity: list_view_set_definitions() and "
            "the view DDL path are both gated to lakehouse_admin.",
            file=sys.stderr,
        )
        sys.exit(1)
    except (ProfileError, OSError) as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
