#!/usr/bin/python3
import subprocess
import os

username = os.environ.get("MICROMEGAS_DB_USERNAME")

subprocess.run(
    "psql -h localhost -p 5432 -U {username}".format(username=username),
    shell=True,
    check=True,
)
