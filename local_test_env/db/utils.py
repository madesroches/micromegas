#!/usr/bin/env python3
"""Shared database utilities for local development environment."""

import os
import subprocess
import sys

APP_DATABASE_NAME = "micromegas_app"


def get_db_username():
    """Get database username from environment, exit if not set."""
    username = os.environ.get("MICROMEGAS_DB_USERNAME")
    if not username:
        print("❌ MICROMEGAS_DB_USERNAME environment variable not set")
        sys.exit(1)
    return username


def ensure_app_database():
    """Create micromegas_app database if it doesn't exist."""
    username = get_db_username()

    # Connect to default postgres database to check if micromegas_app exists
    result = subprocess.run(
        f"docker exec teledb psql -U {username} -tc \"SELECT 1 FROM pg_database WHERE datname = '{APP_DATABASE_NAME}'\"",
        shell=True,
        capture_output=True,
        text=True,
    )

    if "1" not in result.stdout:
        print(f"Creating {APP_DATABASE_NAME} database...")
        subprocess.run(
            f'docker exec teledb psql -U {username} -c "CREATE DATABASE {APP_DATABASE_NAME}"',
            shell=True,
            check=True,
        )
        print(f"✅ {APP_DATABASE_NAME} database created")
    else:
        print(f"✅ {APP_DATABASE_NAME} database already exists")
