#!/usr/bin/env python3
"""Analytics Web App Development Start Script"""

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

def check_flightsql_server():
    """Check if FlightSQL server is running"""
    try:
        # FlightSQL is gRPC, not HTTP, but we can try to connect to the port
        import socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(2)
        result = sock.connect_ex(('127.0.0.1', 50051))
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
    """Kill any existing Next.js dev servers"""
    try:
        # Find and kill next dev processes
        result = subprocess.run(
            ["pkill", "-f", "next dev"],
            capture_output=True,
            text=True
        )
        if result.returncode == 0:
            print_status("Killed existing Next.js dev server", "info")
            time.sleep(1)  # Give it a moment to shut down
            return True
        return False
    except Exception:
        return False

def setup_environment():
    """Set up environment variables"""
    # Generate a random secret for development if not set
    import secrets
    dev_secret = secrets.token_urlsafe(32)

    env_vars = {
        "MICROMEGAS_FLIGHTSQL_URL": "grpc://127.0.0.1:50051",
        "MICROMEGAS_AUTH_TOKEN": "",  # Empty for no-auth mode
        "MICROMEGAS_WEB_CORS_ORIGIN": "http://localhost:3000",  # Frontend origin for CORS
        "MICROMEGAS_AUTH_REDIRECT_URI": "http://localhost:3000/auth/callback",  # OAuth callback URL
        "MICROMEGAS_STATE_SECRET": dev_secret,  # Random secret for OAuth state signing
    }

    for key, default_value in env_vars.items():
        if key not in os.environ:
            os.environ[key] = default_value
            if key == "MICROMEGAS_AUTH_TOKEN":
                if default_value:
                    print_status("Auth token provided", "info")
                else:
                    print_status("No auth token (development mode)", "info")
            else:
                print_status(f"Set {key}={default_value}", "info")

def main():
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

    # Check FlightSQL server
    if not check_flightsql_server():
        print_status("FlightSQL server not detected on port 50051", "warning")
        print_status("Make sure to start your micromegas services first:", "info")
        print_status("python3 local_test_env/ai_scripts/start_services.py", "info")
        print()

    # Setup environment
    setup_environment()

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

        # Check if port 8000 is still in use (by something else)
        if check_port_in_use(8000):
            print_status("Port 8000 is already in use", "error")
            print_status("Another service is running on port 8000", "error")
            print_status("Please stop the service using port 8000 or use a different port", "warning")
            return 1

        # Start backend server
        print_status("Starting Rust backend server...", "info")
        backend_proc = subprocess.Popen(
            ["cargo", "run", "--bin", "analytics-web-srv", "--", "--port", "8000"],
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
                return 1            # Try health check
            try:
                import urllib.request
                import urllib.error

                response = urllib.request.urlopen("http://localhost:8000/analyticsweb/health", timeout=1)
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
        print_status("Starting Next.js development server...", "info")

        # Check if node_modules exists, install if not
        frontend_dir = Path("analytics-web-app")
        if not (frontend_dir / "node_modules").exists():
            print_status("Installing Node.js dependencies...", "info")
            yarn_install = subprocess.run(
                ["yarn", "install"],
                cwd=frontend_dir,
                check=True
            )

        # Start dev server
        frontend_proc = subprocess.Popen(
            ["yarn", "dev"],
            cwd=frontend_dir
        )
        processes.append(frontend_proc)

        # Print status
        print()
        print_status("Analytics Web App is starting up!", "success")
        print()
        print_status("Backend API:    http://localhost:8000/api", "info")
        print()
        print_status("Press Ctrl+C to stop all services", "warning")        # Wait for processes
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