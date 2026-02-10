#!/usr/bin/env python3
"""Analytics Web App Development Start Script"""

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

def check_command_exists(command):
    """Check if a command exists in PATH"""
    try:
        subprocess.run([command, "--version"],
                      capture_output=True,
                      check=True)
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        return False

def check_yarn_installed():
    """Check if yarn is installed"""
    if not check_command_exists("yarn"):
        print_status("Yarn not found. Installing yarn...", "info")
        try:
            subprocess.run(["npm", "install", "-g", "yarn"], check=True)
            print_status("Yarn installed successfully", "success")
            return True
        except subprocess.CalledProcessError:
            print_status("Failed to install yarn. Please install it manually:", "error")
            print_status("npm install -g yarn", "info")
            return False
    return True

def check_flightsql_server(port):
    """Check if FlightSQL server is running on the given port"""
    try:
        # FlightSQL is gRPC, not HTTP, but we can try to connect to the port
        import socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(2)
        result = sock.connect_ex(('127.0.0.1', port))
        sock.close()
        return result == 0
    except:
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

def kill_existing_backend():
    """Kill any existing analytics-web-srv processes"""
    try:
        # Find and kill analytics-web-srv processes
        result = subprocess.run(
            ["pkill", "-f", "analytics-web-srv"],
            capture_output=True,
            text=True
        )
        if result.returncode == 0:
            print_status("Killed existing analytics-web-srv process", "info")
            time.sleep(1)  # Give it a moment to shut down
            return True
        return False
    except Exception:
        return False

def kill_existing_frontend():
    """Kill any existing Vite dev servers"""
    try:
        # Find and kill vite dev processes
        result = subprocess.run(
            ["pkill", "-f", "vite"],
            capture_output=True,
            text=True
        )
        if result.returncode == 0:
            print_status("Killed existing Vite dev server", "info")
            time.sleep(1)  # Give it a moment to shut down
            return True
        return False
    except Exception:
        return False

def normalize_base_path(path):
    """Normalize base path to have leading slash and no trailing slash.

    Args:
        path: The base path string (e.g., 'mm', '/mm', '/mm/')

    Returns:
        Normalized path (e.g., '/mm') or empty string if no path
    """
    if not path:
        return ""
    # Add leading slash if missing
    if not path.startswith("/"):
        path = "/" + path
    # Remove trailing slashes
    return path.rstrip("/")


