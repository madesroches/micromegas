#!/usr/bin/env python3
"""Drop the micromegas_app database (for schema reset during development)."""

import subprocess
import sys

from utils import APP_DATABASE_NAME, get_db_username

username = get_db_username()

# Confirm before dropping
response = input(f"Drop database '{APP_DATABASE_NAME}'? [y/N] ")
if response.lower() != "y":
    print("Aborted")
    sys.exit(0)

# Terminate existing connections first
subprocess.run(
    f"""docker exec teledb psql -U {username} -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{APP_DATABASE_NAME}' AND pid <> pg_backend_pid();" """,
    shell=True,
    capture_output=True,
)

# Drop the database
result = subprocess.run(
    f'docker exec teledb psql -U {username} -c "DROP DATABASE IF EXISTS {APP_DATABASE_NAME}"',
    shell=True,
)

if result.returncode == 0:
    print(f"Dropped {APP_DATABASE_NAME} database")
else:
    print(f"Failed to drop {APP_DATABASE_NAME} database")
    sys.exit(1)
