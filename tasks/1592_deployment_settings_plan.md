# DB-Backed Deployment Settings Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1592

## Overview

Several deployment-wide toggles are env vars read once at startup, so changing one means editing
the env and restarting every service that reads it. Nothing records who changed what, and #1561
showed that a knob can be ignored on one code path while it looks set everywhere else. This plan
adds a `deployment_settings` table in the telemetry DB. Each service caches it with the same
whole-table snapshot cache the audience-grant and group stores already use. Admins edit it from a
new **Admin → Deployment Settings** page, and every change goes to the control-plane audit log.
The first settings to move are `MICROMEGAS_PUBLIC_VIEW_SETS` and the four
`MICROMEGAS_SELF_SERVICE_*` knobs. Each env var stays as the fallback used when no row exists, so
existing deployments behave the same on upgrade. Later settings, such as the per-tag retention
policy in #1695, become one registry entry each, with no new migration.

## Current State

**The knobs being moved**

- `MICROMEGAS_PUBLIC_VIEW_SETS`: parsed by `IsolationConfig::from_env`
  (`rust/analytics/src/lakehouse/read_scope.rs:149-158`). A malformed value fails startup. It is
  resolved once in `FlightSqlServer::build_and_serve` (`rust/public/src/servers/flight_sql_server.rs:235`,
  via `resolve_isolation_config` at `:52`). `with_isolation_config(..)` (`:172`) overrides it.
  `FlightSqlServiceImpl` holds it as `isolation_config: Arc<IsolationConfig>`
  (`flight_sql_service_impl.rs:549`) and copies it into every `CallerContext` in `caller_context`
  (`:629`). From there it reaches `OwnershipRewrite` and `AudienceGuard`
  (`rust/analytics/src/lakehouse/query.rs:131,370`). Since `caller_context` builds the scope per
  request, reading a new value per request needs no change in `micromegas-analytics`.
- `MICROMEGAS_SELF_SERVICE_MINT` and `..._MAX_CLAIMS_PER_CALLER` / `..._MAX_KEYS_PER_CALLER` /
  `..._MAX_GRANTS_PER_CALLER`: read once in `run_web_server`
  (`rust/analytics-web-srv/src/web_server.rs:778-789`) through `resolve_bool_env` /
  `resolve_i64_env` (`:106-137`), then copied as plain fields into `IngestionKeysState`
  (`ingestion_keys.rs:72-81`) and `AudienceGrantsState` (`audience_grants.rs:63,73`). They are
  read at `ingestion_keys.rs:342,448,693` and `audience_grants.rs:323,547,808,944`.

**Precedents being reused**

- `rust/auth/src/db_snapshot.rs`: `SnapshotSource<L: SnapshotLoader>` is a generic whole-table
  snapshot cache. It refreshes lazily, at most once per TTL. After the first successful load it
  keeps serving the last good snapshot. When it has never loaded, it fails with
  `ProviderUnavailable`, and cold-start retries are throttled. `DbAudienceGrantsSource`
  (`db_audience_grants.rs`) and `DbGroupsSource` (`groups.rs:237`) are the two existing loaders.
  Both use `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` (default 60).
- `audience_grants` / `groups` tables: data-lake schema migrations in
  `rust/ingestion/src/sql_migration.rs` (currently `LATEST_DATA_LAKE_SCHEMA_VERSION = 11`). These
  tables live in the same DB, behind the same `MICROMEGAS_SQL_CONNECTION_STRING`, that both
  `flight-sql-srv` and `analytics-web-srv` already read.
- `query_deny_list` (`rust/analytics/src/lakehouse/query_deny_list.rs`, lakehouse migration v9):
  refreshed by a background timer. It is the closer match for "an incident valve that needs ~10 s
  propagation". Settings don't need that latency, so they reuse `SnapshotSource` instead (see
  Trade-offs).
- Admin REST plus audit: `groups.rs` has admin-only routes behind `GroupAdminGate`, which wraps
  `AdminUser` and emits a `"denied"` `MutationAudit` record when it rejects a caller
  (`groups.rs:202-232`). `mutation_audit.rs` provides `MutationAuditRecord`, the action constants
  and `AuditOutcome`. On the frontend, `audience-grants-api.ts` and `groups-api.ts` call these
  routes with plain `authenticatedFetch`, and `AdminPage.tsx` lists the admin cards.

