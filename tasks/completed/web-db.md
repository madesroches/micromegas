# Web App Database Setup

## Status: Complete

- [x] Create shared database utilities module (`local_test_env/db/utils.py`)
- [x] Add `ensure_app_database()` function with environment variable validation
- [x] Update `start_services.py` to auto-create `micromegas_app` database
- [x] Update `start_services_with_oidc.py` to auto-create `micromegas_app` database
- [x] Update `dev.py` to auto-create `micromegas_app` database
- [x] Add `connect_app.py` utility script for psql access

**Next steps:** See [user-defined-screens.md](user-defined-screens.md) Phase 1 & 2 for:
- Schema/migrations for `micromegas_app`
- `MICROMEGAS_APP_SQL_CONNECTION_STRING` environment variable
- `analytics-web-srv` database connection

---

## Overview

Add a second database (`micromegas_app`) in the existing PostgreSQL container for the analytics web app to store custom screen/dashboard definitions.

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

## Implementation Details

### Shared Utilities Module

**File**: `local_test_env/db/utils.py`

Central module containing database utilities used by all startup scripts:

- `APP_DATABASE_NAME` - constant for database name
- `get_db_username()` - gets username from env with validation
- `ensure_app_database()` - creates database if it doesn't exist

### Startup Scripts Integration

All startup scripts import from the shared module and call `ensure_app_database()` after PostgreSQL is ready:

| Script | Import |
|--------|--------|
| `local_test_env/ai_scripts/start_services.py` | `from db.utils import ensure_app_database` |
| `local_test_env/ai_scripts/start_services_with_oidc.py` | `from db.utils import ensure_app_database` |
| `local_test_env/dev.py` | `from db.utils import ensure_app_database` |

### Database Connection Script

**File**: `local_test_env/db/connect_app.py`

Utility to connect to the app database via psql for debugging.

---

## Environment Variables

| Variable | Value | Description |
|----------|-------|-------------|
| `MICROMEGAS_DB_USERNAME` | (existing) | PostgreSQL username |
| `MICROMEGAS_DB_PASSWD` | (existing) | PostgreSQL password |
| `MICROMEGAS_DB_PORT` | (existing) | PostgreSQL port |
| `MICROMEGAS_APP_SQL_CONNECTION_STRING` | `postgres://user:pass@localhost:5432/micromegas_app` | Connection for analytics-web-srv (see [user-defined-screens.md](user-defined-screens.md)) |

---

## Files Summary

### New Files

| File | Purpose |
|------|---------|
| `local_test_env/db/utils.py` | Shared database utilities |
| `local_test_env/db/connect_app.py` | Connect to micromegas_app via psql |

### Modified Files

| File | Changes |
|------|---------|
| `local_test_env/ai_scripts/start_services.py` | Import and call `ensure_app_database()` |
| `local_test_env/ai_scripts/start_services_with_oidc.py` | Import and call `ensure_app_database()` |
| `local_test_env/dev.py` | Import and call `ensure_app_database()` |

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