def setup_environment():
    """Set up environment variables

    Returns:
        tuple: (auth_enabled: bool, base_path: str, backend_port: int, frontend_port: int)
            - auth_enabled: True if OIDC auth is configured, False if running without auth
            - base_path: Normalized base path (e.g., '/mm' or '')
            - backend_port: Port for the backend server
            - frontend_port: Port for the frontend dev server
    """
    # Generate a random secret for development if not set
    import secrets
    dev_secret = secrets.token_urlsafe(32)

    # Handle base path - defaults to /mmlocal for local dev to emphasize it's dynamic
    base_path = normalize_base_path(os.environ.get("MICROMEGAS_BASE_PATH", "/mmlocal"))
    # Update the environment variable with normalized value
    os.environ["MICROMEGAS_BASE_PATH"] = base_path
    print_status(f"Using base path: {base_path}", "info")

    # Port configuration
    backend_port = int(os.environ.get("MICROMEGAS_BACKEND_PORT", "8000"))
    frontend_port = int(os.environ.get("MICROMEGAS_FRONTEND_PORT", "3000"))
    # Set port env vars for Vite to read
    os.environ["MICROMEGAS_BACKEND_PORT"] = str(backend_port)
    os.environ["MICROMEGAS_FRONTEND_PORT"] = str(frontend_port)

    # Build app database connection string for user-defined screens
    db_username = os.environ.get("MICROMEGAS_DB_USERNAME")
    if not db_username:
        print_status("MICROMEGAS_DB_USERNAME environment variable not set", "error")
        print_status("Run: export MICROMEGAS_DB_USERNAME=telemetry", "info")
        sys.exit(1)
    db_passwd = os.environ.get("MICROMEGAS_DB_PASSWD")
    if not db_passwd:
        print_status("MICROMEGAS_DB_PASSWD environment variable not set", "error")
        sys.exit(1)
    db_port = os.environ.get("MICROMEGAS_DB_PORT")
    if not db_port:
        print_status("MICROMEGAS_DB_PORT environment variable not set", "error")
        print_status("Run: export MICROMEGAS_DB_PORT=6432", "info")
        sys.exit(1)
    app_db_conn_string = f"postgres://{db_username}:{db_passwd}@127.0.0.1:{db_port}/micromegas_app"

    env_vars = {
        "MICROMEGAS_AUTH_TOKEN": "",  # Empty for no-auth mode
        "MICROMEGAS_WEB_CORS_ORIGIN": f"http://localhost:{frontend_port}",  # Frontend origin for CORS
        # OAuth callback URL must include base_path so browser URL matches cookie path
        "MICROMEGAS_AUTH_REDIRECT_URI": f"http://localhost:{frontend_port}{base_path}/auth/callback",
        "MICROMEGAS_STATE_SECRET": dev_secret,  # Random secret for OAuth state signing
        # App database for user-defined screens (optional - screens disabled if not set)
        "MICROMEGAS_APP_SQL_CONNECTION_STRING": app_db_conn_string,
    }

    for key, default_value in env_vars.items():
        if key not in os.environ:
            os.environ[key] = default_value
            if key == "MICROMEGAS_AUTH_TOKEN":
                if default_value:
                    print_status("Auth token provided", "info")
                else:
                    print_status("No auth token (development mode)", "info")
            elif key == "MICROMEGAS_STATE_SECRET":
                print_status(f"Set {key}=<generated>", "info")
            elif key == "MICROMEGAS_APP_SQL_CONNECTION_STRING":
                print_status("Screens feature enabled (micromegas_app database)", "success")
            else:
                print_status(f"Set {key}={default_value}", "info")

    # Check if OIDC config is available for authentication
    oidc_configured = "MICROMEGAS_OIDC_CONFIG" in os.environ
    if oidc_configured:
        print_status("MICROMEGAS_OIDC_CONFIG found - authentication enabled", "success")
    else:
        print_status("MICROMEGAS_OIDC_CONFIG not set - will run with --disable-auth", "warning")

    return oidc_configured, base_path, backend_port, frontend_port