**Not moving: `MICROMEGAS_DEFAULT_AUDIENCE`.** The ingestion role stamps it onto new rows. The
read side uses it to `COALESCE` unstamped rows. The maintenance role writes it into materialized
partitions (`metadata_partition_spec.rs:37`, and `mkdocs/docs/admin/flight-sql.md:33`). Changing
it at runtime would leave already-materialized partitions disagreeing with new ones, and the
`AudienceIndex` cache (5-min TTL) would keep serving the old value. That makes it a data
migration, not a toggle, so it stays env-only.

## Design

### Which env vars move

| Class | Vars | Why |
|---|---|---|
| **Move now** | `MICROMEGAS_PUBLIC_VIEW_SETS`, `MICROMEGAS_SELF_SERVICE_MINT`, `MICROMEGAS_SELF_SERVICE_MAX_{CLAIMS,KEYS,GRANTS}_PER_CALLER` | Deployment-wide policy, already read on a per-request path. They need the same value on every replica. |
| **Good later candidates** (separate issues) | `MICROMEGAS_RETENTION_DAYS` → #1695's per-tag retention policy; `MICROMEGAS_QUERY_DENY_MAX_RULES` | Deployment-wide policy. Each is one registry entry plus a call-site change once this store exists. |
| **Stay env-only** | `MICROMEGAS_DEFAULT_AUDIENCE` (see above); connection strings, object-store URIs, listen addresses, OIDC config and secrets, `MICROMEGAS_API_KEYS`; memory budgets, cache sizes, temp-dir caps, telemetry-sink knobs; refresh intervals and TTLs, including `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, which governs this cache | Needed before the DB is reachable, secret, per-process or per-replica tuning, or baked into stored data. |

### Table: data-lake migration v12 (`rust/ingestion/src/sql_migration.rs`)

```sql
CREATE TABLE deployment_settings (
  key        VARCHAR(255) PRIMARY KEY,
  value      JSONB        NOT NULL,
  updated_at TIMESTAMPTZ  NOT NULL,
  updated_by VARCHAR(255) NOT NULL
);
```

This is one generic key/value table, not one table per feature. A key with no row means "unset":
the consuming service falls back to the env var, then to the built-in default. Removing a value is
a `DELETE`. The table keeps only the latest change; the full history lives in the audit log.

### Registry and typed snapshot: new module `rust/public/src/deployment_settings.rs` (`micromegas::deployment_settings`)

The module lives in `micromegas` (`rust/public`) because every consumer already depends on it
(flight-sql, analytics-web-srv, monolith, and later maintenance), and `micromegas` can use both
`micromegas_auth::db_snapshot` and `micromegas_analytics::…::IsolationConfig`.

```rust
pub enum SettingKind {
    Bool,
    Count,       // JSON integer in 0..=i64::MAX
    NameList,    // JSON array of non-empty, trimmed, unique strings
}

pub struct SettingSpec {
    pub key: &'static str,         // "public_view_sets"
    pub env_var: &'static str,     // "MICROMEGAS_PUBLIC_VIEW_SETS"
    pub kind: SettingKind,
    pub default: &'static str,     // JSON text, shown in the UI ("false", "25", "[]")
    pub description: &'static str,
}

pub const SETTINGS: &[SettingSpec] = &[ /* the five entries */ ];

impl SettingSpec {
    pub fn validate(&self, value: &serde_json::Value) -> anyhow::Result<()>;
}
```

`SETTINGS` serves two purposes. It is the list the admin page renders, and its `validate` is the
single validation rule used by both the PUT handler and the loader.

The cached snapshot holds the DB values already parsed into typed fields:

```rust
pub struct SelfServiceSettings { pub mint_enabled: bool, pub max_claims_per_caller: i64,
                                 pub max_keys_per_caller: i64, pub max_grants_per_caller: i64 }

#[derive(Default)]
pub struct DbOverrides {                   // SnapshotLoader::Snapshot; pub for the test seam
    isolation: Option<Arc<IsolationConfig>>,
    self_service_mint: Option<bool>,
    max_claims_per_caller: Option<i64>,
    max_keys_per_caller: Option<i64>,
    max_grants_per_caller: Option<i64>,
}

struct EnvFallbacks {                      // resolved once per process, at startup
    isolation: Arc<IsolationConfig>,
    self_service: SelfServiceSettings,
}

pub struct DeploymentSettingsSource {
    db: Backing,   // enum { Db(SnapshotSource<DeploymentSettingsLoader>), Fixed(Arc<DbOverrides>) }
                   // Fixed(empty) for env_only/pinned; Fixed(x) for with_overrides
    env: EnvFallbacks,
}

