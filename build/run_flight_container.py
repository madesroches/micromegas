#!/usr/bin/env python3
import docker_command
import os

os.environ["MICROMEGAS_TELEMETRY_URL"] = "http://localhost:9000"
os.environ["MICROMEGAS_OBJECT_STORE_URI"] = "file:///lake"

docker_command.run_docker_command(
    "docker run --network=host -v ~/lake:/lake "
    "-e MICROMEGAS_API_KEYS -e MICROMEGAS_TELEMETRY_URL "
    "-e MICROMEGAS_SQL_CONNECTION_STRING -e MICROMEGAS_OBJECT_STORE_URI "
    "-d marcantoinedesroches/micromegas-all:latest "
    "flight-sql-srv",
)
