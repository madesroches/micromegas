"""CLI tool for managing micromegas screens as code.

Provides Terraform-inspired workflow: init, import, pull, plan, apply, list.
"""

import argparse
import difflib
import json
import os
import subprocess
import sys
from pathlib import Path

from micromegas.web_client import WebClient

CONFIG_FILE = "micromegas-screens.json"


# ---------------------------------------------------------------------------
# File I/O helpers
# ---------------------------------------------------------------------------


def read_config():
    """Read and validate micromegas-screens.json from the current directory."""
    path = Path(CONFIG_FILE)
    if not path.exists():
        print(
            f"Error: {CONFIG_FILE} not found in current directory.\n"
            "Run 'micromegas-screens init <server_url>' first.",
            file=sys.stderr,
        )
        sys.exit(1)
    with open(path, "r", encoding="utf-8-sig") as f:
        data = json.load(f)
    for field in ("managed_by", "server"):
        if field not in data:
            print(
                f"Error: {CONFIG_FILE} missing required field '{field}'",
                file=sys.stderr,
            )
            sys.exit(1)
    return data


def _load_screen_json(path):
    """Parse a screen JSON file into a raw dict, without schema validation.

    Raises UnicodeDecodeError or json.JSONDecodeError if the file can't even
    be decoded/parsed -- in which case its contents (including any `name`
    field) are fundamentally unknowable.
    """
    with open(path, "r", encoding="utf-8-sig") as f:
        return json.load(f)


def _validate_screen_dict(data, path):
    """Raise ValueError if a parsed dict is missing a required screen field.

    Unlike a decode/parse failure, `data` is a real dict here -- its `name`
    field (if present) is known even though the screen itself is invalid.
    """
    if not isinstance(data, dict):
        raise ValueError(f"{path}: top-level JSON must be an object")
    for field in ("name", "screen_type", "config"):
        if field not in data:
            raise ValueError(f"{path}: missing required field '{field}'")
    if not isinstance(data["name"], str):
        raise ValueError(f"{path}: field 'name' must be a string")


def read_screen_file(path):
    """Read and validate a screen JSON file."""
    data = _load_screen_json(path)
    _validate_screen_dict(data, path)
    return data


def write_screen_file(path, screen_dict):
    """Write pretty-printed JSON with stable key order."""
    ordered = {}
    for key in ("name", "screen_type", "config", "folder_path", "managed_by"):
        if key in screen_dict:
            ordered[key] = screen_dict[key]
    with open(path, "w", encoding="utf-8") as f:
        json.dump(ordered, f, indent=2, ensure_ascii=False)
        f.write("\n")


def list_local_screens():
    """Scan current directory for screen JSON files (excluding config file).

    Returns (screens, unreadable, invalid_names), distinguishing two tiers of
    "can't use this file" for callers doing delete protection:

    - `screens` maps name -> data for files that parsed and validated
      successfully.
    - `unreadable` is the set of file stems whose identity is genuinely
      undeterminable: the file couldn't even be decoded/parsed as JSON
      (UnicodeDecodeError, json.JSONDecodeError), so we don't have a dict to
      look at, let alone a `name`. Callers should treat a non-empty
      `unreadable` as a reason to fall back to a conservative, repo-wide
      delete suppression, since we can't rule out that one of these files
      would otherwise account for a server-tracked screen.
    - `invalid_names` is the set of `name` values found in files that parsed
      as JSON just fine but failed schema validation (missing `screen_type`/
      `config`/etc -- a `ValueError` from `read_screen_file`). Unlike
      `unreadable`, we *do* have the parsed dict here, so if it has a `name`
      field, that specific name is known-but-locally-invalid: callers can
      protect just that one name from being treated as deleted, without
      suppressing deletes for unrelated, cleanly-removed screens. A
      parsed-but-invalid file with no `name` field at all contributes
      nothing here; its identity is unknown too, but since it's an
      unrelated local-authoring mistake, it doesn't warrant blocking deletes
      for the rest of the repo.
    """
    screens = {}
    unreadable = set()
    invalid_names = set()
    for p in sorted(Path(".").glob("*.json")):
        if p.name == CONFIG_FILE:
            continue
        try:
            data = _load_screen_json(p)
        except UnicodeDecodeError as e:
            print(
                f"Warning: skipping {p}: encoding error ({e}); "
                "not treating as absent",
                file=sys.stderr,
            )
            unreadable.add(p.stem)
            continue
        except json.JSONDecodeError as e:
            print(f"Warning: skipping {p}: {e}", file=sys.stderr)
            unreadable.add(p.stem)
            continue

        try:
            _validate_screen_dict(data, p)
        except ValueError as e:
            print(f"Warning: skipping {p}: {e}", file=sys.stderr)
            name = data.get("name") if isinstance(data, dict) else None
            if isinstance(name, str) and name:
                invalid_names.add(name)
            continue

        screens[data["name"]] = data
    return screens, unreadable, invalid_names


