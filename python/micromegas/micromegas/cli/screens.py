"""CLI tool for managing micromegas screens as code.

Provides Terraform-inspired workflow: init, import, pull, plan, apply, list.
"""

import argparse
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
    with open(path, "r") as f:
        data = json.load(f)
    for field in ("managed_by", "server"):
        if field not in data:
            print(
                f"Error: {CONFIG_FILE} missing required field '{field}'",
                file=sys.stderr,
            )
            sys.exit(1)
    return data


def read_screen_file(path):
    """Read and validate a screen JSON file."""
    with open(path, "r") as f:
        data = json.load(f)
    for field in ("name", "screen_type", "config"):
        if field not in data:
            raise ValueError(f"{path}: missing required field '{field}'")
    return data


def write_screen_file(path, screen_dict):
    """Write pretty-printed JSON with stable key order."""
    ordered = {}
    for key in ("name", "screen_type", "config", "managed_by"):
        if key in screen_dict:
            ordered[key] = screen_dict[key]
    with open(path, "w") as f:
        json.dump(ordered, f, indent=2)
        f.write("\n")


def screen_name_from_path(path):
    """Extract screen name from filename."""
    return Path(path).stem


def list_local_screens():
    """Scan current directory for screen JSON files (excluding config file)."""
    screens = {}
    for p in sorted(Path(".").glob("*.json")):
        if p.name == CONFIG_FILE:
            continue
        try:
            data = read_screen_file(p)
            screens[data["name"]] = data
        except (json.JSONDecodeError, ValueError) as e:
            print(f"Warning: skipping {p}: {e}", file=sys.stderr)
    return screens


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
            from micromegas.auth.oidc import OidcAuthProvider

            auth_provider = OidcAuthProvider.from_file(
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
        repo_root = (
            subprocess.check_output(
                ["git", "rev-parse", "--show-toplevel"], stderr=subprocess.DEVNULL
            )
            .decode()
            .strip()
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        print("Error: not inside a git repository.", file=sys.stderr)
        sys.exit(1)

    # Get current branch
    branch = args.branch
    if not branch:
        try:
            branch = (
                subprocess.check_output(
                    ["git", "branch", "--show-current"], stderr=subprocess.DEVNULL
                )
                .decode()
                .strip()
            )
        except subprocess.CalledProcessError:
            branch = "main"
    if not branch:
        branch = "main"

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
    try:
        import giturlparse

        parsed = giturlparse.parse(remote_url)
        base_url = f"https://{parsed.resource}/{parsed.owner}/{parsed.repo}"
    except ImportError:
        # Fallback: basic SSH to HTTPS conversion
        if remote_url.startswith("git@"):
            # git@github.com:org/repo.git -> https://github.com/org/repo
            base_url = remote_url.replace(":", "/").replace("git@", "https://")
        else:
            base_url = remote_url
        if base_url.endswith(".git"):
            base_url = base_url[:-4]

    # Compute relative path from repo root to current directory
    cwd = os.path.abspath(".")
    rel_path = os.path.relpath(cwd, repo_root)
    if rel_path == ".":
        managed_by = f"{base_url}/tree/{branch}"
    else:
        managed_by = f"{base_url}/tree/{branch}/{rel_path}"

    config_data = {
        "managed_by": managed_by,
        "server": args.server_url,
    }

    with open(CONFIG_FILE, "w") as f:
        json.dump(config_data, f, indent=2)
        f.write("\n")

    print(f"Created {CONFIG_FILE}:")
    print(json.dumps(config_data, indent=2))


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
        except RuntimeError as e:
            print(f"Error fetching '{name}': {e}", file=sys.stderr)
            continue

        # Check if managed by another repo
        existing_owner = screen.get("managed_by")
        if existing_owner and existing_owner != managed_by:
            print(f'Warning: "{name}" is currently managed by:')
            print(f"  {existing_owner}")
            answer = input("Transfer ownership to this repo? [y/N]: ").strip().lower()
            if answer != "y":
                print(f"Skipped '{name}'.")
                continue

        # Write local file
        write_screen_file(local_path, server_screen_to_file(screen))

        # Set managed_by on server
        try:
            client.update_screen(name, screen["config"], managed_by=managed_by)
        except RuntimeError as e:
            print(
                f"Warning: imported '{name}' locally but failed to set managed_by on server: {e}",
                file=sys.stderr,
            )

        print(f"Imported: {name}")


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
        local = list_local_screens()
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
                if existing == new_content:
                    unchanged += 1
                    continue
            except (json.JSONDecodeError, ValueError):
                pass

        write_screen_file(local_path, new_content)
        updated += 1

    print(f"Pull complete: {updated} updated, {unchanged} unchanged.")


def compute_plan(config, client, names=None):
    """Compute an execution plan. Returns (creates, updates, deletes, unchanged, untracked)."""
    managed_by = config["managed_by"]
    local = list_local_screens()

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
            if screens_equal(local_data, server):
                unchanged.append(name)
            else:
                updates.append(name)

    # Check for deletions: server screens tracked by this repo but missing locally
    if not names:
        for name, server in server_by_name.items():
            if server.get("managed_by") == managed_by and name not in local:
                deletes.append(name)

    # List untracked server screens
    for name, server in sorted(server_by_name.items()):
        if name not in local:
            srv_managed = server.get("managed_by")
            if srv_managed != managed_by:
                untracked.append(name)

    return creates, updates, deletes, unchanged, untracked


def format_plan(creates, updates, deletes, unchanged, untracked):
    """Format an execution plan for display."""
    lines = []
    if creates or updates or deletes:
        lines.append("micromegas-screens will perform the following actions:\n")
        for name in creates:
            lines.append(f"  + create: {name}")
        for name in updates:
            lines.append(f"  ~ update: {name}")
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

    creates, updates, deletes, unchanged, untracked = compute_plan(
        config, client, names
    )
    print(format_plan(creates, updates, deletes, unchanged, untracked))


def cmd_apply(args):
    """Apply local screen state to server."""
    config = read_config()
    client = make_client(config)
    managed_by = config["managed_by"]
    names = args.names if args.names else None

    creates, updates, deletes, unchanged, untracked = compute_plan(
        config, client, names
    )

    if not creates and not updates and not deletes:
        print(f"No changes. {len(unchanged)} screens unchanged.")
        return

    print(format_plan(creates, updates, deletes, unchanged, untracked))
    print()

    if not args.auto_approve:
        answer = input("Do you want to apply these changes? [y/N]: ").strip().lower()
        if answer != "y":
            print("Apply cancelled.")
            sys.exit(1)

    print("Applying...\n")

    local = list_local_screens()
    created = 0
    updated_count = 0
    deleted = 0
    errors = 0

    for name in creates:
        screen = local[name]
        try:
            client.create_screen(
                name=screen["name"],
                screen_type=screen["screen_type"],
                config=screen["config"],
                managed_by=managed_by,
            )
            created += 1
        except RuntimeError as e:
            print(f"Error creating '{name}': {e}", file=sys.stderr)
            errors += 1

    for name in updates:
        screen = local[name]
        try:
            client.update_screen(
                name=screen["name"],
                config=screen["config"],
                managed_by=managed_by,
            )
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

    local = list_local_screens()
    server_screens = client.list_screens()
    server_by_name = {s["name"]: s for s in server_screens}

    all_names = sorted(set(local.keys()) | set(server_by_name.keys()))

    if args.format == "json":
        result = []
        for name in all_names:
            in_local = name in local
            in_server = name in server_by_name
            if in_local and in_server:
                status = "synced" if screens_equal(local[name], server_by_name[name]) else "modified"
            elif in_local:
                status = "local-only"
            else:
                status = "server-only"
            result.append({"name": name, "status": status})
        print(json.dumps(result, indent=2))
        return

    # Table format
    print(f"{'Name':<40} {'Status':<15} {'Managed By'}")
    print("-" * 80)
    for name in all_names:
        in_local = name in local
        in_server = name in server_by_name
        if in_local and in_server:
            status = "synced" if screens_equal(local[name], server_by_name[name]) else "modified"
        elif in_local:
            status = "local-only"
        else:
            status = "server-only"

        srv_managed = server_by_name.get(name, {}).get("managed_by", "")
        owner = ""
        if srv_managed == managed_by:
            owner = "this repo"
        elif srv_managed:
            owner = srv_managed
        print(f"{name:<40} {status:<15} {owner}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main():
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
    p_init.add_argument("--branch", default=None, help="Git branch (default: current)")
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
    p_plan.set_defaults(func=cmd_plan)

    # apply
    p_apply = subparsers.add_parser("apply", help="Apply changes to server")
    p_apply.add_argument("names", nargs="*", help="Screen names (default: all)")
    p_apply.add_argument(
        "--auto-approve", action="store_true", help="Skip confirmation prompt"
    )
    p_apply.set_defaults(func=cmd_apply)

    # list
    p_list = subparsers.add_parser("list", help="List screen inventory")
    p_list.add_argument(
        "--format", choices=["table", "json"], default="table", help="Output format"
    )
    p_list.set_defaults(func=cmd_list)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
