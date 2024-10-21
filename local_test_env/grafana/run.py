#!/usr/bin/python3
import subprocess

subprocess.run(
    "docker run -d -p 3000:3000 --name=grafana grafana/grafana-oss",
    shell=True,
    check=True,
)
