#!/usr/bin/python3
import subprocess

subprocess.run(
    "docker run -p 3000:3000 --name=grafana grafana/grafana-oss",
    shell=True,
    check=True,
)
