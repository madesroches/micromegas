#!/usr/bin/env python3
"""Simple script to start the MkDocs development server."""

import os
import subprocess
import sys
from pathlib import Path

# Get the mkdocs directory (where this script is located)
MKDOCS_DIR = Path(__file__).parent.absolute()
REPO_ROOT = MKDOCS_DIR.parent
VENV_MKDOCS = REPO_ROOT / "docs-venv" / "bin" / "mkdocs"
CONFIG_FILE = MKDOCS_DIR / "mkdocs.yml"

def main():
    """Start the MkDocs development server."""

    # Check if venv mkdocs exists
    if not VENV_MKDOCS.exists():
        print(f"Error: MkDocs not found at {VENV_MKDOCS}", file=sys.stderr)
        print("Please run: python3 -m venv docs-venv && docs-venv/bin/pip install mkdocs mkdocs-material mkdocstrings", file=sys.stderr)
        sys.exit(1)

    # Check if config file exists
    if not CONFIG_FILE.exists():
        print(f"Error: Config file not found at {CONFIG_FILE}", file=sys.stderr)
        sys.exit(1)

    # Build command
    cmd = [
        str(VENV_MKDOCS),
        "serve",
        "--config-file", str(CONFIG_FILE),
        "--dev-addr", "0.0.0.0:8765"
    ]

    print(f"Starting MkDocs server...")
    print(f"Server will be available at: http://localhost:8765")
    print(f"Press Ctrl+C to stop")
    print()

    try:
        subprocess.run(cmd, check=True)
    except KeyboardInterrupt:
        print("\nServer stopped.")
    except subprocess.CalledProcessError as e:
        print(f"Error running MkDocs: {e}", file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()
