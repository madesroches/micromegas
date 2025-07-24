#!/bin/python3
import rust_command
import docker_command
import os
import shutil


def main():
    rust_command.run_command("cargo build --release")
    target_dir = os.environ["CARGO_TARGET_DIR"]
    shutil.copyfile(
        os.path.join(target_dir, "release", "telemetry-ingestion-srv"),
        os.path.join(docker_command.docker_root, "telemetry-ingestion-srv"),
    )
    shutil.copyfile(
        os.path.join(target_dir, "release", "flight-sql-srv"),
        os.path.join(docker_command.docker_root, "flight-sql-srv"),
    )
    shutil.copyfile(
        os.path.join(target_dir, "release", "telemetry-admin"),
        os.path.join(docker_command.docker_root, "telemetry-admin"),
    )
    shutil.copyfile(
        os.path.join(target_dir, "release", "pg-gateway-srv"),
        os.path.join(docker_command.docker_root, "pg-gateway-srv"),
    )
    docker_command.run_docker_command("docker build . --tag marcantoinedesroches/micromegas:0.12")
    docker_command.run_docker_command("docker push marcantoinedesroches/micromegas:0.12")


main()
