# Configurable Data Sources for Analytics Web App

**Status: IMPLEMENTED** — All phases complete. Verified with `cargo test`, `cargo clippy`, `cargo fmt`, `yarn build`, `yarn lint`, `yarn test`.

## Context

The analytics web app connects to a single FlightSQL server configured via the `MICROMEGAS_FLIGHTSQL_URL` environment variable, read inside `BearerFlightSQLClientFactory::make_client()` on every query. The goal is to make data sources configurable and stored in the database, with admin-only management and per-query selection. The env var is removed from analytics-web-srv entirely; `http_gateway` is a separate binary and keeps its own env var usage unchanged.

## Approach

Add a `data_sources` table to the app database with CRUD endpoints (admin-only for writes, name-only listing for regular users). Modify the query path so each request specifies a data source name, which is resolved via an in-memory cache to get the FlightSQL URL. The user's JWT is forwarded to whichever FlightSQL server is configured (all share the same OIDC trust). A "default" data source is used as a fallback when screens or built-in pages don't specify one.

### Data source cache

Data source URLs must not be sent by the frontend — doing so would allow any authenticated user to target arbitrary gRPC endpoints, and a malicious screen config (e.g., via import) could exfiltrate other users' JWTs to an attacker-controlled server.

Instead, the backend maintains an in-memory cache (`moka::future::Cache<String, DataSourceConfig>`) mapping data source names to configs:

- **Lazy loading**: entries are loaded from PG on first access via `try_get_with()`
- **TTL expiry**: entries expire after a configurable duration (e.g., 60 seconds), ensuring updates from other processes are picked up
- **CRUD on this process**: invalidate the cache entry so the next resolve fetches fresh from PG
- **Cache hit**: no PG round-trip
- **Cache miss / expired**: single-row PG query to load that entry

This avoids a PG query per analytics request. In multi-process deployments, URL changes on other processes are picked up within the TTL window.

### Deployment note

After upgrading, an admin must create at least one data source (typically named "default") via the admin UI before queries will work.

---

## Phase 1: Backend — Data Sources CRUD ✓

### 1.1 Database migration v1→v2

**File:** `rust/analytics-web-srv/src/app_db/schema.rs`
- Add `create_data_sources_table()`:
  ```sql
  CREATE TABLE data_sources(
      name VARCHAR(255) PRIMARY KEY,
      config JSONB NOT NULL,
      is_default BOOLEAN NOT NULL DEFAULT FALSE,
      created_by VARCHAR(255),
      updated_by VARCHAR(255),
      created_at TIMESTAMPTZ DEFAULT NOW(),
      updated_at TIMESTAMPTZ DEFAULT NOW()
  );
  CREATE UNIQUE INDEX data_sources_one_default
      ON data_sources (is_default) WHERE is_default = TRUE;
  ```
  Initial `config` schema: `{ "url": "https://..." }`. Validated at the application layer — `url` is required, must be http/https.
  The partial unique index ensures at most one data source can be marked as default.

**File:** `rust/analytics-web-srv/src/app_db/migration.rs`
- Bump `LATEST_APP_SCHEMA_VERSION` to 2
- Add migration step: if current version is 1, create data_sources table and update version to 2
- Fresh installs run both steps sequentially (0→1→2) in a single migration call

### 1.2 Data source model

**File:** `rust/analytics-web-srv/src/app_db/models.rs`
- Add `DataSource` struct (name, config: `serde_json::Value`, is_default, created_by, updated_by, created_at, updated_at)
- Add `DataSourceConfig` struct with `url: String` — deserialized from the JSONB `config` column, extensible for future fields
- Add `CreateDataSourceRequest` (name, config, is_default) and `UpdateDataSourceRequest` (config, is_default)
- Rename `validate_screen_name` → `validate_name` and `normalize_screen_name` → `normalize_name`, reuse for data source name validation
- Validate `config`: deserialize as `DataSourceConfig`, `url` must be present and parse as http/https URI

### 1.3 Data source cache

