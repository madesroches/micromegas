#!/usr/bin/env python3

"""
Stop and remove the local MinIO container created by start_minio.py.

PostgreSQL's container is left running across sessions by convention (see
local_test_env/db/run.py); MinIO follows the same pattern in start_minio.py,
so this is the explicit opt-in teardown, mirroring local_test_env/db/delete.py.

Usage: python3 stop_minio.py
"""

import docker

CONTAINER_NAME = "micromegas-minio"


def main():
    client = docker.from_env()
    try:
        container = client.containers.get(CONTAINER_NAME)
        container.remove(force=True)
        print(f"✅ removed {CONTAINER_NAME}")
    except docker.errors.NotFound:
        print(f"no {CONTAINER_NAME} container found")


if __name__ == "__main__":
    main()
