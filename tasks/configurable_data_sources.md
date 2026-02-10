# Configurable Data Sources for Analytics Web App

## Context

The analytics web app connects to a single FlightSQL server configured via the `MICROMEGAS_FLIGHTSQL_URL` environment variable, read inside `BearerFlightSQLClientFactory::make_client()` on every query. The goal is to make data sources configurable and stored in the database, with admin-only management and per-query selection. The env var is removed from analytics-web-srv entirely; `http_gateway` is a separate binary and keeps its own env var usage unchanged.

## Approach

Add a `data_sources` table to the app database with CRUD endpoints (admin-only for writes, name-only listing for regular users). Modify the query path so each request specifies a data source name, which is resolved via an in-memory cache to get the FlightSQL URL. The user's JWT is forwarded to whichever FlightSQL server is configured (all share the same OIDC trust). A "default" data source is used as a fallback when screens or built-in pages don't specify one.

### Data source cache

Data source URLs must not be sent by the frontend — doing so would allow any authenticated user to target arbitrary gRPC endpoints, and a malicious screen config (e.g., via import) could exfiltrate other users' JWTs to an attacker-controlled server.

Instead, the backend maintains an in-memory cache (`Arc<RwLock<HashMap<String, String>>>`) mapping data source names to URLs:

- **Startup**: load all data sources from PG
- **CRUD on this process**: update local cache immediately after the DB write
- **Query cache hit**: no PG round-trip
- **Query cache miss**: refresh entire cache from PG, retry lookup, return `DataSourceNotFound` if still missing

This avoids a PG query per analytics request. In multi-process deployments, the only staleness case is a deleted data source remaining usable on other processes until their next cache miss triggers a refresh — this is harmless.

### Deployment note

After upgrading, an admin must create at least one data source (typically named "default") via the admin UI before queries will work.

---

## Phase 1: Backend — Data Sources CRUD

### 1.1 Database migration v1→v2

**File:** `rust/analytics-web-srv/src/app_db/schema.rs`
- Add `create_data_sources_table()`:
  ```sql
  CREATE TABLE data_sources(
      name VARCHAR(255) PRIMARY KEY,
      url VARCHAR(2048) NOT NULL,
      created_by VARCHAR(255),
      updated_by VARCHAR(255),
      created_at TIMESTAMPTZ DEFAULT NOW(),
      updated_at TIMESTAMPTZ DEFAULT NOW()
  );
  ```

**File:** `rust/analytics-web-srv/src/app_db/migration.rs`
- Bump `LATEST_APP_SCHEMA_VERSION` to 2
- Add migration step: if current version is 1, create data_sources table and update version to 2
- Fresh installs run both steps sequentially (0→1→2) in a single migration call

### 1.2 Data source model

**File:** `rust/analytics-web-srv/src/app_db/models.rs`
- Add `DataSource` struct (name, url, created_by, updated_by, created_at, updated_at)
- Add `CreateDataSourceRequest` (name, url) and `UpdateDataSourceRequest` (url)
- Reuse existing `validate_screen_name` / `normalize_screen_name` for data source name validation
- Add URL validation: must parse as URI, scheme must be `http` or `https` (tonic expects these schemes for gRPC channels)

### 1.3 Data source cache

**New file:** `rust/analytics-web-srv/src/data_source_cache.rs`
- `DataSourceCache` wrapping `Arc<RwLock<HashMap<String, String>>>`
- `load_all(pool: &PgPool) -> Result<DataSourceCache>` — reads all rows, builds map
- `resolve(&self, name: &str) -> Option<String>` — returns URL for a name
- `refresh(&self, pool: &PgPool) -> Result<()>` — reloads entire map from PG
- `insert(&self, name: String, url: String)` — updates cache after create/update
- `remove(&self, name: &str)` — updates cache after delete

### 1.4 Data sources CRUD endpoints

**New file:** `rust/analytics-web-srv/src/data_sources.rs`
- `GET /api/data-sources` — list names only (any authenticated user); returns `Vec<String>`
- `POST /api/data-sources` — create (admin only via `ValidatedUser.is_admin`); updates cache
- `GET /api/data-sources/{name}` — get full details including URL (admin only)
- `PUT /api/data-sources/{name}` — update URL (admin only); updates cache
- `DELETE /api/data-sources/{name}` — delete (admin only); updates cache; no cascade check — screens referencing a deleted data source will fail at query time

### 1.5 Expose `is_admin` in `/auth/me`

