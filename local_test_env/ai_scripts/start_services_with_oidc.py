#!/usr/bin/env python3

"""
Start micromegas services with OIDC authentication enabled

This script starts the flight-sql-srv with OIDC authentication configured
for any OIDC-compliant identity provider.

Prerequisites:
1. Set MICROMEGAS_OIDC_CONFIG environment variable
2. Services must be built (cargo build in rust/ directory)

Usage:
    # Source a pre-configured auth script
    . ~/set_human_auth.sh    # For Auth0
    . ~/set_azure_auth.sh    # For Azure AD

    # Then start services
    python3 start_services_with_oidc.py

Optional:
    export MICROMEGAS_ADMINS='["your-email@example.com"]'
"""

import os
import sys
import subprocess
import time
import json
import requests
import docker
from pathlib import Path


def check_env_vars():
    """Check required environment variables"""
    oidc_config = os.environ.get("MICROMEGAS_OIDC_CONFIG")

    if not oidc_config:
        print("❌ Error: MICROMEGAS_OIDC_CONFIG environment variable not set")
        print()
        print("Please set your OIDC configuration:")
        print(
            '  export MICROMEGAS_OIDC_CONFIG=\'{"issuers": [{"issuer": "...", "audience": "..."}], ...}\''
        )
        print()
        print("Or source a pre-configured auth script:")
        print("  . ~/set_human_auth.sh    # For Auth0")
        print("  . ~/set_azure_auth.sh    # For Azure AD")
        print()
        sys.exit(1)

    print("📝 Note: Server doesn't need OIDC_CLIENT_SECRET")
    print("   The server validates tokens using public keys (JWKS) from the issuer")
    print("   Only OAuth clients need the secret to obtain tokens")
    print()


def get_oidc_config():
    """Get OIDC configuration from environment variable"""
    return os.environ["MICROMEGAS_OIDC_CONFIG"]


def kill_services():
    """Kill any existing services"""
    services = ["telemetry-ingestion-srv", "flight-sql-srv", "telemetry-admin"]
    for service in services:
        try:
            subprocess.run(f"pkill -f {service}", shell=True, check=False)
        except Exception:
            pass
    time.sleep(2)


def check_postgres_running():
    """Check if PostgreSQL Docker container is running"""
    try:
        client = docker.from_env()
        containers = client.containers.list(filters={"name": "teledb"})
        return len(containers) > 0
    except Exception:
        return False


def wait_for_service(url, max_attempts=30, service_name="Service"):
    """Wait for a service to be ready"""
    print(f"⏳ Waiting for {service_name}...")
    for i in range(1, max_attempts + 1):
        try:
            response = requests.get(url, timeout=1)
            if response.status_code in [200, 404]:
                print(f"✅ {service_name} is ready!")
                return True
        except Exception:
            pass

        if i == max_attempts:
            print(f"❌ {service_name} failed to start")
            print(
                f"   Check logs: tail -f /tmp/{service_name.lower().replace(' ', '_')}.log"
            )
            return False
        time.sleep(1)
    return False


def ensure_app_database():
    """Create micromegas_app database if it doesn't exist"""
    username = os.environ.get("MICROMEGAS_DB_USERNAME")

    # Connect to default postgres database to check if micromegas_app exists
    result = subprocess.run(
        f"docker exec teledb psql -U {username} -tc \"SELECT 1 FROM pg_database WHERE datname = 'micromegas_app'\"",
        shell=True,
        capture_output=True,
        text=True,
    )

    if "1" not in result.stdout:
        print("Creating micromegas_app database...")
        subprocess.run(
            f'docker exec teledb psql -U {username} -c "CREATE DATABASE micromegas_app"',
            shell=True,
            check=True,
        )
        print("✅ micromegas_app database created")
    else:
        print("✅ micromegas_app database already exists")


