# User-Defined Screens Feature

**Issue**: https://github.com/madesroches/micromegas/issues/666

## Overview

Add customizable screens to analytics-web-app. Users can take existing screen types (processes, log viewer, metrics), edit the SQL, and save custom configurations. All screens visible to all users.

## Architecture Decisions

- **Database**: New `micromegas_app` database in same PostgreSQL cluster (Aurora in prod, teledb container locally)
- **Storage**: SQL database (not S3) - screens are small JSON configs needing relational queries
- **Backend**: Add sqlx PgPool for direct PostgreSQL to `micromegas_app`, keep FlightSQL for analytics queries
- **Frontend**: Vite + React Router (not Next.js) - uses `routes/` directory with `useParams()` for dynamic routes

---

## Phase 1: Database Infrastructure ✅ DONE

### 1.1 Add Database Module to analytics-web-srv ✅

**Created module**: `rust/analytics-web-srv/src/app_db/`

```
rust/analytics-web-srv/src/app_db/
  mod.rs
  schema.rs      # Table creation SQL
  migration.rs   # Version-based migration (follow ingestion pattern)
  models.rs      # Rust structs (Screen, CreateScreenRequest, UpdateScreenRequest, validation)
```

**Added**: sqlx dependency to `rust/analytics-web-srv/Cargo.toml`

### 1.2 Screen Name Validation ✅

Screen names are used in URLs (`/screen/:name`), so they must be URL-safe and readable.

**Implemented** in `models.rs`: `validate_screen_name()` and `normalize_screen_name()` functions.

**Validation rules:**
- 3-100 characters
- Lowercase letters, numbers, and hyphens only
- Must start with a letter
- Must end with a letter or number
- No consecutive hyphens

**Examples:**
- Valid: `error-logs`, `prod-metrics-v2`, `my-custom-screen`
- Invalid: `Error Logs` (spaces/uppercase), `-errors` (starts with hyphen), `a` (too short)

**Slug generation:**
When saving, the backend should normalize input:
- Convert to lowercase
- Replace spaces with hyphens
- Remove invalid characters
- Collapse consecutive hyphens

If the normalized name conflicts with an existing screen, return 400 with error code `DUPLICATE_NAME`.

**Reserved names:**
- `new` (used for `/screen/new` route)

### 1.3 Schema ✅

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
```

**Navigation model:**
- New screen → `/screen/new` (user selects type, then edits & saves)
- Saved screen → `/screen/:name` (e.g., `/screen/my-error-logs`)

**ScreenType (enum-based, implemented in `screen_types.rs`):**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenType {
    ProcessList,  // serializes as "process_list"
    Metrics,      // serializes as "metrics"
    Log,          // serializes as "log"
}
```

Each type provides:
- `info()` - name, icon, description for UI
- `default_config()` - default SQL/config for new screens
- `as_str()` / `FromStr` - string conversion

**API endpoints:**
- `GET /screen-types` - list all screen types
- `GET /screen-types/:type/default` - get default config for a type

### 1.4 Local Dev Setup ✅ DONE

Database creation is already implemented:
- `local_test_env/ai_scripts/start_services.py` calls `ensure_app_database()`
- `local_test_env/db/utils.py` creates `micromegas_app` database if not exists

Connection string configuration: Set `MICROMEGAS_APP_SQL_CONNECTION_STRING` env var (implemented in Phase 2).

### 1.5 Files Created ✅

| File | Status |
|------|--------|
| `rust/analytics-web-srv/src/app_db/mod.rs` | ✅ Created |
| `rust/analytics-web-srv/src/app_db/schema.rs` | ✅ Created |
| `rust/analytics-web-srv/src/app_db/migration.rs` | ✅ Created |
| `rust/analytics-web-srv/src/app_db/models.rs` | ✅ Created |
| `rust/analytics-web-srv/src/screen_types.rs` | ✅ Created |
| `rust/analytics-web-srv/src/lib.rs` | ✅ Modified (added modules) |
| `rust/analytics-web-srv/Cargo.toml` | ✅ Modified (added sqlx) |

---

## Phase 2: Backend API ✅ DONE