impl DeploymentSettingsSource {
    pub fn new(pool: PgPool, ttl: Duration) -> Result<Self>;   // runs EnvFallbacks::from_env()?
    pub fn env_only() -> Result<Self>;                         // no telemetry-DB pool configured
    pub fn pinned(isolation: Arc<IsolationConfig>) -> Self;    // with_isolation_config / tests
    pub fn with_overrides(db: DbOverrides) -> Result<Self>;    // no-DB tests (tests/ can't see cfg(test))
    pub async fn isolation_config(&self) -> Result<Arc<IsolationConfig>>;
    pub async fn self_service(&self) -> Result<SelfServiceSettings>;
    pub async fn warn_on_shadowed_env(&self);                  // best-effort startup diagnostic
}
```

- **Resolution order, per key:** DB row, then env var, then built-in default. The env-and-default
  half is exactly today's parsing. `IsolationConfig::from_env` stays as is and keeps failing fast
  on a malformed value. The `resolve_bool_env` / `resolve_i64_env` logic moves from
  `web_server.rs` into `EnvFallbacks::from_env` unchanged, so counts still warn and fall back to
  the default.
- **`DeploymentSettingsLoader::fetch`** runs `SELECT key, value FROM deployment_settings`. It
  ignores keys missing from `SETTINGS`, because during a rolling upgrade a newer replica may have
  written keys an older one doesn't know. If a known key fails `validate`, the whole load fails.
  `SnapshotSource` then keeps serving the last good snapshot, or returns `ProviderUnavailable` on a
  cold start. This matches how `AudienceGrantsLoader` handles malformed rows. The refresh-error
  metric is `deployment_settings_refresh_error_count`.
- **TTL:** reuses `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` (default 60). Both settings being moved are
  authorization knobs, and that variable already governs the key, grant and group caches across
  the process. No new knob.
- **Accessor cost per request:** one `SnapshotSource::current()` (an RwLock read on the fast path)
  plus an `Option` check and an `Arc` clone.
- **`warn_on_shadowed_env`:** called once at startup by each server, best effort, in the same way
  `web_server.rs:811-839` checks for a missing mint grant. For every key whose env var is set
  *and* has a DB row, it logs `"{env_var} is set but ignored: deployment_settings.{key} is
  stored"`. This targets the #1561 failure mode: an env var that looks set but has no effect.

### FlightSQL wiring (`rust/public/src/servers/`)

- `FlightSqlServiceImpl`: the field `isolation_config: Arc<IsolationConfig>` becomes
  `settings: Arc<DeploymentSettingsSource>`. In `caller_context`,
  `isolation_config: self.settings.isolation_config().await.map_err(...)` maps a
  `ProviderUnavailable` error to `Status::unavailable` and any other error to `Status::internal`.
  This is the same downcast the `read_policy` arm uses just above.
- `FlightSqlServer::build_and_serve`: `resolve_isolation_config` becomes `resolve_settings`. It
  returns `DeploymentSettingsSource::pinned(c)` when `with_isolation_config(c)` was called.
  Otherwise it returns `DeploymentSettingsSource::new(pool, ttl)` over a
  `dedicated_key_store_pool` built from the lake connection, the same construction the grant store
  uses, on **every** auth branch, so #1561 cannot come back. It still runs before any
  Postgres/object-store setup, because `new` resolves `EnvFallbacks` first and fails fast on a
  malformed `MICROMEGAS_PUBLIC_VIEW_SETS`. The lazy pool makes no connection until the first
  query.
- Monolith: no change needed. It does not call `with_isolation_config`, so the builder's own path
  applies.

### Web server wiring (`rust/analytics-web-srv/src/`)

- `IngestionKeysState` and `AudienceGrantsState` drop their four knob fields and gain
  `settings: Arc<DeploymentSettingsSource>`. `run_web_server` builds one instance and shares it
  between the two states and the new settings route. It uses `new` when `analytics_keys_pool` is
  `Some`, and `env_only()` otherwise.
- Every read site listed under Current State calls `state.settings.self_service().await` once at
  the top of the handler or gate, and maps an error to the module's existing 503 variant (the same
  response a missing pool returns). The startup `info!` at `:784-789` is replaced by
  `warn_on_shadowed_env`.