**File:** `rust/analytics-web-srv/src/auth.rs`
- Add `is_admin: bool` to `UserInfo` struct
- Populate from `ValidatedUser.is_admin` in `auth_me()` handler

**File:** `rust/analytics-web-srv/src/main.rs`
- Update no-auth stub `NoAuthUserInfo` to include `is_admin: true`

### 1.6 Register routes and cache

**File:** `rust/analytics-web-srv/src/main.rs`
- Add `mod data_sources;` and `mod data_source_cache;`
- Load `DataSourceCache` from PG at startup
- Pass `Extension(DataSourceCache)` to both data source routes and query routes
- Register data source routes with auth middleware

---

## Phase 2: Backend — Per-Query Data Source Lookup

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
- Handler takes `Extension(DataSourceCache)`, resolves data source name to URL:
  1. Try `cache.resolve(&name)`
  2. On miss: `cache.refresh(&pool)` then retry `cache.resolve(&name)`
  3. If still missing: return `DataSourceNotFound` error
- Handler also takes `Extension(PgPool)` (needed only for cache refresh on miss)
- Pass URL to `BearerFlightSQLClientFactory::new_with_client_type(url, token, "web")`

---

## Phase 3: Frontend — Data Sources API + Admin UI

### 3.1 Update User type

**File:** `analytics-web-app/src/lib/auth.tsx`
- Add `is_admin?: boolean` to `User` interface

### 3.2 Data sources API

**New file:** `analytics-web-app/src/lib/data-sources-api.ts`
- `listDataSourceNames()` — returns `string[]` (available to all authenticated users)
- `getDataSource()`, `createDataSource()`, `updateDataSource()`, `deleteDataSource()` — admin only
- Follow patterns from `screens-api.ts`

### 3.3 Data sources admin page

**New file:** `analytics-web-app/src/routes/DataSourcesPage.tsx`
- Table listing data sources (name, URL, created_by, updated_at) — fetches full details via admin endpoint
- Create/edit/delete actions (admin only)
- Simple form for name + URL

**File:** `analytics-web-app/src/routes/AdminPage.tsx`
- Add card linking to `/admin/data-sources`

**File:** `analytics-web-app/src/router.tsx`
- Add route for DataSourcesPage

---

## Phase 4: Frontend — Per-Query Data Source Selection

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
- Dropdown fetching from `listDataSourceNames()`
- Used in screen config panels and notebook editors
- Defaults to `"default"` when no data source is specified in config

### 4.4 Update screen renderers

Each renderer reads `config.dataSource` and passes to query calls. When `config.dataSource` is missing or empty, the frontend substitutes `"default"`:
- `LogRenderer.tsx`, `TableRenderer.tsx`, `MetricsRenderer.tsx`, `NotebookRenderer.tsx`, `ProcessListRenderer.tsx`

This handles backward compatibility with existing screens that have no `dataSource` in their config.

### 4.5 Update default screen configs

**File:** `rust/analytics-web-srv/src/screen_types.rs`
- Add `"dataSource": "default"` to each screen type's default config

### 4.6 Built-in pages

Built-in pages (ProcessesPage, ProcessLogPage, PerformanceAnalysisPage) pass `"default"` as the data source name in their query requests, same as any other caller using the fallback.

---

## Phase 5: Cleanup

- Remove `MICROMEGAS_FLIGHTSQL_URL` from analytics-web-srv startup scripts and documentation
- Keep `MICROMEGAS_FLIGHTSQL_URL` in `http_gateway` documentation (separate binary, unchanged)
- Update `start_analytics_web.py` to remove the env var from the analytics-web-srv environment

---

## Security

- **SSRF / credential exfiltration**: URLs are never sent by the frontend. The backend resolves data source names via an in-memory cache backed by PG. Only admins can create or modify data sources. This prevents malicious screen configs from directing other users' JWTs to attacker-controlled servers.
- **Information disclosure**: Regular users can only list data source names. URLs and other details are only visible to admins.
- **URL validation**: Scheme restricted to http/https (as required by tonic for gRPC channels).
- **Auth forwarding**: User's JWT forwarded to configured FlightSQL server. All servers must share OIDC trust.

## Verification

1. `cargo test` from `rust/` — existing + new tests pass
2. `cargo clippy --workspace -- -D warnings`
3. `yarn build && yarn lint` from `analytics-web-app/`
4. Manual: start services, create a data source named "default" via admin UI, verify existing screens and built-in pages work, create a new screen pointing to a different data source, run a query