VOLATILE_KEYS = {"created_by", "updated_by", "created_at", "updated_at"}


def strip_volatile_keys(screen_dict):
    """Remove server-managed volatile keys that change on every save."""
    return {k: v for k, v in screen_dict.items() if k not in VOLATILE_KEYS}


def screens_equal(a, b):
    """Compare two screen dicts ignoring volatile server metadata."""
    return strip_volatile_keys(a) == strip_volatile_keys(b)


def server_screen_to_file(server_screen):
    """Convert a server screen response to file format (strip metadata)."""
    result = {
        "name": server_screen["name"],
        "screen_type": server_screen["screen_type"],
        "config": server_screen["config"],
    }
    if server_screen.get("folder_path"):
        result["folder_path"] = server_screen["folder_path"]
    if server_screen.get("managed_by"):
        result["managed_by"] = server_screen["managed_by"]
    return result


# ---------------------------------------------------------------------------
# Client factory
# ---------------------------------------------------------------------------


def make_client(config):
    """Create a WebClient from config, with optional OIDC auth."""
    auth_provider = None
    issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
    if issuer and client_id:
        client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
        if client_secret:
            from micromegas.auth.oidc import OidcClientCredentialsProvider

            auth_provider = OidcClientCredentialsProvider.from_env()
        else:
            from micromegas.oidc_connection import load_or_login

            auth_provider = load_or_login(
                issuer=issuer,
                client_id=client_id,
            )
    return WebClient(config["server"], auth_provider=auth_provider)


# ---------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------


def cmd_init(args):
    """Initialize the screens directory and config file."""
    if Path(CONFIG_FILE).exists():
        print(f"Error: {CONFIG_FILE} already exists.", file=sys.stderr)
        sys.exit(1)

    # Must be inside a git repo
    try:
        subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], stderr=subprocess.DEVNULL
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        print("Error: not inside a git repository.", file=sys.stderr)
        sys.exit(1)

    # Get remote URL
    remote = args.remote or "origin"
    try:
        remote_url = (
            subprocess.check_output(
                ["git", "remote", "get-url", remote], stderr=subprocess.DEVNULL
            )
            .decode()
            .strip()
        )
    except subprocess.CalledProcessError:
        print(f"Error: git remote '{remote}' not found.", file=sys.stderr)
        sys.exit(1)

    # Parse remote URL to browsable HTTPS URL
    import giturlparse

    parsed = giturlparse.parse(remote_url)
    # Use pathname for the full path (owner/repo may drop nested groups)
    repo_path = parsed.pathname.strip("/").removesuffix(".git")
    base_url = f"https://{parsed.resource}/{repo_path}"
    managed_by = base_url

    config_data = {
        "managed_by": managed_by,
        "server": args.server_url,
    }

    with open(CONFIG_FILE, "w", encoding="utf-8") as f:
        json.dump(config_data, f, indent=2, ensure_ascii=False)
        f.write("\n")

    print(f"Created {CONFIG_FILE}:")
    print(json.dumps(config_data, indent=2, ensure_ascii=False))


