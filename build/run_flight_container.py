#!/usr/bin/env python3
import docker_command
import os

os.environ["MICROMEGAS_TELEMETRY_URL"] = "http://localhost:9000"
os.environ["MICROMEGAS_OBJECT_STORE_URI"] = "file:///lake"

docker_command.run_docker_command(
    "docker run --network=host -v ~/lake:/lake "
    # Depends on the caller's environment having MICROMEGAS_OIDC_CONFIG set, or the
    # analytics_api_keys table already populated -- flight-sql-srv no longer reads
    # MICROMEGAS_API_KEYS.
    "-e MICROMEGAS_OIDC_CONFIG -e MICROMEGAS_TELEMETRY_URL "
    "-e MICROMEGAS_SQL_CONNECTION_STRING -e MICROMEGAS_OBJECT_STORE_URI "
    "-d marcantoinedesroches/micromegas-all:latest "
    "flight-sql-srv",
)
