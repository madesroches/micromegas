#!/usr/bin/env python3

"""
Simple script to start micromegas services for testing
Usage: python3 start_services.py [--release] [--monolith] [--help]
"""

import argparse
import os
import secrets
import sys
import subprocess
import time
import requests
from pathlib import Path
import signal

# Add parent directory to path to import shared utilities
sys.path.insert(0, str(Path(__file__).parent.parent))
from db.utils import ensure_app_database


def run_command(cmd, check=True, shell=True, capture_output=False):
    """Run a shell command"""
    print(f"Running: {cmd}")
    return subprocess.run(cmd, shell=shell, check=check, capture_output=capture_output)


def kill_services():
    """Kill any existing services"""
    services = ["telemetry-ingestion-srv", "flight-sql-srv", "telemetry-admin", "micromegas-monolith"]
    for service in services:
        try:
            subprocess.run(f"pkill -f {service}", shell=True, check=False)
        except Exception:
            pass
    time.sleep(2)


def check_postgres_running():
    """Check if PostgreSQL Docker container is running"""
    try:
        result = subprocess.run(
            "docker ps --filter name=teledb --filter status=running --format '{{.Names}}'",
            shell=True,
            capture_output=True,
            text=True,
        )
        return "teledb" in result.stdout
    except Exception:
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
        except Exception:
            pass

        if i == max_attempts:
            print(f"❌ {service_name} failed to start")
            return False
        time.sleep(1)
    return False


def start_split_mode(rust_dir, target_dir, postgres_pid):
    """Start the four individual services."""
    # Start Ingestion Server
    print("📥 Starting Ingestion Server...")
    with open("/tmp/ingestion.log", "w") as log_file:
        ingestion_process = subprocess.Popen(
            [
                str(target_dir / "telemetry-ingestion-srv"),
                "--listen-endpoint-http",
                "127.0.0.1:9000",
                "--disable-auth",
            ],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=os.environ.copy(),
        )
    ingestion_pid = ingestion_process.pid
    print(f"Ingestion Server PID: {ingestion_pid}")

    if not wait_for_service("http://127.0.0.1:9000/health", service_name="Ingestion Server"):
        sys.exit(1)

    # Start Analytics Server
    print("📊 Starting Analytics Server...")
    with open("/tmp/analytics.log", "w") as log_file:
        analytics_process = subprocess.Popen(
            [str(target_dir / "flight-sql-srv"), "--disable-auth"],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=os.environ.copy(),
        )
    analytics_pid = analytics_process.pid
    print(f"Analytics Server PID: {analytics_pid}")

    # Start Admin Daemon
    print("⚙️ Starting Admin Daemon...")
    with open("/tmp/admin.log", "w") as log_file:
        admin_process = subprocess.Popen(
            [str(target_dir / "telemetry-admin"), "crond"],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=os.environ.copy(),
        )
    admin_pid = admin_process.pid
    print(f"Admin Daemon PID: {admin_pid}")

    print()
    print("🎉 All services started!")
    print("📥 Ingestion Server: http://127.0.0.1:9000")
    print("📊 Analytics Server: port 50051")
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

    pids = [str(ingestion_pid), str(analytics_pid), str(admin_pid)]
    if postgres_pid:
        pids.append(str(postgres_pid))
    return pids