**New file:** `rust/analytics-web-srv/src/data_source_cache.rs`
- `DataSourceCache` wrapping `moka::future::Cache<String, DataSourceConfig>` (name → config)
- Configure with TTL (e.g., 60 seconds) so updates from other processes are picked up
- `resolve(&self, name: &str, pool: &PgPool) -> Result<Option<DataSourceConfig>>` — returns cached config or loads from PG on miss/expiry via `try_get_with()`
- `invalidate(&self, name: &str)` — evicts entry after create/update/delete so next resolve fetches fresh from PG
- No bulk load at startup — entries are loaded lazily on first access
- No special default resolution — the frontend resolves the default data source name from `listDataSources()` and always sends a concrete name

### 1.4 Data sources CRUD endpoints

**New file:** `rust/analytics-web-srv/src/data_sources.rs`
- `GET /api/data-sources` — list names and default flag (any authenticated user); returns `Vec<{ name, is_default }>`
- `POST /api/data-sources` — create (admin only via `ValidatedUser.is_admin`); if this is the first data source, auto-set `is_default: true`; if `is_default: true` is requested, clear default on other rows in a transaction; invalidates cache
- `GET /api/data-sources/{name}` — get full details including URL (admin only)
- `PUT /api/data-sources/{name}` — update config and/or transfer default (admin only); if `is_default: true`, clear default on other rows in a transaction; reject if request would remove the default flag from the current default (400: "cannot remove default flag — set another data source as default instead"); invalidates cache
- `DELETE /api/data-sources/{name}` — delete (admin only); wrap check + delete in a transaction to prevent TOCTOU race (a concurrent PUT could move the default away between the check and delete); reject if `is_default = true` (400: "cannot delete the default data source — set another data source as default first"); invalidates cache; no cascade check — screens referencing a deleted data source will fail at query time

**Default protection invariant:** once a default data source exists, there is always exactly one. The default flag can only move to another source (via PUT or POST with `is_default: true`), never be removed or left empty.

### 1.5 Expose `is_admin` in `/auth/me`

**File:** `rust/analytics-web-srv/src/auth.rs`
- Add `is_admin: bool` to `UserInfo` struct
- Populate from `ValidatedUser.is_admin` in `auth_me()` handler

**File:** `rust/analytics-web-srv/src/main.rs`
- Update no-auth stub `NoAuthUserInfo` to include `is_admin: true`

### 1.6 Register routes and cache

**File:** `rust/analytics-web-srv/src/main.rs`
- Add `mod data_sources;` and `mod data_source_cache;`
- Create `DataSourceCache` (moka cache with TTL, takes `PgPool` for lazy loading)
- Pass `Extension(DataSourceCache)` to both data source routes and query routes
- Register data source routes with auth middleware

---

## Phase 2: Backend — Per-Query Data Source Lookup ✓

### 2.1 Modify `BearerFlightSQLClientFactory`

**File:** `rust/public/src/client/flightsql_client_factory.rs`
- Add `url: String` field
- `new()` and `new_with_client_type()` take `url` as first param
- `make_client()` uses `self.url` instead of reading env var
- Remove `MICROMEGAS_FLIGHTSQL_URL` env var read from this file

Note: `http_gateway.rs` has its own query path and reads the env var directly — not touched here.

### 2.2 Modify query handler

**File:** `rust/analytics-web-srv/src/stream_query.rs`
- Add `data_source: String` to `StreamQueryRequest`
- Add `DataSourceNotFound` variant to `ErrorCode`
- Handler takes `Extension(DataSourceCache)`, resolves data source name to config:
  1. Call `cache.resolve(&request.data_source, &pool)`
  2. If `None`: return `DataSourceNotFound` error
- Pass `config.url` to `BearerFlightSQLClientFactory::new_with_client_type(url, token, "web")`

---

## Phase 3: Frontend — Data Sources API + Admin UI ✓

### 3.1 Update User type

**File:** `analytics-web-app/src/lib/auth.tsx`
- Add `is_admin?: boolean` to `User` interface

### 3.2 Data sources API

**New file:** `analytics-web-app/src/lib/data-sources-api.ts`
- `listDataSources()` — returns `{ name: string, isDefault: boolean }[]` (available to all authenticated users)
- `getDataSource()`, `createDataSource()`, `updateDataSource()`, `deleteDataSource()` — admin only
- Follow patterns from `screens-api.ts`

