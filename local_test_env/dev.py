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
from pathlib import Path

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
    pane_labels = [
        (0, "PostgreSQL"),
        (1, "Ingestion"),
        (2, "Analytics"),
        (3, "Daemon")
    ]
    
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

def start_services():
    """Start all services in tmux panes with proper sequencing"""
    # Start PostgreSQL first
    print("🐘 Starting PostgreSQL...")
    run_command(f"tmux send-keys -t 0 'echo \"🐘 Starting PostgreSQL...\"; cd db && python3 run.py' C-m")
    
    # Start Ingestion Server and wait for it to be ready
    print("📥 Starting Ingestion Server...")
    run_command(f"tmux send-keys -t 1 'echo \"📥 Starting Ingestion Server...\"; cd ../rust && cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000' C-m")
    
    # Wait for ingestion service to be ready
    if not wait_for_service("http://127.0.0.1:9000/health", "Ingestion Server"):
        print("⚠️  Warning: Ingestion server may not be ready, continuing anyway...")
    
    # Start remaining services
    remaining_services = [
        (2, 'echo "📊 Starting Analytics Server..."; cd ../rust && cargo run -p flight-sql-srv -- --disable-auth'),
        (3, 'echo "😈 Starting Daemon..."; cd ../rust && cargo run -p telemetry-admin -- crond')
    ]
    
    for pane_num, command in remaining_services:
        print(f"Starting service in pane {pane_num}...")
        run_command(f"tmux send-keys -t {pane_num} '{command}' C-m")
        time.sleep(1)  # Small delay between service starts

def attach_session():
    """Attach to tmux session"""
    run_command(f"tmux attach-session -t {SESSION}")

def main():
    parser = argparse.ArgumentParser(description="Start Micromegas development environment")
    parser.add_argument("build_mode", nargs="?", default="debug", 
                       choices=["debug", "release"],
                       help="Build mode (default: debug)")
    
    args = parser.parse_args()
    build_mode = args.build_mode
    
    try:
        kill_existing_session()
        build_rust_services(build_mode)
        create_tmux_session()
        start_services()
        attach_session()
        
    except subprocess.CalledProcessError as e:
        print(f"Error: Command failed with exit code {e.returncode}")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nInterrupted by user")
        sys.exit(1)

if __name__ == "__main__":
    main()