def start_monolith_mode(rust_dir, target_dir, postgres_pid):
    """Start a single micromegas-monolith process (all roles)."""
    print("🚀 Starting Monolith (all roles)...")
    env = os.environ.copy()
    # Point to local web app dist if available
    repo_root = rust_dir.parent
    frontend_dir = repo_root / "analytics-web-app" / "dist"

    # Web role requires these vars; set dev defaults if not already in the environment.
    web_port = int(env.get("MICROMEGAS_PORT", "3000"))
    if "MICROMEGAS_WEB_CORS_ORIGIN" not in env:
        env["MICROMEGAS_WEB_CORS_ORIGIN"] = f"http://localhost:{web_port}"
        print(f"Set MICROMEGAS_WEB_CORS_ORIGIN=http://localhost:{web_port}")
    if "MICROMEGAS_BASE_PATH" not in env:
        env["MICROMEGAS_BASE_PATH"] = "/"
        print("Set MICROMEGAS_BASE_PATH=/")
    if "MICROMEGAS_APP_SQL_CONNECTION_STRING" not in env:
        db_user = env.get("MICROMEGAS_DB_USERNAME")
        db_pass = env.get("MICROMEGAS_DB_PASSWD")
        db_port = env.get("MICROMEGAS_DB_PORT")
        if db_user and db_pass and db_port:
            conn = f"postgres://{db_user}:{db_pass}@127.0.0.1:{db_port}/micromegas_app"
            env["MICROMEGAS_APP_SQL_CONNECTION_STRING"] = conn
            print("Set MICROMEGAS_APP_SQL_CONNECTION_STRING (micromegas_app)")
        else:
            print("⚠️  MICROMEGAS_APP_SQL_CONNECTION_STRING not set (screens feature disabled)")

    if not env.get("MICROMEGAS_TELEMETRY_URL"):
        env["MICROMEGAS_TELEMETRY_URL"] = "http://127.0.0.1:9000"
        print("Set MICROMEGAS_TELEMETRY_URL=http://127.0.0.1:9000")
    if not env.get("MICROMEGAS_FLUSH_PERIOD"):
        env["MICROMEGAS_FLUSH_PERIOD"] = "5"
        print("Set MICROMEGAS_FLUSH_PERIOD=5")

    has_oidc = "MICROMEGAS_OIDC_CONFIG" in env or "MICROMEGAS_ANALYTICS_OIDC_CONFIG" in env
    if has_oidc and "MICROMEGAS_STATE_SECRET" not in env:
        env["MICROMEGAS_STATE_SECRET"] = secrets.token_hex(32)
        print("Set MICROMEGAS_STATE_SECRET (generated)")
    if has_oidc and "MICROMEGAS_AUTH_REDIRECT_URI" not in env:
        base_path = env.get("MICROMEGAS_BASE_PATH", "/").rstrip("/")
        redirect_uri = f"http://localhost:{web_port}{base_path}/auth/callback"
        env["MICROMEGAS_AUTH_REDIRECT_URI"] = redirect_uri
        print(f"Set MICROMEGAS_AUTH_REDIRECT_URI={redirect_uri}")
    auth_flag = "--disable-ingestion-auth" if has_oidc else "--disable-auth"

    cmd = [
        str(target_dir / "micromegas-monolith"),
        "--roles", "all",
        "--listen-endpoint-http", "127.0.0.1:9000",
        auth_flag,
    ]
    if frontend_dir.exists():
        cmd += ["--frontend-dir", str(frontend_dir)]

    with open("/tmp/monolith.log", "w") as log_file:
        monolith_process = subprocess.Popen(
            cmd,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=env,
        )
    monolith_pid = monolith_process.pid
    print(f"Monolith PID: {monolith_pid}")

    if not wait_for_service("http://127.0.0.1:9000/health", service_name="Monolith (ingestion)"):
        sys.exit(1)

    print()
    print("🎉 Monolith started!")
    print("📥 Ingestion: http://127.0.0.1:9000")
    print("📊 FlightSQL: port 50051")
    base_path = env.get("MICROMEGAS_BASE_PATH", "/").rstrip("/")
    web_url = f"http://localhost:{web_port}{base_path}/"
    print(f"🌐 Web app:   \033]8;;{web_url}\033\\{web_url}\033]8;;\033\\")
    print()
    print("PIDs:")
    print(f"  Monolith: {monolith_pid}")
    if postgres_pid:
        print(f"  PostgreSQL: {postgres_pid}")
    print()
    print("Log: tail -f /tmp/monolith.log")

    pids = [str(monolith_pid)]
    if postgres_pid:
        pids.append(str(postgres_pid))
    return pids


def main():
    parser = argparse.ArgumentParser(
        description="Start micromegas services for testing"
    )
    parser.add_argument(
        "--release", action="store_true", help="Build and run in release mode"
    )
    parser.add_argument(
        "--monolith", action="store_true",
        help="Start a single micromegas-monolith process instead of four separate services"
    )
    args = parser.parse_args()

    script_dir = Path(__file__).parent.absolute()
    rust_dir = script_dir.parent.parent / "rust"

    release_flag = " --release" if args.release else ""
    mode = "release" if args.release else "debug"

    # Set environment variable for CPU tracing in development
    os.environ["MICROMEGAS_ENABLE_CPU_TRACING"] = "true"
    print("🔧 CPU tracing enabled for development")

    if args.monolith:
        print(f"🔧 Building monolith ({mode})...")
        os.chdir(rust_dir)
        run_command(f"cargo build --bin micromegas-monolith{release_flag}")
    else:
        print(f"🔧 Building all services ({mode})...")
        os.chdir(rust_dir)
        run_command(
            f"cargo build --bin telemetry-ingestion-srv --bin flight-sql-srv --bin telemetry-admin{release_flag}"
        )

    print("🚀 Starting services...")
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

    ensure_app_database()
    os.chdir(rust_dir)

    cargo_target_dir = os.environ.get("CARGO_TARGET_DIR")
    if cargo_target_dir:
        target_dir = Path(cargo_target_dir) / mode
    else:
        target_dir = rust_dir / "target" / mode

    if args.monolith:
        pids = start_monolith_mode(rust_dir, target_dir, postgres_pid)
    else:
        pids = start_split_mode(rust_dir, target_dir, postgres_pid)

    with open("/tmp/micromegas_pids.txt", "w") as f:
        f.write(" ".join(pids))

    print()
    print(f"To stop services: kill {' '.join(pids)}")
    print()
    print("⏳ Waiting a moment for services to fully start...")
    time.sleep(3)
    print("✅ Ready to test!")


if __name__ == "__main__":
    main()
