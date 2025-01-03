#!/bin/python3
import subprocess
import pathlib
import rust_command
import os
import shutil

docker_root = pathlib.Path(__file__).parent.parent.absolute() / "docker"

def run_docker_command(cmd):
    print("cmd=", cmd, "cwd=", docker_root)
    subprocess.run(cmd, shell=True, cwd=docker_root, check=True)

def main():
    rust_command.run_command("cargo build --release")
    target_dir = os.environ["CARGO_TARGET_DIR"]
    shutil.copyfile(
        os.path.join(target_dir, "release", "telemetry-ingestion-srv"),
        os.path.join(docker_root, "telemetry-ingestion-srv"),
    )
    shutil.copyfile(
        os.path.join(target_dir, "release", "flight-sql-srv"),
        os.path.join(docker_root, "flight-sql-srv"),
    )
    run_docker_command("docker build . --tag marcantoinedesroches/micromegas:0.3")
    run_docker_command("docker push marcantoinedesroches/micromegas:0.3")


main()
