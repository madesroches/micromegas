#!/usr/bin/env python3
"""
Run the analytics-web container locally for testing.

Required environment variables:
    MICROMEGAS_FLIGHTSQL_URL - FlightSQL server URL
    MICROMEGAS_WEB_CORS_ORIGIN - CORS origin for the frontend
    MICROMEGAS_STATE_SECRET - Secret for OAuth state signing
    MICROMEGAS_OIDC_CONFIG - OIDC provider configuration JSON

Optional environment variables:
    MICROMEGAS_COOKIE_DOMAIN - Cookie domain for auth
    MICROMEGAS_SECURE_COOKIES - Set to 'true' for HTTPS
    MICROMEGAS_AUTH_REDIRECT_URI - OAuth callback URL
"""

import argparse
import subprocess
import sys

DOCKERHUB_USER = "marcantoinedesroches"
IMAGE_NAME = f"{DOCKERHUB_USER}/micromegas-analytics-web"

# Environment variables to pass through
ENV_VARS = [
    "MICROMEGAS_FLIGHTSQL_URL",
    "MICROMEGAS_WEB_CORS_ORIGIN",
    "MICROMEGAS_STATE_SECRET",
    "MICROMEGAS_OIDC_CONFIG",
    "MICROMEGAS_COOKIE_DOMAIN",
    "MICROMEGAS_SECURE_COOKIES",
    "MICROMEGAS_AUTH_REDIRECT_URI",
]


def main():
    parser = argparse.ArgumentParser(description="Run analytics-web container")
    parser.add_argument(
        "--port", "-p",
        default="3000",
        help="Host port to bind (default: 3000)"
    )
    parser.add_argument(
        "--tag", "-t",
        default="latest",
        help="Image tag (default: latest)"
    )
    parser.add_argument(
        "--detach", "-d",
        action="store_true",
        help="Run in background"
    )
    parser.add_argument(
        "--disable-auth",
        action="store_true",
        help="Disable authentication (development only)"
    )

    args = parser.parse_args()

    cmd = ["docker", "run", "--rm"]

    if args.detach:
        cmd.append("-d")
    else:
        cmd.append("-it")

    # Port mapping
    cmd.extend(["-p", f"{args.port}:3000"])

    # Pass environment variables
    for var in ENV_VARS:
        cmd.extend(["-e", var])

    # Image and command
    cmd.append(f"{IMAGE_NAME}:{args.tag}")

    if args.disable_auth:
        cmd.append("--disable-auth")

    print(f">>> {' '.join(cmd)}")
    result = subprocess.run(cmd)
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
