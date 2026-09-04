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

Admin membership lives in the `admins` local group, not an env var (the
MICROMEGAS_ADMINS family is no longer read) -- see
mkdocs/docs/admin/groups.md. On a fresh DB the v10 migration seeds `admins`
with a wildcard ('*') member, making every authenticated caller an admin
until you take over with:
    micromegas-groups --url http://localhost:3000 add admins user:<you>
    micromegas-groups --url http://localhost:3000 remove admins '*'
"""

import os
import sys
import subprocess
import time
import json
import hashlib
import secrets
import requests
import docker
from pathlib import Path

# Add parent directory to path to import shared utilities
sys.path.insert(0, str(Path(__file__).parent.parent))
from db.utils import ensure_app_database, get_db_username


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


def generate_local_ingestion_key():
    """Generate a random, local-only ingestion API key for this dev session.

    Enabling auth on the ingestion server (below) means every
    `#[micromegas_main]` binary's own self-telemetry — ingestion, flight-sql,
    and the maintenance daemon all POST to port 9000 — needs a credential too:
    the server-side `MICROMEGAS_OIDC_CONFIG` these processes already share does
    not feed the *sink* side of `#[micromegas_main]`, only
    `MICROMEGAS_INGESTION_API_KEY` (or the OIDC client-credentials trio) does.
    """
    return "mmk_" + secrets.token_urlsafe(32)


def kill_services():
    """Kill any existing services"""
    services = ["telemetry-ingestion-srv", "flight-sql-srv", "telemetry-maintenance-srv"]
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


def wait_for_migration_v6(username, max_attempts=200, interval_secs=0.1):
    """Polls the data lake until its schema has reached migration v6.

    `ingestion_api_keys` is created in v5, but its `audience` column -- which the seed INSERT
    below needs -- is only added in v6, and `execute_migration` commits each version in its own
    transaction, so polling for the table's mere existence could land between the two commits on
    a fresh database. This also tolerates `relation "migration" does not exist` on a fresh
    database, since the ingestion binary creates that table itself.

    `wait_for_service`'s 1-second granularity is too coarse here -- the sink's own retry schedule
    starts at ~10ms, so the real floor on how fast this can observe the row is the latency of one
    `psql` exec, not a fixed sleep.
    """
    print("⏳ Waiting for the data lake schema to reach migration v6...")
    for attempt in range(1, max_attempts + 1):
        result = subprocess.run(
            f"docker exec teledb psql -U {username} -tc "
            '"SELECT version FROM migration ORDER BY version DESC LIMIT 1"',
            shell=True,
            capture_output=True,
            text=True,
        )
        version_str = result.stdout.strip()
        if version_str:
            try:
                if int(version_str) >= 6:
                    print(f"✅ Schema at migration v{version_str}")
                    return True
            except ValueError:
                pass
        # An empty result covers both "relation does not exist" (fresh database, the ingestion
        # binary has not created it yet) and "no rows yet" -- either way, just keep polling.
        if attempt == max_attempts:
            print("❌ Data lake schema never reached migration v6")
            return False
        time.sleep(interval_secs)
    return False


def seed_ingestion_key(username, key):
    """Seeds a live `ingestion_api_keys` row for `key`, revoking any row this script minted on a
    previous run first -- `name` is not unique, so a fresh INSERT alone would accumulate one live
    credential per run.
    """
    key_hash = hashlib.sha256(key.encode()).hexdigest()
    subprocess.run(
        f"docker exec teledb psql -U {username} -c \""
        "UPDATE ingestion_api_keys SET revoked_at = now(), revoked_by = 'start_services_with_oidc' "
        "WHERE name = 'local-self-telemetry' AND revoked_at IS NULL\"",
        shell=True,
        check=True,
    )
    subprocess.run(
        f"docker exec teledb psql -U {username} -c \""
        "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience) "
        f"VALUES (gen_random_uuid(), decode('{key_hash}','hex'), 'local-self-telemetry', now(), "
        "'start_services_with_oidc', 'public')\"",
        shell=True,
        check=True,
    )


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

    # Self-telemetry sink: without these, the services don't report their own logs/metrics
    # anywhere, the same default-if-unset block `start_services.py` sets for split mode.
    if not env.get("MICROMEGAS_TELEMETRY_URL"):
        env["MICROMEGAS_TELEMETRY_URL"] = "http://127.0.0.1:9000"
        print("Set MICROMEGAS_TELEMETRY_URL=http://127.0.0.1:9000")
    if not env.get("MICROMEGAS_FLUSH_PERIOD"):
        env["MICROMEGAS_FLUSH_PERIOD"] = "5"
        print("Set MICROMEGAS_FLUSH_PERIOD=5")

    # Provision a local ingestion credential: the ingestion server now runs
    # WITH auth (see below), so every service's own self-telemetry needs a
    # credential — set MICROMEGAS_INGESTION_API_KEY in the shared env, passed
    # to all three service processes, before any of them start.
    local_ingestion_key = generate_local_ingestion_key()
    env["MICROMEGAS_INGESTION_API_KEY"] = local_ingestion_key
    print("🔑 Provisioned a local self-telemetry ingestion key for this session")
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

    # Start Ingestion Server (WITH auth): the local self-telemetry key
    # provisioned above is seeded into the `ingestion_api_keys` DB table below,
    # so it accepts that key from the other processes' self-telemetry sinks.
    print("📥 Starting Ingestion Server (WITH auth)...")
    ingestion_env = env.copy()
    # Widens the residual race between the ingestion process binding its listener and the seed
    # row landing in the table (see `wait_for_migration_v6`/`seed_ingestion_key` below) into a
    # ~10s window of 401s instead of a single miss, if it fires at all in the dev path.
    ingestion_env["MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS"] = "0"
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
            ],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=ingestion_env,
        )
    ingestion_pid = ingestion_process.pid
    print(f"Ingestion Server PID: {ingestion_pid}")

    # Seed the local self-telemetry key into `ingestion_api_keys` as early as possible relative
    # to the sink's own `insert_process` attempts: `connect_to_remote_data_lake` (which runs
    # `execute_migration` and commits v6 in its own transaction) executes before
    # `serve_ingestion` binds the listener, so polling from here normally lands the row before
    # the port ever opens, leaving only a narrow residual race on the very first request.
    db_username = get_db_username()
    if wait_for_migration_v6(db_username):
        seed_ingestion_key(db_username, local_ingestion_key)
        print("🔑 Seeded the local self-telemetry key into ingestion_api_keys")
    else:
        print("❌ Could not seed the local self-telemetry key -- schema never reached v6")
        sys.exit(1)
    print()

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

    # Start Maintenance Daemon
    print("⚙️  Starting Maintenance Daemon...")
    with open("/tmp/daemon.log", "w") as log_file:
        maintenance_process = subprocess.Popen(
            ["cargo", "run", "-p", "telemetry-maintenance-srv"],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=env,
        )
    maintenance_pid = maintenance_process.pid
    print(f"Maintenance Daemon PID: {maintenance_pid}")
    print()

    # Summary
    print("=" * 70)
    print("🎉 All services started with OIDC authentication enabled!")
    print("=" * 70)
    print()
    print(
        "📥 Ingestion Server: http://127.0.0.1:9000 (auth enabled — local API key + OIDC)"
    )
    print("📊 Analytics Server: grpc://127.0.0.1:50051 (OIDC auth required)")
    print()
    print("🔐 Authentication: See OIDC config above")
    print()
    print("PIDs:")
    print(f"  Ingestion: {ingestion_pid}")
    print(f"  Analytics: {analytics_pid}")
    print(f"  Maintenance: {maintenance_pid}")
    if postgres_pid:
        print(f"  PostgreSQL: {postgres_pid}")
    print()
    print("Logs:")
    print("  tail -f /tmp/ingestion.log")
    print("  tail -f /tmp/analytics.log   # Watch for OIDC auth events")
    print("  tail -f /tmp/daemon.log")
    print()

    # Save PIDs
    pids = [str(ingestion_pid), str(analytics_pid), str(maintenance_pid)]
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
