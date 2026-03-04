#!/usr/bin/python3
import subprocess
import os

username = os.environ.get("MICROMEGAS_DB_USERNAME")
port = os.environ.get("MICROMEGAS_DB_PORT", "5432")

subprocess.run(
    "psql -h localhost -p {port} -U {username}".format(port=port, username=username),
    shell=True,
    check=True,
)
