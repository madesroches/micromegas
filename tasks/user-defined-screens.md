# User-Defined Screens Feature

## Overview

Add customizable screens to analytics-web-app. Users can take existing screen types (processes, log viewer, metrics), edit the SQL, and save custom configurations. All screens visible to all users.

## Architecture Decisions

- **Database**: New `micromegas_app` database in same PostgreSQL cluster (Aurora in prod, teledb container locally)
- **Storage**: SQL database (not S3) - screens are small JSON configs needing relational queries
- **Backend**: Add sqlx for direct PostgreSQL, keep FlightSQL for analytics queries

---

## Phase 1: Database Infrastructure

### 1.1 Add Database Module to analytics-web-srv

**New module**: `rust/analytics-web-srv/src/app_db/`

```
rust/analytics-web-srv/src/app_db/
  mod.rs
  schema.rs      # Table creation SQL
  migration.rs   # Version-based migration (follow ingestion pattern)
  models.rs      # Rust structs
```

Dependencies already available in analytics-web-srv: sqlx, uuid, chrono, serde

### 1.2 Schema

Screen types are code-driven (not stored in DB) - they define component rendering, default SQL, and variables. Only user-created screens are persisted.

```sql
CREATE TABLE migration (version INTEGER NOT NULL);
INSERT INTO migration VALUES (1);

CREATE TABLE screens (
    name VARCHAR(255) PRIMARY KEY,  -- unique, used in URLs
    screen_type VARCHAR(50) NOT NULL,
    config JSONB NOT NULL,
    created_by VARCHAR(255),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_screens_screen_type ON screens(screen_type);
```

**Navigation model:**
- New screen → `/screen/new/:type` (factory creates default, user edits & saves)
- Saved screen → `/screen/:name` (e.g., `/screen/my-error-logs`)

**ScreenFactory (registry pattern):**

```rust
// rust/analytics-web-srv/src/screen_factory.rs
pub trait ScreenType: Send + Sync {
    fn name(&self) -> &str;
    fn icon(&self) -> &str;
    fn make_default(&self) -> serde_json::Value;
}

pub struct ScreenFactory {
    types: HashMap<String, Box<dyn ScreenType>>,
}

impl ScreenFactory {
    pub fn new() -> Self { ... }
    pub fn register(&mut self, screen_type: Box<dyn ScreenType>) { ... }
    pub fn list_types(&self) -> Vec<ScreenTypeInfo> { ... }
    pub fn make_default(&self, name: &str) -> Option<serde_json::Value> { ... }
}
```

**API endpoints:**
- `GET /screen-types` - list registered types
- `GET /screen-types/:name/default` - factory creates default screen of that type

### 1.3 Local Dev Setup

**Update**: `local_test_env/ai_scripts/start_services.py`
- After PostgreSQL starts, create `micromegas_app` database if not exists
- New env var: `MICROMEGAS_APP_SQL_CONNECTION_STRING`

### 1.4 Files to Modify/Create

| File | Action |
|------|--------|
| `rust/analytics-web-srv/src/app_db/mod.rs` | Create |
| `rust/analytics-web-srv/src/app_db/schema.rs` | Create |
| `rust/analytics-web-srv/src/app_db/migration.rs` | Create |
| `rust/analytics-web-srv/src/app_db/models.rs` | Create |
| `local_test_env/ai_scripts/start_services.py` | Add DB creation |

---

## Phase 2: Backend API

### 2.1 Add PostgreSQL Pool to analytics-web-srv

**Modify**: `rust/analytics-web-srv/src/main.rs`
- Add `app_db_pool: sqlx::PgPool` to state
- Initialize from `MICROMEGAS_APP_SQL_CONNECTION_STRING`
- Run migrations on startup

### 2.2 REST Endpoints

| Method | Path | Handler |
|--------|------|---------|
| GET | /screen-types | list registered types from factory |
| GET | /screen-types/:type/default | factory.make_default(type) |
| GET | /screens | list user screens |
| GET | /screens/:name | get screen by name |
| POST | /screens | create screen |
| PUT | /screens/:name | update screen |
| DELETE | /screens/:name | delete screen |

### 2.3 Files to Modify/Create

| File | Action |
|------|--------|
| `rust/analytics-web-srv/src/main.rs` | Add PgPool, ScreenFactory, routes |
| `rust/analytics-web-srv/src/screen_factory.rs` | Create (trait, factory, type impls) |
| `rust/analytics-web-srv/src/screens.rs` | Create (CRUD handlers) |

