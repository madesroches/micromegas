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

def setup_environment():
    """Set up environment variables"""
    env_vars = {
        "MICROMEGAS_FLIGHTSQL_URL": "grpc://127.0.0.1:50051",
        "MICROMEGAS_AUTH_TOKEN": "",  # Empty for no-auth mode
    }
    
    for key, default_value in env_vars.items():
        if key not in os.environ:
            os.environ[key] = default_value
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
        # Start backend server
        print_status("Starting Rust backend server...", "info")
        backend_proc = subprocess.Popen(
            ["cargo", "run", "--bin", "analytics-web-srv", "--", "--port", "8000"],
            cwd="rust",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )
        processes.append(backend_proc)
        
        # Wait for backend to start
        time.sleep(3)
        
        # Check if backend started successfully
        if backend_proc.poll() is not None:
            print_status("Backend server failed to start", "error")
            return 1
        
        # Start frontend dev server
        print_status("Starting Next.js development server...", "info")
        
        # Check if node_modules exists, install if not
        frontend_dir = Path("analytics-web-app")
        if not (frontend_dir / "node_modules").exists():
            print_status("Installing Node.js dependencies...", "info")
            npm_install = subprocess.run(
                ["npm", "install"],
                cwd=frontend_dir,
                check=True
            )
        
        # Start dev server
        frontend_proc = subprocess.Popen(
            ["npm", "run", "dev"],
            cwd=frontend_dir,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )
        processes.append(frontend_proc)
        
        # Print status
        print()
        print_status("Analytics Web App is starting up!", "success")
        print()
        print_status("Frontend (dev): http://localhost:3001", "info")
        print_status("Backend API:    http://localhost:8000/api", "info") 
        print_status("Health Check:   http://localhost:8000/api/health", "info")
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