### Settings REST routes: new `rust/analytics-web-srv/src/deployment_settings.rs`

The routes are admin-only, registered next to `/api/groups` in `build_protected_routes`, and
follow `groups.rs`'s structure:

| Route | Behavior |
|---|---|
| `GET {base}/api/deployment-settings` | Returns one entry per `SETTINGS` item: `{key, kind, env_var, default, description, value \| null, updated_at \| null, updated_by \| null}`. Reads the table directly, not through the cache, so the page shows what was just written. |
| `PUT {base}/api/deployment-settings/{key}` with body `{value}` | Rejects an unknown key with 404 and a failed `validate` with 400. Otherwise, in one transaction: `SELECT value … FOR UPDATE` (the previous value), then `INSERT … ON CONFLICT (key) DO UPDATE`. Returns the new entry. |
| `DELETE {base}/api/deployment-settings/{key}` | Deletes the row, so the key reverts to env or default. Returns 404 when no row existed. |

- **Gate:** `SettingsAdminGate`, which copies `GroupAdminGate`: it delegates to `AdminUser` and
  audits a rejection as `"denied"`.
- **Audit:** new `action::SET_SETTING` / `action::RESET_SETTING`. `MutationAuditRecord` gets three
  optional fields, appended last: `setting`, `value`, and `previous_value`. The two values are
  JSON text, truncated by `truncate_for_audit`. Each handler follows the existing wrapper +
  `_inner` + `audit.emit(&result)` shape, and a new `DeploymentSettingsError` implements
  `AuditOutcome`.
- **Propagation:** the replica that handled the write sees the new value within one TTL. There is
  no local invalidation, which matches how grants behave today. The page says "within one cache
  refresh" rather than "immediately".

### Frontend (`analytics-web-app/src/`)

- `lib/deployment-settings-api.ts`: `fetchDeploymentSettings`, `setDeploymentSetting(key, value)`
  and `resetDeploymentSetting(key)`, with an error class, in the same `authenticatedFetch` shape as
  `groups-api.ts`.
- `routes/DeploymentSettingsPage.tsx`: `AuthGuard requireAdmin`, with breadcrumb and header in the
  same layout as `QueryDenyListPage`. Settings are grouped by key prefix into tables showing
  setting, value, source pill (Database or Env/default) and last change. Each row has an Edit
  dialog that fits its kind (toggle, number input, chip list) and, when a row is stored, a Reset
  button that opens a `ConfirmDialog`. After every write the page re-fetches the list instead of
  patching it locally.
- `AdminPage.tsx`: adds a "Deployment Settings" card with `adminOnly: true`. `router.tsx` adds the
  `/admin/settings` route.

## Mockups

- `tasks/1592_deployment_settings_mockups/settings-page.html`: the settings page with grouped
  tables, source pills and the list-editor dialog open. There is only one option: the page is a
  plain list in the style of the existing admin pages, so there was no layout choice worth
  mocking up separately.

## Implementation Steps

1. **Schema:** add `upgrade_data_lake_schema_v12` (the `deployment_settings` table) to
   `rust/ingestion/src/sql_migration.rs`, bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 12, and add
   the step to `execute_migration`.
2. **Store:** create `rust/public/src/deployment_settings.rs` with `SettingKind`, `SettingSpec`,
   `SETTINGS`, `validate`, `DeploymentSettingsLoader`, `EnvFallbacks` and
   `DeploymentSettingsSource`, and export it from `rust/public/src/lib.rs`. Move
   `resolve_bool_env` / `resolve_u64_env` / `resolve_i64_env` from `web_server.rs` into it.
3. **FlightSQL:** replace `isolation_config` with `settings` in `flight_sql_service_impl.rs`, and
   replace `resolve_isolation_config` with `resolve_settings` in `flight_sql_server.rs`. Update
   `isolation_config_fail_fast_tests.rs` and the in-file tests at `flight_sql_server.rs:500-600`.
4. **Web server state:** switch `IngestionKeysState` / `AudienceGrantsState` to `settings`, update
   every read site, and build the shared source in `run_web_server`. Update
   `ingestion_keys_tests.rs`, `audience_grants_tests.rs` and `routing_tests.rs` to construct the
   source with `with_overrides`.
5. **Settings routes:** add `deployment_settings.rs` (routes, `SettingsAdminGate`,
   `DeploymentSettingsError`), extend `mutation_audit.rs` (two actions and three fields), and
   register the routes in `web_server.rs`.