---

## Phase 3: Frontend - Screen Browser

### 3.1 API Client

**New file**: `analytics-web-app/src/lib/screens-api.ts`
- `getScreenTypes()` - list registered types
- `getDefaultScreen(typeName)` - get default config from factory
- `getScreens()`, `createScreen()`, `updateScreen()`, `deleteScreen()`

### 3.2 Screen Browser Page

**New file**: `analytics-web-app/src/app/screens/page.tsx`
- Grid of all screens grouped by type
- Click to open, "Create New" per type

### 3.3 Updated Sidebar

**Modify**: `analytics-web-app/src/components/layout/Sidebar.tsx`
- Fetch screens from API
- Show system screens + link to /screens browser
- Dynamic icons based on screen_type

### 3.4 Files to Modify/Create

| File | Action |
|------|--------|
| `analytics-web-app/src/lib/screens-api.ts` | Create |
| `analytics-web-app/src/app/screens/page.tsx` | Create |
| `analytics-web-app/src/components/layout/Sidebar.tsx` | Modify |

---

## Phase 4: Frontend - Screen Viewer/Editor

### 4.1 Dynamic Screen Viewer

**New file**: `analytics-web-app/src/app/screen/[id]/page.tsx`
- Load screen config from API
- Render based on `component_type`:
  - `table`: Reuse QueryEditor + table pattern from processes
  - `metrics`: Reuse TimeSeriesChart pattern
  - `trace`: Reuse trace generation pattern
- "Edit SQL" button opens editor
- "Save As" creates copy with new name

### 4.2 Screen Renderer Component

**New file**: `analytics-web-app/src/components/ScreenRenderer.tsx`
- Props: `screen`, `onSaveAs`
- Switches on component_type
- Handles variable substitution in SQL

### 4.3 Save As Dialog

**New file**: `analytics-web-app/src/components/SaveScreenDialog.tsx`
- Modal with name, description inputs
- Calls createScreen API

### 4.4 Files to Create

| File | Action |
|------|--------|
| `analytics-web-app/src/app/screen/[id]/page.tsx` | Create |
| `analytics-web-app/src/components/ScreenRenderer.tsx` | Create |
| `analytics-web-app/src/components/SaveScreenDialog.tsx` | Create |

---

## Phase 5: Variables System

### 5.1 Variable Handling

Variables come from multiple sources (priority order):
1. URL params (`?process_id=xxx`)
2. Screen's saved variable values
3. Screen type's default values

### 5.2 Variable Bar Component

**New file**: `analytics-web-app/src/components/VariableBar.tsx`
- Horizontal bar showing current variable values
- Click to edit each variable
- Changes update URL params

### 5.3 Backend Variable Substitution

**Modify**: `rust/analytics-web-srv/src/main.rs` (query handler)
- Accept variables in query request
- Substitute `$var` patterns before executing SQL

### 5.4 Files to Modify/Create

| File | Action |
|------|--------|
| `analytics-web-app/src/components/VariableBar.tsx` | Create |
| `analytics-web-app/src/app/screen/[id]/page.tsx` | Add variable handling |
| `rust/analytics-web-srv/src/main.rs` | Enhance query substitution |

---

## Phase 6: Replace Existing Pages

### 6.1 Remove Hardcoded Pages

Delete and replace with screen-based routing:
- Delete `app/processes/page.tsx` → use `/screen/:id` for processes screen
- Delete `app/process_log/page.tsx` → use `/screen/:id` for log screen
- Delete `app/process_metrics/page.tsx` → use `/screen/:id` for metrics screen
- Delete `app/process_trace/page.tsx` → use `/screen/:id` for trace screen
- Update `/` redirect to go to default processes screen

### 6.2 UI Polish

- Loading states for screen list
- Error handling for failed screen loads
- Confirmation dialog for delete
- Sorting screens by name/date

---

## Key Files Reference

**Backend patterns**:
- `rust/ingestion/src/sql_migration.rs` - Migration pattern to follow
- `rust/analytics-web-srv/src/main.rs` - Route registration pattern
- `rust/analytics-web-srv/src/auth.rs` - Auth middleware

**Frontend patterns**:
- `analytics-web-app/src/app/processes/page.tsx` - QueryEditor integration
- `analytics-web-app/src/components/layout/Sidebar.tsx` - Navigation
- `analytics-web-app/src/lib/api.ts` - API client pattern

**Dev setup**:
- `local_test_env/ai_scripts/start_services.py` - Service startup