def cmd_import(args):
    """Import existing server screens into the local directory."""
    config = read_config()
    client = make_client(config)
    managed_by = config["managed_by"]

    for name in args.names:
        local_path = Path(f"{name}.json")
        if local_path.exists():
            print(
                f"Error: {local_path} already exists (already imported).",
                file=sys.stderr,
            )
            continue

        try:
            screen = client.get_screen(name)

            # Check if managed by another repo
            existing_owner = screen.get("managed_by")
            if existing_owner and existing_owner != managed_by:
                print(f'Warning: "{name}" is currently managed by:')
                print(f"  {existing_owner}")
                answer = (
                    input("Transfer ownership to this repo? [y/N]: ").strip().lower()
                )
                if answer != "y":
                    print(f"Skipped '{name}'.")
                    continue

            # Set managed_by on server, then download the screen
            client.update_screen(name, screen["config"], managed_by=managed_by)
            screen = client.get_screen(name)
            write_screen_file(local_path, server_screen_to_file(screen))
            print(f"Imported: {name}")
        except RuntimeError as e:
            print(f"Error importing '{name}': {e}", file=sys.stderr)


def cmd_pull(args):
    """Refresh tracked screens from server to disk."""
    config = read_config()
    client = make_client(config)

    if args.names:
        names = args.names
        # Verify they exist locally
        for name in names:
            if not Path(f"{name}.json").exists():
                print(
                    f"Error: {name}.json not found locally. Use 'import' to adopt new screens.",
                    file=sys.stderr,
                )
                sys.exit(1)
    else:
        local, _unreadable, _invalid_names = list_local_screens()
        names = list(local.keys())

    if not names:
        print("No screens to pull.")
        return

    updated = 0
    unchanged = 0
    for name in names:
        try:
            screen = client.get_screen(name)
        except RuntimeError as e:
            print(f"Error fetching '{name}': {e}", file=sys.stderr)
            continue

        local_path = Path(f"{name}.json")
        new_content = server_screen_to_file(screen)

        if local_path.exists():
            try:
                existing = read_screen_file(local_path)
                if server_screen_to_file(existing) == new_content:
                    unchanged += 1
                    continue
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                print(
                    f"Warning: skipping '{name}': {local_path} could not be "
                    f"read ({e}); not overwriting.",
                    file=sys.stderr,
                )
                continue
            except ValueError:
                pass

        write_screen_file(local_path, new_content)
        updated += 1

    print(f"Pull complete: {updated} updated, {unchanged} unchanged.")


def compute_plan(config, client, names=None, local_scan=None):
    """Compute an execution plan. Returns (creates, updates, deletes, unchanged, untracked).

    `local_scan`, if given, is a previously-computed `list_local_screens()`
    result (screens, unreadable, invalid_names). Callers that already need
    to scan the local directory themselves (e.g. `cmd_apply`, which also
    reuses `local` after computing the plan) should pass their scan in here
    instead of letting this function scan again, so unreadable-file warnings
    aren't printed twice for a single command invocation.
    """
    managed_by = config["managed_by"]
    local, unreadable, invalid_names = local_scan or list_local_screens()

    if names:
        local = {k: v for k, v in local.items() if k in names}

    # Fetch all server screens
    server_screens = client.list_screens()
    server_by_name = {s["name"]: s for s in server_screens}

    creates = []
    updates = []
    deletes = []
    unchanged = []
    untracked = []

    # Check local screens against server
    for name, local_data in sorted(local.items()):
        if name not in server_by_name:
            creates.append(name)
        else:
            server = server_by_name[name]
            normalized_local = server_screen_to_file(local_data)
            normalized_server = server_screen_to_file(server)
            if screens_equal(normalized_local, normalized_server):
                unchanged.append(name)
            else:
                updates.append((name, normalized_local, normalized_server))

    # Check for deletions: server screens tracked by this repo but missing
    # locally. Deletes are only ever considered in whole-repo mode (`not
    # names`); in named-subset mode this loop never runs, so there's nothing
    # to warn about and no reason to consult `unreadable`/`invalid_names`.
    if not names:
        if unreadable:
            # These files couldn't even be decoded/parsed, so their `name`
            # is fundamentally unknowable -- we can't rule out that one of
            # them corresponds to a server-tracked screen being reported as
            # "missing locally". Skip delete computation entirely rather
            # than risk a silent delete.
            print(
                f"Warning: {len(unreadable)} local file(s) could not be read "
                f"({', '.join(sorted(unreadable))}); skipping delete computation "
                "since deletes cannot be safely determined.",
                file=sys.stderr,
            )
        else:
            # Files that parsed but failed schema validation have a known
            # `name` (when present) -- protect exactly that name instead of
            # suppressing deletes for the whole repo.
            for name, server in server_by_name.items():
                if (
                    server.get("managed_by") == managed_by
                    and name not in local
                    and name not in invalid_names
                ):
                    deletes.append(name)

    # List untracked server screens
    for name, server in sorted(server_by_name.items()):
        if name not in local:
            srv_managed = server.get("managed_by")
            if srv_managed != managed_by:
                untracked.append(name)

    return creates, updates, deletes, unchanged, untracked