def main():
    parser = argparse.ArgumentParser(description="Start Analytics Web App Development Environment")
    parser.add_argument("--disable-auth", action="store_true", help="Disable authentication even if OIDC config is present")
    args = parser.parse_args()

    print_status("Starting Analytics Web App Development Environment", "info")
    print_status("Telemetry data exploration and analysis platform", "info")
    print()

    # Check prerequisites
    if not check_command_exists("cargo"):
        print_status("Cargo not found. Please install Rust.", "error")
        return 1

    if not check_command_exists("node"):
        print_status("Node.js not found. Please install Node.js 18+.", "error")
        return 1

    if not check_yarn_installed():
        return 1

    # Setup environment first to get port configuration
    auth_enabled, base_path, backend_port, frontend_port = setup_environment()

    # Check FlightSQL server
    flightsql_port = int(os.environ.get("MICROMEGAS_FLIGHTSQL_PORT", "50051"))
    if not check_flightsql_server(flightsql_port):
        print_status(f"FlightSQL server not detected on port {flightsql_port}", "warning")
        print_status("Make sure to start your micromegas services first:", "info")
        print_status("python3 local_test_env/ai_scripts/start_services.py", "info")
        print()

    # Override auth if --disable-auth flag is passed
    if args.disable_auth:
        auth_enabled = False
        print_status("Authentication disabled via --disable-auth flag", "warning")

    # Change to micromegas root directory
    micromegas_dir = Path(__file__).parent.parent
    os.chdir(micromegas_dir)

    processes = []

    def cleanup():
        """Clean up background processes"""
        print()
        print_status("Shutting down services...", "info")
        for proc in processes:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        print_status("All services stopped", "success")

    def signal_handler(signum, frame):
        cleanup()
        sys.exit(0)

    # Set up signal handlers
    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)

    try:
        # Kill any existing analytics-web-srv processes first
        kill_existing_backend()

        # Kill any existing Next.js dev servers
        kill_existing_frontend()

        # Check if backend port is still in use (by something else)
        if check_port_in_use(backend_port):
            print_status(f"Port {backend_port} is already in use", "error")
            print_status(f"Another service is running on port {backend_port}", "error")
            print_status(f"Please stop the service using port {backend_port} or set MICROMEGAS_BACKEND_PORT", "warning")
            return 1

        # Start backend server
        print_status("Starting Rust backend server...", "info")
        backend_cmd = ["cargo", "run", "--bin", "analytics-web-srv", "--", "--port", str(backend_port)]
        if not auth_enabled:
            backend_cmd.append("--disable-auth")
        backend_proc = subprocess.Popen(
            backend_cmd,
            cwd="rust"
        )
        processes.append(backend_proc)

        # Wait for backend to start with health check polling
        print_status("Waiting for backend to start...", "info")
        backend_ready = False
        max_attempts = 90  # 90 seconds max (allows for slow builds)

        for attempt in range(max_attempts):
            # Check if backend process died
            if backend_proc.poll() is not None:
                print_status("Backend server failed to start", "error")

                # Show backend error output
                try:
                    stdout, stderr = backend_proc.communicate(timeout=1)
                    if stderr:
                        print_status("Backend error output:", "error")
                        print(stderr.decode('utf-8'))
                    if stdout:
                        print_status("Backend stdout:", "info")
                        print(stdout.decode('utf-8'))
                except Exception as e:
                    print_status(f"Could not get backend output: {e}", "warning")

                print()
                return 1            # Try health check (health endpoint is at {base_path}/health)
            try:
                import urllib.request
                import urllib.error

                health_url = f"http://localhost:{backend_port}{base_path}/health"
                response = urllib.request.urlopen(health_url, timeout=1)
                if response.status == 200:
                    backend_ready = True
                    print_status("Backend server is ready!", "success")
                    break
            except (urllib.error.URLError, urllib.error.HTTPError, OSError):
                # Backend not ready yet, wait and try again
                pass

            time.sleep(1)

        if not backend_ready:
            print_status("Backend server health check timed out after 90 seconds", "error")
            print()
            print_status("The backend process is running but not responding to health checks", "error")
            return 1

        # Start frontend dev server
        print_status("Starting Vite development server...", "info")

        # Check if node_modules exists, install if not
        frontend_dir = Path("analytics-web-app")
        if not (frontend_dir / "node_modules").exists():
            print_status("Installing Node.js dependencies...", "info")
            yarn_install = subprocess.run(
                ["yarn", "install"],
                cwd=frontend_dir,
                check=True
            )

        frontend_proc = subprocess.Popen(
            ["yarn", "dev"],
            cwd=frontend_dir
        )
        processes.append(frontend_proc)

        # Print status
        print()
        print_status("Analytics Web App is starting up!", "success")
        print()
        print_status(f"Frontend:       http://localhost:{frontend_port}{base_path}/", "info")
        print_status(f"Backend:        http://localhost:{backend_port}{base_path}/", "info")
        if base_path:
            print_status(f"Note: Using base path '{base_path}'", "info")
        print()
        print_status("Press Ctrl+C to stop all services", "warning")

        # Wait for processes
        while True:
            time.sleep(1)

            # Check if any process died
            for proc in processes:
                if proc.poll() is not None:
                    print_status(f"Process {proc.pid} exited unexpectedly", "error")
                    cleanup()
                    return 1

    except KeyboardInterrupt:
        cleanup()
        return 0
    except Exception as e:
        print_status(f"Error: {e}", "error")
        cleanup()
        return 1

if __name__ == "__main__":
    sys.exit(main())