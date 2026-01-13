#!/usr/bin/env python3

"""
Micromegas Development Environment Startup Script
Usage: python3 dev.py [debug|release]
"""

import sys
import os
import subprocess
import argparse
import time
import requests
import docker
from pathlib import Path

from db.utils import ensure_app_database

SESSION = "micromegas"
SCRIPT_DIR = Path(__file__).parent.absolute()
RUST_DIR = SCRIPT_DIR.parent / "rust"


def run_command(cmd, check=True, shell=True):
    """Run a shell command"""
    print(f"Running: {cmd}")
    return subprocess.run(cmd, shell=shell, check=check)


def kill_existing_session():
    """Kill existing tmux session if it exists"""
    try:
        run_command(f"tmux kill-session -t {SESSION}", check=False)
    except subprocess.CalledProcessError:
        pass


def build_rust_services(build_mode):
    """Build Rust services in specified mode"""
    build_flags = "--release" if build_mode == "release" else ""
    print(f"🔧 Building Rust services in {build_mode} mode...")

    os.chdir(str(RUST_DIR))
    run_command(f"cargo build {build_flags} -p telemetry-ingestion-srv")
    run_command(f"cargo build {build_flags} -p telemetry-admin")
    run_command(f"cargo build {build_flags} -p flight-sql-srv")
    os.chdir(str(SCRIPT_DIR))


def create_tmux_session():
    """Create and configure tmux session"""
    print("🚀 Starting services in tmux session...")

    # Create session and main window
    run_command(f"tmux new-session -d -s {SESSION} -n services")

    # Create 4-pane layout
    run_command(f"tmux split-window -h -t {SESSION}:services")
    run_command(f"tmux split-window -v -t {SESSION}:services.0")
    run_command(f"tmux split-window -v -t {SESSION}:services.2")

    # Label panes
    pane_labels = [(0, "PostgreSQL"), (1, "Ingestion"), (2, "Analytics"), (3, "Daemon")]

    for pane_num, label in pane_labels:
        run_command(f"tmux select-pane -t {pane_num} -T '{label}'")


def wait_for_service(url, service_name, timeout=60, check_interval=2):
    """Wait for a service to become available"""
    print(f"⏳ Waiting for {service_name} to be ready at {url}...")
    start_time = time.time()

    while time.time() - start_time < timeout:
        try:
            response = requests.get(url, timeout=5)
            if response.status_code < 500:  # Accept any non-server-error response
                print(f"✅ {service_name} is ready!")
                return True
        except (requests.exceptions.RequestException, requests.exceptions.Timeout):
            pass

        print(f"⏳ {service_name} not ready yet, retrying in {check_interval}s...")
        time.sleep(check_interval)

    print(f"❌ Timeout waiting for {service_name} after {timeout}s")
    return False


def start_postgres():
    """Start PostgreSQL container with proper error handling"""
    print("🐘 Starting PostgreSQL...")

    try:
        client = docker.from_env()

        # Check if container exists
        try:
            container = client.containers.get("teledb")

            if container.status == "running":
                print("✅ PostgreSQL container is already running")
                return True
            else:
                print("🔄 Starting existing PostgreSQL container...")
                container.start()
                print("✅ PostgreSQL container started successfully")
                return True

        except docker.errors.NotFound:
            print("🆕 Creating new PostgreSQL container...")
            # Build image if it doesn't exist
            if len(client.images.list(name="teledb")) == 0:
                print("🔧 Building PostgreSQL image...")
                os.chdir(str(SCRIPT_DIR / "db"))
                import build

                build.build()
                os.chdir(str(SCRIPT_DIR))

            # Get environment variables (no defaults to match original run.py behavior)
            username = os.environ.get("MICROMEGAS_DB_USERNAME")
            passwd = os.environ.get("MICROMEGAS_DB_PASSWD")
            port = os.environ.get("MICROMEGAS_DB_PORT")

            # Create and start container
            container = client.containers.run(
                "teledb",
                name="teledb",
                environment={"POSTGRES_PASSWORD": passwd, "POSTGRES_USER": username},
                ports={"5432/tcp": int(port)},
                detach=True,
            )
            print("✅ PostgreSQL container created and started")
            return True

    except Exception as e:
        print(f"❌ Failed to manage PostgreSQL container: {e}")
        return False


