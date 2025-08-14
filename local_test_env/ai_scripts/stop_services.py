#!/usr/bin/env python3

"""
Simple script to stop micromegas services
Usage: python3 stop_services.py
"""

import os
import subprocess
import signal
from pathlib import Path

def kill_pid(pid):
    """Kill a process by PID"""
    try:
        os.kill(pid, signal.SIGTERM)
        return True
    except (OSError, ProcessLookupError):
        return False

def kill_by_name(service_name):
    """Kill services by name pattern"""
    try:
        subprocess.run(f"pkill -f {service_name}", shell=True, check=False)
    except:
        pass

def main():
    print("🛑 Stopping micromegas services...")
    
    # Kill services by saved PIDs
    pids_file = Path("/tmp/micromegas_pids.txt")
    if pids_file.exists():
        try:
            pids_content = pids_file.read_text().strip()
            pids = [int(pid) for pid in pids_content.split()]
            print(f"Killing PIDs: {pids}")
            
            for pid in pids:
                kill_pid(pid)
            
            pids_file.unlink()
        except (ValueError, FileNotFoundError):
            print("Warning: Could not parse PIDs file")
    
    # Kill any remaining services by name
    services = ["telemetry-ingestion-srv", "flight-sql-srv", "telemetry-admin"]
    for service in services:
        kill_by_name(service)
    
    print("✅ Services stopped")
    
    # Clean up log files
    log_files = ["/tmp/ingestion.log", "/tmp/analytics.log", "/tmp/admin.log"]
    for log_file in log_files:
        try:
            os.remove(log_file)
        except FileNotFoundError:
            pass

if __name__ == "__main__":
    main()