6. **Frontend:** add the API client, the page, the admin card and the route, plus their tests.
7. **Docs and changelog:** see Documentation.

## Files to Modify

- `rust/ingestion/src/sql_migration.rs`
- `rust/public/src/deployment_settings.rs` (new), `rust/public/src/lib.rs`
- `rust/public/src/servers/flight_sql_server.rs`, `rust/public/src/servers/flight_sql_service_impl.rs`
- `rust/public/tests/isolation_config_fail_fast_tests.rs`, `rust/public/tests/deployment_settings_tests.rs` (new)
- `rust/analytics-web-srv/src/web_server.rs`, `ingestion_keys.rs`, `audience_grants.rs`, `mutation_audit.rs`, `deployment_settings.rs` (new), `lib.rs`
- `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`, `audience_grants_tests.rs`, `routing_tests.rs`, `mutation_audit_tests.rs`, `deployment_settings_tests.rs` (new)
- `analytics-web-app/src/lib/deployment-settings-api.ts` (new), `src/routes/DeploymentSettingsPage.tsx` (new), `src/routes/AdminPage.tsx`, `src/router.tsx`, plus tests under `__tests__`
- `mkdocs/docs/admin/authorization.md`, `flight-sql.md`, `monolith.md`, `web-app.md`, `control-plane-audit-log.md`
- `CHANGELOG.md`

## Trade-offs

- **One generic table rather than one table per feature.** `audience_grants` and
  `query_deny_list` are collections of rows with their own natural keys. These settings are
  scalars or small lists, so a table per knob would mean a migration and a bespoke loader for
  every boolean. Typing is enforced in Rust (`SettingSpec::validate` on write and on load) rather
  than with SQL `CHECK`s, since JSONB can't express per-key schemas cleanly. If a future setting
  really is a collection with its own identity, it should still get its own table.
- **Env as the fallback, not as an override or a one-time seed.** If env overrode the DB, the
  admin UI would be unreliable: replicas whose env is set would ignore it. Seeding the DB from env
  at first startup gives one source of truth, but it adds a startup write path and a surprise for
  local dev, where editing an env var after the first boot would do nothing. With env as the
  fallback, upgrading requires zero action. The remaining risk, an operator setting an env var
  that a DB row already shadows, is reported by `warn_on_shadowed_env`.
- **Lazy TTL cache (`SnapshotSource`) rather than a `query_deny_list`-style background timer.**
  `SnapshotSource` already handles cold-start throttling and serving the last good snapshot, and
  both of its existing loaders sit on the same pool. A 60 s propagation bound is fine for policy
  toggles. The deny list needs ~10 s because it is an incident valve.
- **The store lives in `micromegas` (`rust/public`), not `micromegas-auth`.** Settings are not all
  auth-related (e.g. #1695's retention), and `rust/public` is the one crate that can see both
  `SnapshotSource` and `IsolationConfig`.
- **REST routes rather than SQL UDTFs.** The deny list page goes through FlightSQL because its
  rules are evaluated there. Settings serve several services, and the grant and group admin pages
  already set the REST-plus-audit precedent. This also adds nothing to the SQL surface, which is
  the stable interface.
- **Rejected: an invalid DB value for one key falls back to env just for that key.** Falling back
  could silently widen exposure (for example, env listing more public view sets than the stored
  value). Failing the whole load and serving the last good snapshot fails safe and matches
  `AudienceGrantsLoader`.

## Decisions

- `MICROMEGAS_DEFAULT_AUDIENCE` stays env-only: it is written into stored data, so changing it is
  a data migration, not a toggle.
- Accepted risk, following the grant/group precedent: on a schema behind v12 the settings load
  fails, so FlightSQL queries and self-service routes return a retryable 503 until ingestion runs
  the migration. Upgrade order is the same as for v10 (`groups`).

## Documentation

- `mkdocs/docs/admin/authorization.md`: the knob table at `:18-25` gains a column for the setting
  key, and a short "Deployment settings" section covers the admin page, the DB → env → default
  order, propagation within the cache TTL, and the shadowed-env warning. The self-service sections
  (`:226`, `:330`) say "the `self_service_mint` setting (env fallback
  `MICROMEGAS_SELF_SERVICE_MINT`)".
- `mkdocs/docs/admin/flight-sql.md:32` and `monolith.md`: `MICROMEGAS_PUBLIC_VIEW_SETS` is
  described as the fallback for `public_view_sets`.