def format_screen_diff(local_dict, server_dict, use_color):
    """Produce a unified diff between server and local screen JSON."""
    server_json = json.dumps(
        server_dict, indent=2, sort_keys=True, ensure_ascii=False
    ).splitlines()
    local_json = json.dumps(
        local_dict, indent=2, sort_keys=True, ensure_ascii=False
    ).splitlines()
    diff_lines = list(
        difflib.unified_diff(server_json, local_json, fromfile="server", tofile="local")
    )
    if not diff_lines:
        return ""
    result = []
    for line in diff_lines:
        if use_color:
            if line.startswith("---") or line.startswith("+++"):
                line = f"\033[1m{line}\033[0m"
            elif line.startswith("@@"):
                line = f"\033[36m{line}\033[0m"
            elif line.startswith("-"):
                line = f"\033[31m{line}\033[0m"
            elif line.startswith("+"):
                line = f"\033[32m{line}\033[0m"
        result.append(f"    {line}")
    return "\n".join(result)


def format_plan(creates, updates, deletes, unchanged, untracked, use_color=False):
    """Format an execution plan for display."""
    lines = []
    if creates or updates or deletes:
        lines.append("micromegas-screens will perform the following actions:\n")
        for name in creates:
            lines.append(f"  + create: {name}")
        for name, local_dict, server_dict in updates:
            lines.append(f"  ~ update: {name}")
            diff = format_screen_diff(local_dict, server_dict, use_color)
            if diff:
                lines.append(diff)
        for name in deletes:
            lines.append(f"  - delete: {name} (tracked, removed from local)")
        lines.append(
            f"\nPlan: {len(creates)} to create, {len(updates)} to update, "
            f"{len(deletes)} to delete, {len(unchanged)} unchanged."
        )
    else:
        lines.append(f"No changes. {len(unchanged)} screens unchanged.")

    if untracked:
        lines.append("\nUntracked screens on server (use 'import' to start tracking):")
        for name in untracked:
            lines.append(f"  ? {name}")

    return "\n".join(lines)


def cmd_plan(args):
    """Preview what apply would change."""
    config = read_config()
    client = make_client(config)
    names = args.names if args.names else None
    use_color = sys.stdout.isatty() and args.color

    creates, updates, deletes, unchanged, untracked = compute_plan(
        config, client, names
    )
    print(format_plan(creates, updates, deletes, unchanged, untracked, use_color))


def cmd_apply(args):
    """Apply local screen state to server."""
    config = read_config()
    client = make_client(config)
    managed_by = config["managed_by"]
    names = args.names if args.names else None

    # Scan the directory once for this whole command invocation; compute_plan
    # reuses this scan instead of scanning again, so unreadable-file warnings
    # aren't printed twice.
    local_scan = list_local_screens()
    creates, updates, deletes, unchanged, untracked = compute_plan(
        config, client, names, local_scan=local_scan
    )

    if not creates and not updates and not deletes:
        print(f"No changes. {len(unchanged)} screens unchanged.")
        return

    use_color = sys.stdout.isatty() and args.color
    print(format_plan(creates, updates, deletes, unchanged, untracked, use_color))
    print()

    if not args.auto_approve:
        answer = input("Do you want to apply these changes? [y/N]: ").strip().lower()
        if answer != "y":
            print("Apply cancelled.")
            sys.exit(1)

    print("Applying...\n")

    local, _unreadable, _invalid_names = local_scan
    created = 0
    updated_count = 0
    deleted = 0
    errors = 0

    def ensure_local_managed_by(name, screen):
        if not screen.get("managed_by"):
            screen["managed_by"] = managed_by
            write_screen_file(Path(f"{name}.json"), screen)

    for name in creates:
        screen = local[name]
        try:
            client.create_screen(
                name=screen["name"],
                screen_type=screen["screen_type"],
                config=screen["config"],
                managed_by=managed_by,
                folder_path=screen.get("folder_path"),
            )
            ensure_local_managed_by(name, screen)
            created += 1
        except RuntimeError as e:
            print(f"Error creating '{name}': {e}", file=sys.stderr)
            errors += 1

    for name, _, _ in updates:
        screen = local[name]
        try:
            client.update_screen(
                name=screen["name"],
                config=screen["config"],
                managed_by=managed_by,
                folder_path=screen.get("folder_path"),
            )
            ensure_local_managed_by(name, screen)
            updated_count += 1
        except RuntimeError as e:
            print(f"Error updating '{name}': {e}", file=sys.stderr)
            errors += 1

    for name in deletes:
        try:
            client.delete_screen(name)
            deleted += 1
        except RuntimeError as e:
            print(f"Error deleting '{name}': {e}", file=sys.stderr)
            errors += 1

    print(
        f"Apply complete! {created} created, {updated_count} updated, {deleted} deleted."
    )
    if errors:
        print(f"{errors} error(s) occurred.", file=sys.stderr)
        sys.exit(1)


