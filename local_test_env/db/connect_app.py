#!/usr/bin/env python3
import sys
from utils import get_db_username, APP_DATABASE_NAME
import subprocess

username = get_db_username()

subprocess.run(
    f"docker exec -it teledb psql -U {username} -d {APP_DATABASE_NAME}",
    shell=True,
    check=True,
)