### 2.1 Add PostgreSQL Pool to analytics-web-srv ✅

**Modified**: `rust/analytics-web-srv/src/main.rs`

Uses `Extension` to inject the pool (matches existing `AuthToken` pattern):
```rust
// In main(), after creating the pool:
let app_db_pool: sqlx::PgPool = /* ... */;

// Add to routes via layer:
.layer(Extension(app_db_pool))

// In handlers, extract via:
Extension(pool): Extension<sqlx::PgPool>
```

**Environment variable**: `MICROMEGAS_APP_SQL_CONNECTION_STRING`
- Format: `postgres://user:pass@host:5432/micromegas_app`
- For local dev, add to `start_analytics_web.py` or use same credentials as main DB

**Startup sequence** (implemented):
1. Read connection string from env (optional - graceful degradation if not set)
2. Create PgPool
3. Run migrations
4. Add pool to screen routes via Extension layer

**Error handling**: If env var not set, screens feature is disabled with a warning. If connection/migration fails, server exits with error.

### 2.2 REST Endpoints ✅

| Method | Path | Handler |
|--------|------|---------|
| GET | /screen-types | `list_screen_types` |
| GET | /screen-types/:type_name/default | `get_default_config` |
| GET | /screens | `list_screens` |
| GET | /screens/:name | `get_screen` |
| POST | /screens | `create_screen` |
| PUT | /screens/:name | `update_screen` |
| DELETE | /screens/:name | `delete_screen` |

All endpoints return proper error responses:
- 404 for not found
- 400 for invalid input (bad screen type, duplicate name, validation errors)
- 500 for database errors

CORS updated to allow PUT and DELETE methods.

### 2.3 Files Modified/Created ✅

| File | Status |
|------|--------|
| `rust/analytics-web-srv/src/main.rs` | ✅ Modified (PgPool, routes, CORS) |
| `rust/analytics-web-srv/src/screens.rs` | ✅ Created (CRUD handlers) |

Note: `screen_types.rs` already created in Phase 1.

---

## Phase 3: Frontend - Screen Browser

### 3.1 API Client

**New file**: `analytics-web-app/src/lib/screens-api.ts`

Use existing `authenticatedFetch()` from `lib/api.ts`:
```typescript
import { authenticatedFetch } from './api'
import { getConfig } from './config'

export interface Screen {
  name: string
  screen_type: string
  config: ScreenConfig
  created_by?: string
  created_at: string
  updated_at: string
}

export async function getScreens(): Promise<Screen[]> { ... }
export async function getScreen(name: string): Promise<Screen> { ... }
export async function createScreen(screen: CreateScreenRequest): Promise<Screen> { ... }
export async function updateScreen(name: string, screen: UpdateScreenRequest): Promise<Screen> { ... }
export async function deleteScreen(name: string): Promise<void> { ... }
export async function getScreenTypes(): Promise<ScreenTypeInfo[]> { ... }
export async function getDefaultScreen(typeName: string): Promise<ScreenConfig> { ... }
```

### 3.2 Screen Browser Page

**New file**: `analytics-web-app/src/routes/ScreensPage.tsx`
- Grid of all screens grouped by type
- Click to open, "Create New" per type
- Use existing UI components (Card, Button, etc.)

**Add route** in `router.tsx`:
```typescript
const ScreensPage = lazy(() => import('@/routes/ScreensPage'))
// ...
<Route path="/screens" element={<ScreensPage />} />
```

### 3.3 Updated Sidebar

**Modify**: `analytics-web-app/src/components/layout/Sidebar.tsx`
- Fetch screens from API with `useState` + `useEffect` (no caching needed for <100 screens)
- Show system screens + link to /screens browser
- Dynamic icons based on screen_type

### 3.4 Files to Modify/Create

| File | Action |
|------|--------|
| `analytics-web-app/src/lib/screens-api.ts` | Create |
| `analytics-web-app/src/routes/ScreensPage.tsx` | Create |
| `analytics-web-app/src/router.tsx` | Add /screens route |
| `analytics-web-app/src/components/layout/Sidebar.tsx` | Modify |

---

## Phase 4: Frontend - Screen Viewer/Editor

