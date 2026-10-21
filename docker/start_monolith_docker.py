#!/usr/bin/env python3
"""Monolith Docker Start Script

Runs the pre-built monolith image through docker/docker-compose.monolith.yaml and prints the URLs
to open once the stack is ready.
"""

import argparse
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

PROJECT_NAME = "micromegas-monolith"
SERVICE_NAME = "micromegas"
COMPOSE_FILE = Path(__file__).resolve().parent / "docker-compose.monolith.yaml"
MIN_COMPOSE_VERSION = (2, 23, 1)
WEB_PORT = 3000
INGESTION_PORT = 9000
FLIGHTSQL_PORT = 50051
DEFAULT_TIMEOUT = 90.0


def print_status(message, status_type="info"):
    """Print colored status messages"""
    colors = {
        "info": "\033[94m",
        "success": "\033[92m",
        "warning": "\033[93m",
        "error": "\033[91m",
        "reset": "\033[0m",
    }
    icons = {"info": "🚀", "success": "✅", "warning": "⚠️", "error": "❌"}
    color = colors.get(status_type, colors["info"])
    icon = icons.get(status_type, "📍")
    print(f"{color}{icon} {message}{colors['reset']}")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(
        description="Start the Micromegas monolith using Docker images"
    )
    parser.add_argument(
        "--pull",
        action="store_true",
        help="Always pull the latest image (default: use the local image if present)",
    )
    parser.add_argument(
        "--follow",
        action="store_true",
        help="Tail logs after startup; Ctrl+C stops the stack if this run started it (keeps containers and data)",
    )
    parser.add_argument(
        "--down",
        action="store_true",
        help="Stop and remove the containers instead of starting",
    )
    parser.add_argument(
        "--volumes",
        action="store_true",
        help="With --down, also delete the data volumes",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT,
        help=f"Seconds to wait for readiness (default: {DEFAULT_TIMEOUT:g})",
    )
    args = parser.parse_args(argv)
    if args.volumes and not args.down:
        parser.error("--volumes is only valid with --down")
    return args


def build_compose_cmd(*compose_args):
    return [
        "docker",
        "compose",
        "-p",
        PROJECT_NAME,
        "-f",
        str(COMPOSE_FILE),
        *compose_args,
    ]


def build_up_cmd(pull):
    args = ["up", "-d"]
    if pull:
        args += ["--pull", "always"]
    return build_compose_cmd(*args)


def build_down_cmd(volumes):
    args = ["down"]
    if volumes:
        args.append("-v")
    return build_compose_cmd(*args)


def parse_compose_version(text):
    """Parse '2.23.1' or 'v2.23.1-desktop.1' into a comparable tuple, or None."""
    match = re.match(r"\s*v?(\d+)\.(\d+)\.(\d+)", text)
    if not match:
        return None
    return tuple(int(g) for g in match.groups())


def compose_version_ok(text):
    version = parse_compose_version(text)
    return version is not None and version >= MIN_COMPOSE_VERSION


def is_already_running(ps_output):
    return bool(ps_output.strip())


def format_summary():
    query = 'micromegas-query "SELECT time, level, msg FROM log_entries LIMIT 10" --begin 1h'
    return [
        (f"Web App: http://localhost:{WEB_PORT}/", "info"),
        (f"Ingestion: http://localhost:{INGESTION_PORT}", "info"),
        (f"FlightSQL: grpc://localhost:{FLIGHTSQL_PORT}", "info"),
        (f"Query it: {query}", "info"),
        ("View logs: " + " ".join(build_compose_cmd("logs", "-f")), "info"),
        ("Stop: " + " ".join(build_compose_cmd("stop")), "info"),
        ("Remove: python3 " + str(Path(__file__).resolve()) + " --down", "info"),
    ]


def wait_for_ready(probe, timeout, interval=1.0, sleep=time.sleep, clock=time.monotonic):
    deadline = clock() + timeout
    while True:
        if probe():
            return True
        if clock() >= deadline:
            return False
        sleep(interval)


def http_probe(url, require_ok):
    """A web server answering with any status is up; /ready must return 200."""
    try:
        with urllib.request.urlopen(url, timeout=2) as resp:
            return resp.status == 200 or not require_ok
    except urllib.error.HTTPError:
        return not require_ok
    except (urllib.error.URLError, OSError):
        return False


def stack_probe():
    return http_probe(f"http://localhost:{WEB_PORT}/", False) and http_probe(
        f"http://localhost:{INGESTION_PORT}/ready", True
    )


def run_quiet(cmd):
    return subprocess.run(cmd, capture_output=True, text=True)


def check_docker():
    try:
        subprocess.run(["docker", "info"], capture_output=True, check=True)
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        return False


def get_compose_version():
    try:
        result = run_quiet(["docker", "compose", "version", "--short"])
    except FileNotFoundError:
        return ""
    return result.stdout if result.returncode == 0 else ""


def check_port_in_use(port):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.settimeout(1)
        return sock.connect_ex(("localhost", port)) == 0
    except OSError:
        return False
    finally:
        sock.close()


def main():
    args = parse_args()

    if not check_docker():
        print_status("Docker is not available. Please install and start Docker.", "error")
        return 1

    version_text = get_compose_version()
    if not compose_version_ok(version_text):
        print_status(
            "Docker Compose v2.23.1+ is required "
            f"(found: {version_text.strip() or 'none'})",
            "error",
        )
        return 1

    if args.down:
        print_status("Removing the monolith stack...", "info")
        result = run_quiet(build_down_cmd(args.volumes))
        if result.returncode != 0:
            print_status(f"Failed to remove the stack: {result.stderr}", "error")
            return 1
        print_status("Stack removed" + (" (data volumes deleted)" if args.volumes else ""), "success")
        return 0

    print_status("Starting the Micromegas monolith (Docker)", "info")
    ps = run_quiet(build_compose_cmd("ps", "--status", "running", "-q", SERVICE_NAME))
    already_running = ps.returncode == 0 and is_already_running(ps.stdout)

    if already_running and not args.pull:
        print_status("Monolith is already running", "warning")
    else:
        if not already_running:
            for port in (WEB_PORT, INGESTION_PORT, FLIGHTSQL_PORT):
                if check_port_in_use(port):
                    print_status(f"Port {port} is already in use", "error")
                    return 1
        print_status("Starting containers...", "info")
        result = run_quiet(build_up_cmd(args.pull))
        if result.returncode != 0:
            print_status(f"Failed to start the stack: {result.stderr}", "error")
            return 1

    print_status("Waiting for the monolith to be ready...", "info")
    if not wait_for_ready(stack_probe, args.timeout):
        print_status(f"Not ready after {args.timeout:g}s; last log lines:", "error")
        logs = run_quiet(build_compose_cmd("logs", "--tail", "50", SERVICE_NAME))
        print(logs.stdout + logs.stderr)
        return 1

    print_status("Monolith is ready", "success")
    print()
    for message, status_type in format_summary():
        print_status(message, status_type)

    if args.follow:
        print()
        print_status("Following logs, press Ctrl+C to stop", "warning")
        try:
            subprocess.run(build_compose_cmd("logs", "-f"))
        except KeyboardInterrupt:
            print()
            if already_running:
                print_status("Stopped following logs; the stack is still running", "info")
            else:
                print_status("Stopping containers...", "info")
                run_quiet(build_compose_cmd("stop"))
                print_status("Containers stopped", "success")
    return 0


if __name__ == "__main__":
    sys.exit(main())
