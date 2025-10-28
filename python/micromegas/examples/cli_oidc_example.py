#!/usr/bin/env python3
"""
Example demonstrating CLI OIDC authentication usage.

This script shows how environment variables enable OIDC authentication
for CLI tools without requiring code changes.
"""

import os
import sys

# Example environment variable configuration for OIDC
example_env_vars = """
# Required for OIDC authentication
export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"
export MICROMEGAS_OIDC_CLIENT_ID="your-client-id.apps.googleusercontent.com"

# Optional - only needed for Web app OAuth clients (not for Desktop/Native apps)
export MICROMEGAS_OIDC_CLIENT_SECRET="your-client-secret"

# Optional - custom token file location (default: ~/.micromegas/tokens.json)
export MICROMEGAS_TOKEN_FILE="~/.config/micromegas/tokens.json"

# Optional - analytics server URI (default: grpc://localhost:50051)
export MICROMEGAS_ANALYTICS_URI="grpc+tls://analytics.example.com:50051"
"""

usage_flow = """
First-time usage flow:
----------------------
1. Set environment variables (see above)
2. Run any CLI tool (e.g., python -m micromegas.cli.query_processes)
3. Browser opens for authentication
4. After successful login, tokens saved to ~/.micromegas/tokens.json
5. Query executes

Subsequent usage:
-----------------
1. Run CLI tools as normal
2. Tokens automatically loaded from file
3. Tokens auto-refresh if expiring soon (5-min buffer)
4. No browser interaction needed

Logout:
-------
Clear saved tokens:
$ micromegas_logout

Or manually delete token file:
$ rm ~/.micromegas/tokens.json

Backward compatibility:
-----------------------
Corporate wrapper still works (takes precedence):
export MICROMEGAS_PYTHON_MODULE_WRAPPER="your_corporate_module"
"""


def main():
    print("=" * 70)
    print("OIDC Authentication for Micromegas CLI Tools")
    print("=" * 70)
    print()
    print("Environment Variables Configuration:")
    print("-" * 70)
    print(example_env_vars)
    print()
    print("Usage Flow:")
    print("-" * 70)
    print(usage_flow)
    print()
    print("Example Commands:")
    print("-" * 70)
    print("# Set up environment")
    print('export MICROMEGAS_OIDC_ISSUER="https://accounts.google.com"')
    print('export MICROMEGAS_OIDC_CLIENT_ID="123-abc.apps.googleusercontent.com"')
    print()
    print("# First time - opens browser")
    print("python -m micromegas.cli.query_processes --since 1h")
    print()
    print("# Subsequent calls - no browser")
    print("python -m micromegas.cli.query_process_log <process-id>")
    print()
    print("# Clear tokens")
    print("micromegas_logout")
    print()
    print("All existing CLI tools work without modification!")
    print("=" * 70)


if __name__ == "__main__":
    main()