### 4.1 Dynamic Screen Viewer

**New file**: `analytics-web-app/src/routes/ScreenPage.tsx`

Uses React Router's `useParams()`:
```typescript
import { useParams } from 'react-router-dom'

export default function ScreenPage() {
  const { name } = useParams<{ name: string }>()
  // Load screen config from API
  // Render based on screen_type
}
```

**Add routes** in `router.tsx`:
```typescript
const ScreenPage = lazy(() => import('@/routes/ScreenPage'))
// ...
<Route path="/screen/new" element={<ScreenPage />} />
<Route path="/screen/:name" element={<ScreenPage />} />
```

Render based on `screen_type`:
- `process_list`: Reuse QueryEditor + table pattern from ProcessesPage
- `metrics`: Reuse TimeSeriesChart pattern from ProcessMetricsPage
- `log`: Reuse log viewer pattern from ProcessLogPage

Features:
- "Edit SQL" button opens editor
- "Save As" creates copy with new name

### 4.2 Screen Renderer Component

**New file**: `analytics-web-app/src/components/ScreenRenderer.tsx`
- Props: `screen`, `onSaveAs`
- Switches on screen_type
- Handles variable substitution in SQL

### 4.3 Save As Dialog

**New file**: `analytics-web-app/src/components/SaveScreenDialog.tsx`
- Modal with name, description inputs
- Validates name uniqueness
- Calls createScreen API

### 4.4 Files to Create

| File | Action |
|------|--------|
| `analytics-web-app/src/routes/ScreenPage.tsx` | Create |
| `analytics-web-app/src/router.tsx` | Add /screen routes |
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
- Changes update URL params via `useSearchParams()`

### 5.3 Backend Variable Substitution

**Modify**: `rust/analytics-web-srv/src/stream_query.rs` (query handler)
- Accept variables in query request
- Substitute `$var` patterns before executing SQL
- Use parameterized queries where possible for security

### 5.4 Files to Modify/Create

| File | Action |
|------|--------|
| `analytics-web-app/src/components/VariableBar.tsx` | Create |
| `analytics-web-app/src/routes/ScreenPage.tsx` | Add variable handling |
| `rust/analytics-web-srv/src/stream_query.rs` | Enhance query substitution |

---

## Phase 6: Migration (Optional)

### 6.1 Gradual Migration

Keep existing pages working alongside new screen system:
- `routes/ProcessesPage.tsx` - keep as-is (basis for `process_list` screens)
- `routes/ProcessLogPage.tsx` - keep as-is (basis for `log` screens)
- `routes/ProcessMetricsPage.tsx` - keep as-is (basis for `metrics` screens)

Add new screen routes without removing old ones. Migration can happen later once the screen system is proven.

### 6.2 UI Polish

- Loading states for screen list
- Error handling for failed screen loads
- Confirmation dialog for delete
- Sorting screens by name/date
- Empty state when no screens exist

---

## Testing

### Backend Tests

Create `rust/analytics-web-srv/tests/screens_test.rs`:
- Test CRUD operations
- Test migration runs correctly
- Test invalid inputs return proper errors

### Frontend Tests

Create `analytics-web-app/src/lib/__tests__/screens-api.test.ts`:
- Test API client functions
- Mock responses

---

## Key Files Reference

**Backend patterns**:
- `rust/ingestion/src/sql_migration.rs` - Migration pattern to follow
- `rust/analytics-web-srv/src/main.rs` - Route registration pattern
- `rust/analytics-web-srv/src/auth.rs` - Auth middleware
- `rust/analytics-web-srv/src/stream_query.rs` - Query handling

**Frontend patterns**:
- `analytics-web-app/src/routes/ProcessesPage.tsx` - QueryEditor integration
- `analytics-web-app/src/components/layout/Sidebar.tsx` - Navigation
- `analytics-web-app/src/lib/api.ts` - API client pattern with `authenticatedFetch()`
- `analytics-web-app/src/router.tsx` - Route definitions

**Dev setup**:
- `local_test_env/ai_scripts/start_services.py` - Service startup (already creates micromegas_app DB)
- `local_test_env/db/utils.py` - Database utilities
