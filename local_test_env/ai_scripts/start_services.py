#!/usr/bin/env python3

"""
Simple script to start micromegas services for testing
Usage: python3 start_services.py
"""

import os
import sys
import subprocess
import time
import requests
from pathlib import Path
import signal

def run_command(cmd, check=True, shell=True, capture_output=False):
    """Run a shell command"""
    print(f"Running: {cmd}")
    return subprocess.run(cmd, shell=shell, check=check, capture_output=capture_output)

def kill_services():
    """Kill any existing services"""
    services = ["telemetry-ingestion-srv", "flight-sql-srv", "telemetry-admin"]
    for service in services:
        try:
            subprocess.run(f"pkill -f {service}", shell=True, check=False)
        except:
            pass
    time.sleep(2)

def check_postgres_running():
    """Check if PostgreSQL is already running"""
    try:
        result = subprocess.run("pgrep -f postgres", shell=True, capture_output=True)
        return result.returncode == 0
    except:
        return False

def wait_for_service(url, max_attempts=30, service_name="Service"):
    """Wait for a service to be ready"""
    print(f"⏳ Waiting for {service_name}...")
    for i in range(1, max_attempts + 1):
        try:
            response = requests.get(url, timeout=1)
            # Accept any HTTP response (200, 404, etc.) as long as the server is responding
            if response.status_code in [200, 404]:
                print(f"✅ {service_name} is ready!")
                return True
        except:
            pass

        if i == max_attempts:
            print(f"❌ {service_name} failed to start")
            return False
        time.sleep(1)
    return False

def main():
    script_dir = Path(__file__).parent.absolute()
    rust_dir = script_dir.parent.parent / "rust"

    # Set environment variable for CPU tracing in development
    os.environ["MICROMEGAS_ENABLE_CPU_TRACING"] = "true"
    print("🔧 CPU tracing enabled for development")

    print("🔧 Building services...")
    os.chdir(rust_dir)
    run_command("cargo build")

    print("🚀 Starting services...")

    # Kill any existing services
    kill_services()

    # Start PostgreSQL if not running
    print("🐘 Checking PostgreSQL...")
    postgres_pid = None
    if not check_postgres_running():
        print("Starting PostgreSQL...")
        db_dir = script_dir.parent / "db"
        os.chdir(db_dir)
        postgres_process = subprocess.Popen(["python3", "run.py"])
        postgres_pid = postgres_process.pid
        print(f"PostgreSQL PID: {postgres_pid}")
        time.sleep(5)
    else:
        print("PostgreSQL already running")

    os.chdir(rust_dir)

    # Start Ingestion Server
    print("📥 Starting Ingestion Server...")
    with open("/tmp/ingestion.log", "w") as log_file:
        ingestion_process = subprocess.Popen([
            "cargo", "run", "-p", "telemetry-ingestion-srv", "--",
            "--listen-endpoint-http", "127.0.0.1:9000"
        ], stdout=log_file, stderr=subprocess.STDOUT, env=os.environ.copy())
    ingestion_pid = ingestion_process.pid
    print(f"Ingestion Server PID: {ingestion_pid}")

    # Wait for ingestion server to be ready
    if not wait_for_service("http://127.0.0.1:9000/health", service_name="Ingestion Server"):
        sys.exit(1)

    # Start Analytics Server
    print("📊 Starting Analytics Server...")
    with open("/tmp/analytics.log", "w") as log_file:
        analytics_process = subprocess.Popen([
            "cargo", "run", "-p", "flight-sql-srv", "--", "--disable-auth"
        ], stdout=log_file, stderr=subprocess.STDOUT, env=os.environ.copy())
    analytics_pid = analytics_process.pid
    print(f"Analytics Server PID: {analytics_pid}")

    # Start Admin Daemon
    print("⚙️ Starting Admin Daemon...")
    with open("/tmp/admin.log", "w") as log_file:
        admin_process = subprocess.Popen([
            "cargo", "run", "-p", "telemetry-admin", "--", "crond"
        ], stdout=log_file, stderr=subprocess.STDOUT, env=os.environ.copy())
    admin_pid = admin_process.pid
    print(f"Admin Daemon PID: {admin_pid}")    print()
    print("🎉 All services started!")
    print("📥 Ingestion Server: http://127.0.0.1:9000")
    print("📊 Analytics Server: port 32010")
    print()
    print("PIDs:")
    print(f"  Ingestion: {ingestion_pid}")
    print(f"  Analytics: {analytics_pid}")
    print(f"  Admin: {admin_pid}")
    if postgres_pid:
        print(f"  PostgreSQL: {postgres_pid}")
    print()
    print("Logs:")
    print("  tail -f /tmp/ingestion.log")
    print("  tail -f /tmp/analytics.log")
    print("  tail -f /tmp/admin.log")
    print()

    # Save PIDs for cleanup script
    pids = [str(ingestion_pid), str(analytics_pid), str(admin_pid)]
    if postgres_pid:
        pids.append(str(postgres_pid))

    with open("/tmp/micromegas_pids.txt", "w") as f:
        f.write(" ".join(pids))

    print(f"To stop services: kill {' '.join(pids)}")
    print()
    print("⏳ Waiting a moment for services to fully start...")
    time.sleep(3)
    print("✅ Ready to test!")

if __name__ == "__main__":
    main()