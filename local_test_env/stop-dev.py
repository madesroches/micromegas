#!/usr/bin/env python3

"""
Micromegas Development Environment Stop Script
Usage: python3 dev-stop.py
"""

import subprocess
import sys

SESSION = "micromegas"

def run_command(cmd, check=True, shell=True):
    """Run a shell command"""
    print(f"Running: {cmd}")
    return subprocess.run(cmd, shell=shell, check=check, capture_output=True, text=True)

def check_session_exists():
    """Check if tmux session exists"""
    try:
        result = run_command(f"tmux has-session -t {SESSION}", check=False)
        return result.returncode == 0
    except subprocess.CalledProcessError:
        return False

def kill_session():
    """Kill the tmux session"""
    if not check_session_exists():
        print(f"No tmux session '{SESSION}' found - nothing to stop")
        return
    
    try:
        print(f"🛑 Stopping {SESSION} development environment...")
        run_command(f"tmux kill-session -t {SESSION}")
        print(f"✅ Successfully stopped {SESSION} session and all services")
    except subprocess.CalledProcessError as e:
        print(f"Error stopping session: {e}")
        sys.exit(1)

def main():
    try:
        kill_session()
    except KeyboardInterrupt:
        print("\nInterrupted by user")
        sys.exit(1)

if __name__ == "__main__":
    main()