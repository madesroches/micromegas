#!/usr/bin/env python3
import argparse
import os
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas_logout",
        description="Clear saved OIDC authentication tokens",
    )
    parser.parse_args()

    token_file = os.environ.get(
        "MICROMEGAS_TOKEN_FILE", str(Path.home() / ".micromegas" / "tokens.json")
    )

    if Path(token_file).exists():
        Path(token_file).unlink()
        print(f"Tokens cleared from {token_file}")
    else:
        print("No saved tokens found")


if __name__ == "__main__":
    main()
