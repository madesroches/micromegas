#!/usr/bin/env python3
"""Analytics Web App Docker Start Script

Runs the analytics-web-app using local Docker images instead of building from source.
Useful for testing production-like deployments locally.
"""

import argparse
import subprocess
import sys
import time
import signal
import os
from pathlib import Path


def print_status(message, status_type="info"):
    """Print colored status messages"""
    colors = {
        "info": "\033[94m",      # Blue
        "success": "\033[92m",   # Green
        "warning": "\033[93m",   # Yellow
        "error": "\033[91m",     # Red
        "reset": "\033[0m"       # Reset
    }

    icons = {
        "info": "🚀",
        "success": "✅",
        "warning": "⚠️",
        "error": "❌"
    }

    color = colors.get(status_type, colors["info"])
    icon = icons.get(status_type, "📍")
    reset = colors["reset"]

    print(f"{color}{icon} {message}{reset}")


def check_docker():
    """Check if Docker is available and running"""
    try:
        result = subprocess.run(
            ["docker", "info"],
            capture_output=True,
            check=True
        )
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        return False


def check_image_exists(image):
    """Check if a Docker image exists locally"""
    try:
        result = subprocess.run(
            ["docker", "image", "inspect", image],
            capture_output=True,
            check=True
        )
        return True
    except subprocess.CalledProcessError:
        return False


def check_port_in_use(port):
    """Check if a port is already in use"""
    import socket
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(1)
        result = sock.connect_ex(('localhost', port))
        sock.close()
        return result == 0
    except Exception:
        return False


def check_flightsql_server(host, port):
    """Check if FlightSQL server is running"""
    import socket
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(2)
        result = sock.connect_ex((host, port))
        sock.close()
        return result == 0
    except Exception:
        return False


def stop_existing_container(container_name):
    """Stop and remove existing container if it exists"""
    try:
        subprocess.run(
            ["docker", "stop", container_name],
            capture_output=True,
            check=False
        )
        subprocess.run(
            ["docker", "rm", container_name],
            capture_output=True,
            check=False
        )
        return True
    except Exception:
        return False


def normalize_base_path(path):
    """Normalize base path to have leading slash and no trailing slash."""
    if not path:
        return ""
    if not path.startswith("/"):
        path = "/" + path
    return path.rstrip("/")