def cmd_list(args):
    """Show screen inventory."""
    config = read_config()
    client = make_client(config)
    managed_by = config["managed_by"]

    local, _unreadable, _invalid_names = list_local_screens()
    server_screens = client.list_screens()
    server_by_name = {s["name"]: s for s in server_screens}

    all_names = sorted(set(local.keys()) | set(server_by_name.keys()))

    def screen_status(name):
        in_local = name in local
        in_server = name in server_by_name
        if in_local and in_server:
            return (
                "synced"
                if screens_equal(
                    server_screen_to_file(local[name]),
                    server_screen_to_file(server_by_name[name]),
                )
                else "modified"
            )
        if in_local:
            return "local-only"
        return "server-only"

    if args.format == "json":
        result = [{"name": name, "status": screen_status(name)} for name in all_names]
        print(json.dumps(result, indent=2, ensure_ascii=False))
        return

    # Table format
    print(f"{'Name':<40} {'Status':<15} {'Managed By'}")
    print("-" * 80)
    for name in all_names:
        srv_managed = server_by_name.get(name, {}).get("managed_by", "")
        owner = ""
        if srv_managed == managed_by:
            owner = "this repo"
        elif srv_managed:
            owner = srv_managed
        print(f"{name:<40} {screen_status(name):<15} {owner}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main():
    # Ensure stdout can always print the non-ASCII diff output produced by
    # format_screen_diff(), regardless of the platform's default encoding.
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")

    parser = argparse.ArgumentParser(
        prog="micromegas-screens",
        description="Manage micromegas screens as code",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # init
    p_init = subparsers.add_parser("init", help="Initialize screens directory")
    p_init.add_argument("server_url", help="analytics-web-srv URL")
    p_init.add_argument(
        "--remote", default=None, help="Git remote name (default: origin)"
    )
    p_init.set_defaults(func=cmd_init)

    # import
    p_import = subparsers.add_parser("import", help="Import screens from server")
    p_import.add_argument("names", nargs="+", help="Screen names to import")
    p_import.set_defaults(func=cmd_import)

    # pull
    p_pull = subparsers.add_parser("pull", help="Pull screens from server")
    p_pull.add_argument("names", nargs="*", help="Screen names (default: all local)")
    p_pull.set_defaults(func=cmd_pull)

    # plan
    p_plan = subparsers.add_parser("plan", help="Preview changes")
    p_plan.add_argument("names", nargs="*", help="Screen names (default: all)")
    p_plan.add_argument(
        "--color",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Colored diff output",
    )
    p_plan.set_defaults(func=cmd_plan)

    # apply
    p_apply = subparsers.add_parser("apply", help="Apply changes to server")
    p_apply.add_argument("names", nargs="*", help="Screen names (default: all)")
    p_apply.add_argument(
        "--auto-approve", action="store_true", help="Skip confirmation prompt"
    )
    p_apply.add_argument(
        "--color",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Colored diff output",
    )
    p_apply.set_defaults(func=cmd_apply)

    # list
    p_list = subparsers.add_parser("list", help="List screen inventory")
    p_list.add_argument(
        "--format", choices=["table", "json"], default="table", help="Output format"
    )
    p_list.set_defaults(func=cmd_list)

    args = parser.parse_args()
    try:
        args.func(args)
    except RuntimeError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
