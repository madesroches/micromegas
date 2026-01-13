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

### Shared Utilities Module

**File**: `local_test_env/db/utils.py`

Central module containing database utilities used by all startup scripts:

```python
#!/usr/bin/env python3
"""Shared database utilities for local development environment."""

import os
import subprocess
import sys

APP_DATABASE_NAME = "micromegas_app"


def get_db_username():
    """Get database username from environment, exit if not set."""
    username = os.environ.get("MICROMEGAS_DB_USERNAME")
    if not username:
        print("❌ MICROMEGAS_DB_USERNAME environment variable not set")
        sys.exit(1)
    return username


def ensure_app_database():
    """Create micromegas_app database if it doesn't exist."""
    username = get_db_username()

    # Connect to default postgres database to check if micromegas_app exists
    result = subprocess.run(
        f"docker exec teledb psql -U {username} -tc \"SELECT 1 FROM pg_database WHERE datname = '{APP_DATABASE_NAME}'\"",
        shell=True,
        capture_output=True,
        text=True,
    )

    if "1" not in result.stdout:
        print(f"Creating {APP_DATABASE_NAME} database...")
        subprocess.run(
            f'docker exec teledb psql -U {username} -c "CREATE DATABASE {APP_DATABASE_NAME}"',
            shell=True,
            check=True,
        )
        print(f"✅ {APP_DATABASE_NAME} database created")
    else:
        print(f"✅ {APP_DATABASE_NAME} database already exists")
```

### Startup Scripts Integration

All startup scripts import from the shared module:

**`local_test_env/ai_scripts/start_services.py`**:
```python
sys.path.insert(0, str(Path(__file__).parent.parent))
from db.utils import ensure_app_database
```

**`local_test_env/ai_scripts/start_services_with_oidc.py`**:
```python
sys.path.insert(0, str(Path(__file__).parent.parent))
from db.utils import ensure_app_database
```

**`local_test_env/dev.py`**:
```python
from db.utils import ensure_app_database
```

Each script calls `ensure_app_database()` after PostgreSQL is ready.

### Database Connection Script

**File**: `local_test_env/db/connect_app.py`

```python
#!/usr/bin/env python3
from utils import get_db_username, APP_DATABASE_NAME
import subprocess

username = get_db_username()

subprocess.run(
    f"docker exec -it teledb psql -U {username} -d {APP_DATABASE_NAME}",
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
| `local_test_env/db/utils.py` | Shared database utilities (ensure_app_database, get_db_username) |
| `local_test_env/db/connect_app.py` | Connect to micromegas_app via psql |

### Modified Files

| File | Changes |
|------|---------|
| `local_test_env/ai_scripts/start_services.py` | Import and call `ensure_app_database()` from shared module |
| `local_test_env/ai_scripts/start_services_with_oidc.py` | Import and call `ensure_app_database()` from shared module |
| `local_test_env/dev.py` | Import and call `ensure_app_database()` from shared module |

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
   cd local_test_env/db && python3 connect_app.py
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
