#!/usr/bin/python3
import subprocess
import docker
import build

client = docker.from_env()
containers = client.containers.list(all=True, filters={"name": "teledb"})
container = None
if len(containers) > 0:
    assert len(containers) == 1
    container = containers[0]
    subprocess.run(
        "docker container rm " + container.id,
        shell=True,
        check=True,
    )
else:
    print("no container found")