def wait_for_postgres(timeout=15):
    """Wait for PostgreSQL to be ready using Docker API with active polling"""
    print("⏳ Waiting for PostgreSQL to be ready...")

    try:
        client = docker.from_env()
        container = client.containers.get("teledb")

        # Get the correct username from environment
        username = os.environ.get("MICROMEGAS_DB_USERNAME", "postgres")

        max_attempts = int(timeout * 2)  # 0.5s intervals
        for attempt in range(max_attempts):
            try:
                exit_code, output = container.exec_run(f"pg_isready -U {username}")
                if exit_code == 0:
                    print("✅ PostgreSQL is ready!")
                    return True
            except Exception:
                pass
            time.sleep(0.5)

        print(f"❌ PostgreSQL failed to become ready within {timeout} seconds")
        return False

    except Exception as e:
        print(f"❌ Failed to check PostgreSQL readiness: {e}")
        return False


def start_services(build_mode):
    """Start all services in tmux panes with proper sequencing"""
    run_flags = "--release" if build_mode == "release" else ""

    # Start PostgreSQL first with proper container management
    if not start_postgres():
        print("❌ Failed to start PostgreSQL, exiting...")
        sys.exit(1)

    # Wait for PostgreSQL to be ready
    if not wait_for_postgres():
        print("❌ PostgreSQL failed to become ready, exiting...")
        sys.exit(1)

    # Ensure the app database exists
    ensure_app_database()

    # Show live PostgreSQL logs in the first pane
    run_command(f"tmux send-keys -t 0 'docker logs -f teledb' C-m")

    # Start Ingestion Server and wait for it to be ready
    print("📥 Starting Ingestion Server...")
    run_command(
        f"tmux send-keys -t 1 'echo \"📥 Starting Ingestion Server...\"; cd ../rust && cargo run {run_flags} -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000 --disable-auth' C-m"
    )

    # Wait for ingestion service to be ready
    if not wait_for_service("http://127.0.0.1:9000/health", "Ingestion Server"):
        print("❌ Error: Ingestion server failed to start")
        sys.exit(1)

    # Start remaining services
    remaining_services = [
        (
            2,
            f'echo "📊 Starting Analytics Server..."; cd ../rust && cargo run {run_flags} -p flight-sql-srv -- --disable-auth',
        ),
        (
            3,
            f'echo "😈 Starting Daemon..."; cd ../rust && cargo run {run_flags} -p telemetry-admin -- crond',
        ),
    ]

    for pane_num, command in remaining_services:
        print(f"Starting service in pane {pane_num}...")
        run_command(f"tmux send-keys -t {pane_num} '{command}' C-m")
        time.sleep(1)  # Small delay between service starts


def attach_session():
    """Attach to tmux session"""
    run_command(f"tmux attach-session -t {SESSION}")


def main():
    parser = argparse.ArgumentParser(
        description="Start Micromegas development environment"
    )
    parser.add_argument(
        "build_mode",
        nargs="?",
        default="debug",
        choices=["debug", "release"],
        help="Build mode (default: debug)",
    )

    args = parser.parse_args()
    build_mode = args.build_mode

    try:
        kill_existing_session()
        build_rust_services(build_mode)
        create_tmux_session()
        start_services(build_mode)
        attach_session()

    except subprocess.CalledProcessError as e:
        print(f"Error: Command failed with exit code {e.returncode}")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nInterrupted by user")
        sys.exit(1)


if __name__ == "__main__":
    main()