def main():
    print("🔐 Starting Micromegas services with OIDC authentication")
    print()

    # Check environment
    check_env_vars()

    # Get paths
    script_dir = Path(__file__).parent.absolute()
    rust_dir = script_dir.parent.parent / "rust"

    # Get OIDC config
    oidc_config = get_oidc_config()
    print("📝 OIDC Configuration:")
    print(json.dumps(json.loads(oidc_config), indent=2))
    print()

    # Set environment variables
    env = os.environ.copy()
    # MICROMEGAS_OIDC_CONFIG already set in environment, no need to override
    env["MICROMEGAS_ENABLE_CPU_TRACING"] = "true"

    if "MICROMEGAS_ADMINS" in os.environ:
        print(f"👑 Admin users: {os.environ['MICROMEGAS_ADMINS']}")
        print()

    # Build services
    print("🔧 Building services...")
    os.chdir(rust_dir)
    result = subprocess.run(["cargo", "build"], env=env)
    if result.returncode != 0:
        print("❌ Build failed")
        sys.exit(1)

    print("🚀 Starting services...")
    print()

    # Kill any existing services
    kill_services()

    # Start PostgreSQL if not running
    print("🐘 Checking PostgreSQL...")
    postgres_pid = None
    if not check_postgres_running():
        # Check if container exists but is stopped
        client = docker.from_env()
        containers = client.containers.list(all=True, filters={"name": "teledb"})
        if len(containers) > 0:
            # Container exists, just start it
            print("Starting existing PostgreSQL container...")
            container = containers[0]
            container.start()
            print("PostgreSQL container started")
        else:
            # Container doesn't exist, run the setup script
            print("Creating new PostgreSQL container...")
            db_dir = script_dir.parent / "db"
            os.chdir(db_dir)
            postgres_process = subprocess.Popen(["python3", "run.py"])
            postgres_pid = postgres_process.pid
            print(f"PostgreSQL PID: {postgres_pid}")
        time.sleep(5)
    else:
        print("PostgreSQL already running")

    # Ensure the app database exists
    ensure_app_database()
    print()

    os.chdir(rust_dir)

    # Start Ingestion Server (no auth)
    print("📥 Starting Ingestion Server (no auth)...")
    with open("/tmp/ingestion.log", "w") as log_file:
        ingestion_process = subprocess.Popen(
            [
                "cargo",
                "run",
                "-p",
                "telemetry-ingestion-srv",
                "--",
                "--listen-endpoint-http",
                "127.0.0.1:9000",
                "--disable-auth",
            ],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=env,
        )
    ingestion_pid = ingestion_process.pid
    print(f"Ingestion Server PID: {ingestion_pid}")

    # Wait for ingestion server
    if not wait_for_service(
        "http://127.0.0.1:9000/health", service_name="Ingestion Server"
    ):
        sys.exit(1)
    print()

    # Start Analytics Server WITH OIDC AUTH
    print("📊 Starting Analytics Server (WITH OIDC AUTH)...")
    with open("/tmp/analytics.log", "w") as log_file:
        analytics_process = subprocess.Popen(
            ["cargo", "run", "-p", "flight-sql-srv"],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=env,
        )
    analytics_pid = analytics_process.pid
    print(f"Analytics Server PID: {analytics_pid}")
    # Give analytics server time to start
    time.sleep(5)
    print()

    # Start Admin Daemon
    print("⚙️  Starting Admin Daemon...")
    with open("/tmp/admin.log", "w") as log_file:
        admin_process = subprocess.Popen(
            ["cargo", "run", "-p", "telemetry-admin", "--", "crond"],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=env,
        )
    admin_pid = admin_process.pid
    print(f"Admin Daemon PID: {admin_pid}")
    print()

    # Summary
    print("=" * 70)
    print("🎉 All services started with OIDC authentication enabled!")
    print("=" * 70)
    print()
    print("📥 Ingestion Server: http://127.0.0.1:9000 (no auth)")
    print("📊 Analytics Server: grpc://127.0.0.1:50051 (OIDC auth required)")
    print()
    print("🔐 Authentication: See OIDC config above")
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
    print("  tail -f /tmp/analytics.log   # Watch for OIDC auth events")
    print("  tail -f /tmp/admin.log")
    print()

    # Save PIDs
    pids = [str(ingestion_pid), str(analytics_pid), str(admin_pid)]
    if postgres_pid:
        pids.append(str(postgres_pid))

    with open("/tmp/micromegas_pids.txt", "w") as f:
        f.write(" ".join(pids))

    print(f"To stop services: kill {' '.join(pids)}")
    print("Or run: python3 stop_services.py")
    print()
    print("=" * 70)
    print("Next steps:")
    print("  1. Run: python3 test_oidc_auth.py")
    print("  2. Browser will open for OIDC authentication")
    print("  3. Tokens saved to ~/.micromegas/tokens.json")
    print("=" * 70)


if __name__ == "__main__":
    main()