def main():
    parser = argparse.ArgumentParser(
        description="Start Analytics Web App using Docker images"
    )
    parser.add_argument(
        "--disable-auth",
        action="store_true",
        help="Disable authentication"
    )
    parser.add_argument(
        "--image",
        default="marcantoinedesroches/micromegas-analytics-web:latest",
        help="Docker image to use (default: marcantoinedesroches/micromegas-analytics-web:latest)"
    )
    parser.add_argument(
        "--port",
        type=int,
        default=int(os.environ.get("MICROMEGAS_WEB_PORT", "3000")),
        help="Port to expose the web app on (default: 3000)"
    )
    parser.add_argument(
        "--flightsql-host",
        default=os.environ.get("MICROMEGAS_FLIGHTSQL_HOST", "host.docker.internal"),
        help="FlightSQL server host (default: host.docker.internal for local services)"
    )
    parser.add_argument(
        "--flightsql-port",
        type=int,
        default=int(os.environ.get("MICROMEGAS_FLIGHTSQL_PORT", "50051")),
        help="FlightSQL server port (default: 50051)"
    )
    parser.add_argument(
        "--base-path",
        default=os.environ.get("MICROMEGAS_BASE_PATH", ""),
        help="Base path for sub-path deployment (e.g., /micromegas). Required by Docker image."
    )
    parser.add_argument(
        "--detach", "-d",
        action="store_true",
        help="Run container in detached mode"
    )
    args = parser.parse_args()

    container_name = "micromegas-analytics-web"

    print_status("Starting Analytics Web App (Docker)", "info")
    print()

    # Check Docker is available
    if not check_docker():
        print_status("Docker is not available. Please install and start Docker.", "error")
        return 1

    # Check image exists
    if not check_image_exists(args.image):
        print_status(f"Docker image not found: {args.image}", "error")
        print_status("Build it with: docker build -f docker/analytics-web.Dockerfile -t marcantoinedesroches/micromegas-analytics-web:latest .", "info")
        return 1

    print_status(f"Using image: {args.image}", "info")

    # Normalize and validate base path (required for Docker image)
    base_path = normalize_base_path(args.base_path)
    if not base_path:
        print_status("--base-path is required for Docker deployment (e.g., --base-path /micromegas)", "error")
        return 1
    print_status(f"Using base path: {base_path}", "info")

    # Check FlightSQL server (on host machine)
    # For Docker, we need to check if it's accessible from host first
    local_flightsql_host = "127.0.0.1" if args.flightsql_host == "host.docker.internal" else args.flightsql_host
    if not check_flightsql_server(local_flightsql_host, args.flightsql_port):
        print_status(f"FlightSQL server not detected on {local_flightsql_host}:{args.flightsql_port}", "warning")
        print_status("Make sure to start your micromegas services first:", "info")
        print_status("  python3 local_test_env/ai_scripts/start_services.py", "info")
        print()

    # Check if port is in use
    if check_port_in_use(args.port):
        print_status(f"Port {args.port} is already in use", "warning")
        # Try to stop existing container
        stop_existing_container(container_name)
        time.sleep(1)
        if check_port_in_use(args.port):
            print_status(f"Port {args.port} is still in use by another process", "error")
            return 1

    # Stop any existing container with same name
    stop_existing_container(container_name)

    # Build docker run command
    docker_cmd = [
        "docker", "run",
        "--name", container_name,
        "-p", f"{args.port}:3000",
        "--add-host", "host.docker.internal:host-gateway",  # Allow container to reach host services
    ]

    # Environment variables
    env_vars = {
        "MICROMEGAS_FLIGHTSQL_URL": f"grpc://{args.flightsql_host}:{args.flightsql_port}",
        "MICROMEGAS_AUTH_TOKEN": os.environ.get("MICROMEGAS_AUTH_TOKEN", ""),
        "MICROMEGAS_WEB_CORS_ORIGIN": f"http://localhost:{args.port}",
        "MICROMEGAS_BASE_PATH": base_path,  # Required by image, can be empty
    }

    # Add OIDC config if available
    if "MICROMEGAS_OIDC_CONFIG" in os.environ:
        env_vars["MICROMEGAS_OIDC_CONFIG"] = os.environ["MICROMEGAS_OIDC_CONFIG"]
        print_status("OIDC authentication enabled", "success")

    # Add state secret for OAuth
    if "MICROMEGAS_STATE_SECRET" in os.environ:
        env_vars["MICROMEGAS_STATE_SECRET"] = os.environ["MICROMEGAS_STATE_SECRET"]
    else:
        import secrets
        env_vars["MICROMEGAS_STATE_SECRET"] = secrets.token_urlsafe(32)

    # Add redirect URI for OAuth callback
    env_vars["MICROMEGAS_AUTH_REDIRECT_URI"] = os.environ.get(
        "MICROMEGAS_AUTH_REDIRECT_URI",
        f"http://localhost:{args.port}{base_path}/auth/callback"
    )

    for key, value in env_vars.items():
        docker_cmd.extend(["-e", f"{key}={value}"])

    # Add detach flag if requested
    if args.detach:
        docker_cmd.append("-d")
    else:
        docker_cmd.append("--rm")  # Remove container on stop when not detached

    # Add image
    docker_cmd.append(args.image)

    # Add command arguments (base path is env var only, not CLI arg)
    cmd_args = ["--port", "3000", "--frontend-dir", "/app/frontend"]
    if args.disable_auth or "MICROMEGAS_OIDC_CONFIG" not in os.environ:
        cmd_args.append("--disable-auth")
        print_status("Authentication disabled", "warning")

    docker_cmd.extend(cmd_args)

    print_status("Starting Docker container...", "info")
    print()

    # Signal handler for cleanup
    def signal_handler(signum, frame):
        print()
        print_status("Stopping container...", "info")
        subprocess.run(["docker", "stop", container_name], capture_output=True)
        print_status("Container stopped", "success")
        sys.exit(0)

    if not args.detach:
        signal.signal(signal.SIGINT, signal_handler)
        signal.signal(signal.SIGTERM, signal_handler)

    try:
        if args.detach:
            result = subprocess.run(docker_cmd, capture_output=True, text=True)
            if result.returncode != 0:
                print_status(f"Failed to start container: {result.stderr}", "error")
                return 1

            print_status("Container started in detached mode", "success")
            print()
            print_status(f"Web App: http://localhost:{args.port}{base_path}/", "info")
            print()
            print_status(f"View logs: docker logs -f {container_name}", "info")
            print_status(f"Stop: docker stop {container_name}", "info")
        else:
            print_status(f"Web App: http://localhost:{args.port}{base_path}/", "success")
            print_status("Press Ctrl+C to stop", "warning")
            print()

            # Run container in foreground
            subprocess.run(docker_cmd)

    except KeyboardInterrupt:
        signal_handler(None, None)
    except Exception as e:
        print_status(f"Error: {e}", "error")
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
