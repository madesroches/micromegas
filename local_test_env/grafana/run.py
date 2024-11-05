#!/usr/bin/python3
import subprocess
import docker

client = docker.from_env()
containers = client.containers.list(all=True, filters={"name": "grafana"})
if len(containers) > 0:
    assert len(containers) == 1
    subprocess.run(
        "docker start -a -i grafana",
        shell=True,
        check=True,
    )
else:
    subprocess.run(
        "docker run -p 3000:3000 --name=grafana grafana/grafana-oss",
        shell=True,
        check=True,
    )
