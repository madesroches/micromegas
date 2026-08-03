#!/usr/bin/env python3
import argparse
from pathlib import Path

from micromegas.cli.config import ProfileError, default_token_file


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas_logout",
        description="Clear saved OIDC authentication tokens",
    )
    parser.add_argument(
        "--profile",
        help="Only clear this profile's cached tokens",
    )
    args = parser.parse_args()

    if args.profile is not None:
        try:
            targets = [Path(default_token_file(args.profile))]
        except ProfileError as e:
            parser.error(str(e))
    else:
        token_dir = Path.home() / ".micromegas"
        targets = [token_dir / "tokens.json", *sorted(token_dir.glob("tokens-*.json"))]

    removed = False
    for token_file in targets:
        if token_file.exists():
            token_file.unlink()
            print(f"Tokens cleared from {token_file}")
            removed = True
    if not removed:
        print("No saved tokens found")


if __name__ == "__main__":
    main()
