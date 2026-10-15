#!/usr/bin/env python3
"""Fails if the `client` feature of the micromegas crate pulls in the service stack."""

import pathlib
import subprocess
import sys

FORBIDDEN_CRATES = (
    "datafusion",
    "foyer",
    "micromegas-analytics",
    "micromegas-auth",
    "micromegas-ingestion",
    "micromegas-object-cache",
    "micromegas-otel-ingestion",
    "micromegas-perfetto",
    "object_store",
    "sqlx",
)


def find_forbidden(tree_output: str) -> list[str]:
    """Returns the sorted forbidden crate names found in `cargo tree --prefix none` output."""
    found = set()
    for line in tree_output.splitlines():
        tokens = line.split()
        if tokens and tokens[0] in FORBIDDEN_CRATES:
            found.add(tokens[0])
    return sorted(found)


def main() -> int:
    result = subprocess.run(
        [
            "cargo",
            "tree",
            "-p",
            "micromegas",
            "--no-default-features",
            "--features",
            "client",
            "-e",
            "normal",
            "--prefix",
            "none",
        ],
        check=True,
        capture_output=True,
        text=True,
        cwd=pathlib.Path(__file__).parent.parent / "rust",
    )
    forbidden = find_forbidden(result.stdout)
    if forbidden:
        print(f"client feature pulls in forbidden crates: {', '.join(forbidden)}")
        return 1
    print("client feature dependency check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
