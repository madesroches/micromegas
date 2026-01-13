#!/usr/bin/python3
import os
import subprocess

username = os.environ.get("MICROMEGAS_DB_USERNAME")

subprocess.run(
    f"docker exec -it teledb psql -U {username} -d micromegas_app",
    shell=True,
    check=True,
)