### 3.3 Data sources admin page

**New file:** `analytics-web-app/src/routes/DataSourcesPage.tsx`
- Table listing data sources (name, URL from config, created_by, updated_at) — fetches full details via admin endpoint
- Create/edit/delete actions (admin only)
- Form for name + URL (initially; extensible as `DataSourceConfig` grows)

**File:** `analytics-web-app/src/routes/AdminPage.tsx`
- Add card linking to `/admin/data-sources`

**File:** `analytics-web-app/src/router.tsx`
- Add route for DataSourcesPage

---

## Phase 4: Frontend — Per-Query Data Source Selection ✓

### 4.1 Update query params

**File:** `analytics-web-app/src/lib/arrow-stream.ts`
- Add `dataSource: string` to `StreamQueryParams`
- Send `data_source` in POST body
- Add `DATA_SOURCE_NOT_FOUND` to `ErrorCode`

### 4.2 Thread data source through hooks

- `useScreenQuery` (`lib/screen-renderers/useScreenQuery.ts`) — add `dataSource` param, pass through
- `useSqlHandlers` — pass through
- `useCellExecution` (notebooks) — add `dataSource` param
- `useStreamQuery` execute calls already take `StreamQueryParams` — no structural change needed

### 4.3 Data source selector component

**New file:** `analytics-web-app/src/components/DataSourceSelector.tsx`
- Dropdown fetching from `listDataSources()`
- Shows all named data sources; the one marked `isDefault` is pre-selected when no data source is specified in config
- Used in screen config panels and notebook editors

### 4.4 Update screen renderers

Each renderer reads `config.dataSource` and passes to query calls. When `config.dataSource` is missing or empty, the frontend resolves the default from `listDataSources()` (the entry with `isDefault: true`) and sends that concrete name:
- `LogRenderer.tsx`, `TableRenderer.tsx`, `MetricsRenderer.tsx`, `NotebookRenderer.tsx`, `ProcessListRenderer.tsx`

This handles backward compatibility with existing screens that have no `dataSource` in their config.

### 4.5 Default data source hook

**New file:** `analytics-web-app/src/hooks/useDefaultDataSource.ts`
- Returns the name of the entry with `isDefault: true` from the data source list already fetched for macro substitution — no extra API call
- Used by screen renderers (as fallback when `config.dataSource` is unset) and built-in pages
- Avoids duplicating default-resolution logic across components

### 4.6 Built-in pages

Built-in pages (ProcessesPage, ProcessLogPage, PerformanceAnalysisPage) use the same cached data source list to resolve the default name and send it with each query.

---

## Phase 5: Cleanup ✓

- Remove `MICROMEGAS_FLIGHTSQL_URL` from analytics-web-srv startup scripts and documentation
- Keep `MICROMEGAS_FLIGHTSQL_URL` in `http_gateway` documentation (separate binary, unchanged)
- Update `start_analytics_web.py` to remove the env var from the analytics-web-srv environment

---

## Security

- **SSRF / credential exfiltration**: URLs are never sent by the frontend. The backend resolves data source names via an in-memory cache backed by PG. Only admins can create or modify data sources. This prevents malicious screen configs from directing other users' JWTs to attacker-controlled servers.
- **Information disclosure**: Regular users can only list data source names. URLs and other details are only visible to admins.
- **Config validation**: `config` JSONB is deserialized and validated at the application layer; `url` must be http/https (as required by tonic for gRPC channels).
- **Auth forwarding**: User's JWT forwarded to configured FlightSQL server. All servers must share OIDC trust.

## Verification ✓

1. `cargo test` — all tests pass ✓
2. `cargo clippy --workspace -- -D warnings` — clean ✓
3. `cargo fmt` — clean ✓
4. `yarn build` — clean ✓
5. `yarn lint` (eslint) — clean ✓
6. `yarn test` — 644 tests pass ✓
7. Manual: start services, create a data source named "default" via admin UI, verify existing screens and built-in pages work, create a new screen pointing to a different data source, run a query
