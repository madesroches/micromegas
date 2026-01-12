# Web App Database Setup

## Overview

Add a second database (`micromegas_app`) in the existing PostgreSQL container for the analytics web app to store custom screen/dashboard definitions.

## Current State

- **Single PostgreSQL container** "teledb" on port 5432
- Contains `telemetry` database for telemetry metadata
- Environment: `MICROMEGAS_DB_USERNAME`, `MICROMEGAS_DB_PASSWD`, `MICROMEGAS_DB_PORT`

## Architecture

```
┌─────────────────────────────────────┐
│  PostgreSQL container: teledb       │
│  Port: 5432                         │
│  ┌───────────────┐ ┌──────────────┐ │
│  │   telemetry   │ │micromegas_app│ │
│  │  (processes,  │ │  (screens,   │ │
│  │   streams,    │ │   configs)   │ │
│  │   blocks)     │ │              │ │
│  └───────────────┘ └──────────────┘ │
└─────────────────────────────────────┘
         │                   │
         ▼                   ▼
┌─────────────────┐  ┌──────────────────┐
│  flight-sql-srv │  │ analytics-web-srv│
│  ingestion-srv  │  │                  │
└─────────────────┘  └──────────────────┘
```

---

## Implementation

### 1. Update `start_services.py`

**File**: `local_test_env/ai_scripts/start_services.py`

After PostgreSQL starts and is ready, create the `micromegas_app` database if it doesn't exist:

```python
def ensure_app_database():
    """Create micromegas_app database if it doesn't exist"""
    username = os.environ.get("MICROMEGAS_DB_USERNAME")
    passwd = os.environ.get("MICROMEGAS_DB_PASSWD")
    port = os.environ.get("MICROMEGAS_DB_PORT")

    # Connect to default postgres database to create new database
    result = subprocess.run(
        f'docker exec teledb psql -U {username} -tc "SELECT 1 FROM pg_database WHERE datname = \'micromegas_app\'"',
        shell=True, capture_output=True, text=True
    )

    if '1' not in result.stdout:
        print("Creating micromegas_app database...")
        subprocess.run(
            f'docker exec teledb psql -U {username} -c "CREATE DATABASE micromegas_app"',
            shell=True, check=True
        )
        print("✅ micromegas_app database created")
    else:
        print("✅ micromegas_app database already exists")
```

Call this after `check_postgres_running()` succeeds.

### 2. Update `start_services_with_oidc.py`

**File**: `local_test_env/ai_scripts/start_services_with_oidc.py`

Same changes as start_services.py.

### 3. Update `dev.py`

**File**: `local_test_env/dev.py`

Add `ensure_app_database()` call after `wait_for_postgres()` in `start_services()`.

### 4. Add Database Utility Script

**New file**: `local_test_env/db/connect_app.py`

```python
#!/usr/bin/python3
import os
import subprocess

username = os.environ.get("MICROMEGAS_DB_USERNAME")
port = os.environ.get("MICROMEGAS_DB_PORT")

subprocess.run(
    f"docker exec -it teledb psql -U {username} -d micromegas_app",
    shell=True,
    check=True,
)
```

---

## Environment Variables

New variable for analytics-web-srv:

| Variable | Value | Description |
|----------|-------|-------------|
| `MICROMEGAS_APP_SQL_CONNECTION_STRING` | `postgres://user:pass@localhost:5432/micromegas_app` | Connection to app database |

The existing `MICROMEGAS_SQL_CONNECTION_STRING` continues to point to the `telemetry` database.

---

## Files Summary

### New Files

| File | Purpose |
|------|---------|
| `local_test_env/db/connect_app.py` | Connect to micromegas_app via psql |

### Modified Files

| File | Changes |
|------|---------|
| `local_test_env/ai_scripts/start_services.py` | Add `ensure_app_database()` |
| `local_test_env/ai_scripts/start_services_with_oidc.py` | Add `ensure_app_database()` |
| `local_test_env/dev.py` | Add `ensure_app_database()` call |

---

## Verification

1. **Start services**
   ```bash
   python3 local_test_env/ai_scripts/start_services.py
   ```

2. **Verify database exists**
   ```bash
   docker exec teledb psql -U postgres -l | grep micromegas_app
   # Should show micromegas_app in database list
   ```

3. **Connect to app database**
   ```bash
   python3 local_test_env/db/connect_app.py
   # Should open psql shell to micromegas_app
   ```

4. **Test restart**
   ```bash
   python3 local_test_env/ai_scripts/stop_services.py
   python3 local_test_env/ai_scripts/start_services.py
   # Database should persist and not be recreated
   ```

---

## Out of Scope

- Schema/migrations for micromegas_app (see `tasks/user-defined-screens.md` Phase 1)
- Analytics-web-srv code changes to connect to new database
