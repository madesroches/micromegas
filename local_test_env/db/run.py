#!/usr/bin/python3
import os
import subprocess
import docker
import build

username = os.environ.get("MICROMEGAS_DB_USERNAME")
passwd = os.environ.get("MICROMEGAS_DB_PASSWD")
port = os.environ.get("MICROMEGAS_DB_PORT")

client = docker.from_env()
if len(client.images.list(name="teledb")) == 0:
    build.build()
containers = client.containers.list(all=True, filters={"name": "teledb"})
container = None
if len(containers) > 0:
    assert len(containers) == 1
    container = containers[0]
    subprocess.run(
        "docker start -a -i teledb",
        shell=True,
        check=True,
    )
else:
    subprocess.run(
        "docker run --name teledb -e POSTGRES_PASSWORD={passwd} -e POSTGRES_USER={username} -p {port}:5432 teledb".format(
            username=username, passwd=passwd, port=port
        ),
        shell=True,
        check=True,
    )
