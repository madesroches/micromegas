#!/bin/python3
import docker_command
import os

os.environ["MICROMEGAS_TELEMETRY_URL"] = "http://localhost:9000"

docker_command.run_docker_command(
    "docker run --network=host -e MICROMEGAS_FLIGHTSQL_URL -e MICROMEGAS_TELEMETRY_URL -d marcantoinedesroches/micromegas:0.12 /http-gateway-srv",
)