- `mkdocs/docs/admin/web-app.md`: the Deployment Settings page and its REST routes.
- `mkdocs/docs/admin/control-plane-audit-log.md`: the `set_setting` / `reset_setting` actions and
  the `setting` / `value` / `previous_value` fields, plus an example query.
- `CHANGELOG.md` (Unreleased): the feature (#1592), and **Minor breaking change:**
  `FlightSqlServiceImpl::new` takes a `DeploymentSettingsSource` instead of an `IsolationConfig`,
  and `IngestionKeysState` / `AudienceGrantsState` replace their knob fields with `settings`.

## Testing Strategy

All automated tests run without a database. Per `CONTRIBUTING.md`, they use `connect_lazy` pools
and the `with_overrides` seam.

- `rust/public/tests/deployment_settings_tests.rs`:
  - `validate` for each kind:
    - Bool: accepts a JSON bool and rejects `"true"`.
    - Count: accepts `0` and rejects `-1`, `1.5` and a value above `i64::MAX`.
    - NameList: accepts `["a","b"]` and rejects an empty string, untrimmed or duplicate entries,
      and non-arrays.
  - Row parsing into `DbOverrides`: unknown keys are ignored, and an invalid known key makes the
    whole parse fail. Split `fetch` so the row → `DbOverrides` step is a pure function that takes
    `(key, value)` pairs.
  - Resolution order for each accessor: DB, then env, then default.
  - `pinned` ignores both env and DB.
  - The shadowed-env warning logic, as a pure `shadowed(&rows, env_lookup)` function.
- `isolation_config_fail_fast_tests.rs`: still fails at build time on a malformed
  `MICROMEGAS_PUBLIC_VIEW_SETS`, now on every auth branch (pins #1561).
- `rust/public/tests/read_policy_threading_tests.rs` (or a sibling): `caller_context` copies the
  source's current `IsolationConfig`. Building with overrides A and then B gives different
  `public_view_sets`, which shows the value is read per request rather than captured at
  construction.
- `rust/analytics-web-srv/tests/`:
  - The existing self-service gate tests are ported to `with_overrides`.
  - A new case shows that one state with overrides off returns 403, and with overrides on lets the
    caller through (per-request read).
  - `deployment_settings_tests.rs`:
    - A non-admin is rejected with 403 and a `"denied"` audit record.
    - An unknown key on PUT returns 404.
    - An invalid body returns 400 before any DB access (lazy pool).
    - The GET response shape is built from `SETTINGS` plus canned rows.
  - `mutation_audit_tests.rs`: the new fields serialize, and are omitted when absent.
- Frontend (vitest): API client encoding and error mapping; the page renders the source pill,
  each Edit dialog submits the right JSON per kind, and the page re-fetches after a write.

## Manual Verification

These checks need several running services plus a browser. The unit tests above cover each piece,
but not the end-to-end path through a real Postgres and two processes.

1. `python3 local_test_env/ai_scripts/start_services.py`, then run `analytics-web-srv` with auth
   enabled. Confirm that `/tmp/ingestion.log` shows the migration reaching v12.
2. Open `http://127.0.0.1:3000/admin/settings` as an admin. All five settings show "Env / default".
3. As a non-admin, query a view set's rows through `micromegas-query`. Rows are audience-filtered.
   Then, as admin, set `public_view_sets` to that view set and wait about 60 s. The same query now
   returns unfiltered rows, with no restart.
4. Turn `self_service_mint` on. Within about 60 s a non-admin's `/admin/ingestion-keys` mint
   succeeds instead of returning 403.
5. Run `micromegas-query "SELECT msg FROM log_entries WHERE target = 'control_plane_audit' ORDER BY time DESC LIMIT 5" --begin 1h`.
   The output shows `set_setting` records with `previous_value`.
6. Restart flight-sql with `MICROMEGAS_PUBLIC_VIEW_SETS` set. The log shows the shadowed-env
   warning.
7. Reset `public_view_sets`. Filtering returns within about 60 s.

## Open Questions

- **Env as the fallback vs. a one-time seed (Trade-offs, second bullet).** The plan uses env as the
  fallback. Choose seeding instead if you want the DB to be the only source of truth after the
  first boot.
- **Should `MICROMEGAS_QUERY_DENY_MAX_RULES` move in this change too?** It is one more registry
  entry plus a call-site change in `deny_queries`. It's left out to keep this change to the knobs
  named in the issue.
