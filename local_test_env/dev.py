#!/usr/bin/env python3

"""
Micromegas Development Environment Startup Script
Usage: python3 dev.py [debug|release]
"""

import sys
import os
import subprocess
import argparse
from pathlib import Path

SESSION = "micromegas"
RUST_DIR = "../rust"

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
    
    os.chdir(RUST_DIR)
    run_command(f"cargo build {build_flags}")
    os.chdir("../local_test_env")

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
        (3, "Admin")
    ]
    
    for pane_num, label in pane_labels:
        run_command(f"tmux select-pane -t {pane_num} -T '{label}'")

def start_services():
    """Start all services in tmux panes"""
    services = [
        (0, 'echo "🐘 Starting PostgreSQL..."; cd db && python3 run.py'),
        (1, 'echo "📥 Starting Ingestion Server..."; cd ../rust && cargo run -p telemetry-ingestion-srv -- --listen-endpoint-http 127.0.0.1:9000'),
        (2, 'echo "📊 Starting Analytics Server..."; cd ../rust && cargo run -p flight-sql-srv -- --disable-auth'),
        (3, 'echo "⚙️  Starting Admin Daemon..."; cd ../rust && cargo run -p telemetry-admin -- crond')
    ]
    
    for pane_num, command in services:
        run_command(f"tmux send-keys -t {pane_num} '{command}' C-m")

def create_dev_window(build_mode):
    """Create additional development window"""
    run_command(f"tmux new-window -t {SESSION} -n dev")
    run_command(f"tmux send-keys -t {SESSION}:dev 'cd ../rust && echo \"Development window - build mode: {build_mode}\"' C-m")

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
        create_dev_window(build_mode)
        attach_session()
        
    except subprocess.CalledProcessError as e:
        print(f"Error: Command failed with exit code {e.returncode}")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nInterrupted by user")
        sys.exit(1)

if __name__ == "__main__":
    main()