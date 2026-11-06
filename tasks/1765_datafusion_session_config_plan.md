# DataFusion Session Config Settings Plan

Issue: https://github.com/madesroches/micromegas/issues/1765

## Overview

Every DataFusion session micromegas builds starts from `SessionConfig::default()`, so
`target_partitions` / `planning_concurrency` come from `std::thread::available_parallelism()`.
On CPU-quota'd containers whose VM exposes more cores than the quota allows, that over-fans every
plan, and nothing short of a code change can fix it. This adds two DB-backed settings holding
DataFusion config overrides — `datafusion_config_query` for the query (FlightSQL) service and
`datafusion_config_maintenance` for the maintenance daemon — because the two services run on
different hardware and different workloads (interactive queries vs. background merges). Each
service applies its own overrides to every session it builds, picking up a change on the next
settings-cache refresh with no restart.

## Current State

- **Session construction.** All query sessions go through `make_session_context`
  (`rust/analytics/src/lakehouse/query.rs:304`), which builds
  `SessionConfig::default()` and forces `parquet.enable_page_index = false`,
  `sql_parser.collect_spans = true` and `with_information_schema(true)` (`query.rs:313-320`).
  Other session sites:
  - `make_merge_session_context` (`rust/analytics/src/lakehouse/merge.rs:49`) wraps
    `make_session_context` and then forces `optimizer.repartition_file_scans = false`.
  - `plan_sorted_merge` (`merge.rs:110-124`) forces `enable_round_robin_repartition = false`,
    `repartition_aggregations = false`, `prefer_existing_sort = true`, `repartition_joins = false`
    on the session it is handed.
  - `query_partitions_context` (`query.rs:77-96`) builds a bare `SessionConfig::default()`
    session over a partition list; it is reached only through `query_partitions` (`query.rs:100`),
    whose callers (`jit_partitions.rs` ×7, `partition_source_data.rs:276`) all hold a
    `LakehouseContext` and pass its `runtime()`, `reader_factory()` and `lake().blob_storage`.
  - Scratch sessions in the merge writer loop (`merge.rs:471-472`) and
    `metadata_partition_spec.rs:258` run min/max over one in-memory batch; intentionally unchanged.
  - Construction-time planning sessions in `SqlBatchView::new` (`sql_batch_view.rs:115`),
    `ExportLogView::new` (`export_log_view.rs:118`) and the DDL validator
    (`view_definition.rs:359,383`) build a fresh `LakehouseContext::new(lake, runtime)` just to
    plan the extract query and derive a schema. They never execute data work.
  - `StaticTablesConfigurator::from_env` (`static_tables_configurator.rs:57`) builds a one-off
    startup session to list files.
- **Who holds what.** `LakehouseContext` (`rust/analytics/src/lakehouse/lakehouse_context.rs:30`)
  is the shared per-service handle (lake, runtime, metadata cache, reader factory, deny list).
  FlightSQL derives a per-query copy with `with_runtime` (struct-update clone, `:181`), so any
  field added to it travels into every query session. Views' write/merge paths receive the
  caller's `LakehouseContext` (e.g. `merge.rs:442`, `sql_batch_view.rs:248-259`).
- **Settings.** `rust/public/src/settings.rs` holds the `SETTINGS` registry (`SettingSpec` with
  `key`, `env_var`, `kind`, `default`, `description`), `SettingSpec::validate` (shared by the
  PUT handler in `rust/analytics-web-srv/src/settings.rs:290` and the loader),
  `DbOverrides::from_rows` (typed parse; a known key that fails validation fails the whole load),
  `EnvFallbacks::from_env` (env tier, fail-fast on malformed values) and `SettingsSource`
  (DB → env → default, cached by `SnapshotSource` with stale-on-error after the first load,
  `rust/auth/src/db_snapshot.rs:177`).
- **Wiring.** `SettingsSource` is built in the FlightSQL builder
  (`rust/public/src/servers/flight_sql_server.rs:299-303`), in `telemetry-maintenance-srv`
  (`rust/telemetry-maintenance-srv/src/main.rs:70`) and in the monolith's maintenance branch
  (`rust/monolith/src/main.rs:360`). Both maintenance entry points hand it to
  `daemon(lakehouse, view_registry, settings, ...)` (`rust/public/src/servers/maintenance.rs:435`).
  In the monolith one `LakehouseContext` Arc is shared by the flightsql and maintenance roles,
  in one process.
- **Crate layering.** `settings.rs` lives in the `micromegas` (public) crate, which depends on
  `micromegas_analytics`; `make_session_context` is in `micromegas_analytics` and cannot see
  `SettingsSource`.
- **Admin UI.** `analytics-web-app/src/routes/SettingsPage.tsx` renders and edits each entry by
  `kind` (`analytics-web-app/src/lib/settings-api.ts:8`); `tag_horizons` already has a
  key/value row editor (`SettingsPage.tsx:266-325`).
- **Process properties** are attached in the `micromegas_main` macro
  (`rust/micromegas-proc-macros/src/lib.rs:280`) before telemetry starts — before any DB is
  reachable — so a DB-stored value cannot be recorded there.

## Design

### Analytics crate: overrides type and source trait

New module `rust/analytics/src/lakehouse/datafusion_config.rs`:

```rust
/// Validated DataFusion config overrides: key -> value as `ConfigOptions::set` takes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataFusionConfigOverrides(BTreeMap<String, String>);

/// Keys every session forces; an override could never take effect, so storing one is rejected.
pub const PINNED_KEYS: &[&str] = &[
    "datafusion.execution.parquet.enable_page_index",
    "datafusion.sql_parser.collect_spans",
    "datafusion.catalog.information_schema",
];

impl DataFusionConfigOverrides {
    /// Strict: any bad entry is an error. Used by the PUT handler.
    pub fn parse_strict(value: &serde_json::Value) -> Result<Self>;
    /// Lenient: a non-object is an error, but an entry DataFusion rejects is skipped with a warning.
    pub fn parse_lenient(value: &serde_json::Value) -> Result<Self>;
    pub fn apply(&self, options: &mut ConfigOptions) -> Result<()>;
    pub fn is_empty(&self) -> bool;
}

#[async_trait]
pub trait DataFusionConfigSource: Send + Sync + Debug {
    async fn overrides(&self) -> Result<Arc<DataFusionConfigOverrides>>;
}

/// The source of a `LakehouseContext` nobody configured: no overrides.
#[derive(Debug)]
pub struct NoDataFusionOverrides;
```

The value is a sparse override map applied on top of `SessionConfig::default()`: any key it
doesn't list keeps DataFusion's default, so `{"datafusion.execution.target_partitions": 1}`
changes that one option and nothing else. An unset setting (no row, or `{}`) leaves the
service on pure defaults, and each service's setting is independent of the other's.

Parsing rules (one entry-validation function shared by both parse modes):
- The value must be a JSON object. Each entry's value must be a JSON string, number or boolean,
  converted to the string `ConfigOptions::set` expects (`1` → `"1"`, `true` → `"true"`), so both
  `{"datafusion.execution.target_partitions": 1}` and `"1"` work. Null, arrays and objects are
  rejected.
- A key in `PINNED_KEYS` is rejected.
- The entry is applied to a scratch `ConfigOptions::new()`; an unknown key or unparsable value
  (DataFusion's own error) is rejected. `datafusion.runtime.*` keys are not in `ConfigOptions`
  and are therefore rejected as unknown — the memory budget stays
  `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB`.

Merge-pinned keys (`repartition_file_scans` and the four `plan_sorted_merge` optimizer settings)
are **not** rejected: they are legitimate tuning knobs for ordinary queries. They lose to the
forced values in merges because overrides are applied when the session config is built, and the
merge paths mutate the config afterwards — the order the code already has.

### LakehouseContext carries the source

- New field `datafusion_config: Arc<dyn DataFusionConfigSource>`, set to `NoDataFusionOverrides`
  by `new` and `with_caches`, preserved by `with_runtime`'s struct-update clone.
- `pub fn with_datafusion_config(&self, source: Arc<dyn DataFusionConfigSource>) -> Arc<Self>` —
  struct-update clone like `with_runtime`, so caches and the reader factory stay shared. This is
  what lets the monolith run flightsql and maintenance with different configs off one
  `LakehouseContext`.
- `pub async fn session_config(&self) -> Result<SessionConfig>`: `SessionConfig::default()`,
  apply `self.datafusion_config.overrides().await?`, then return it. This is the single place a
  session's base config comes from.

### Session sites use it

- `make_session_context`: `lakehouse.session_config().await?` then the existing forced
  `set_bool`s / `with_information_schema(true)` (forced values last, so they win even if a pinned
  key ever slipped past validation).
- `query_partitions` / `query_partitions_context`: take `lakehouse: &LakehouseContext` instead of
  `(runtime, reader_factory, object_store)` — every caller already destructures a lakehouse into
  those three — and build from `lakehouse.session_config()`.
- `make_merge_session_context` and `plan_sorted_merge` are unchanged; they already apply their
  forced values after `make_session_context`.
- Construction-time planning sessions (`SqlBatchView::new`, `ExportLogView::new`, the DDL
  validator) and `StaticTablesConfigurator` keep the no-overrides default: they only derive
  schemas or list files at startup, and no tuning knob changes a schema.

### Public crate: settings and adapter

In `rust/public/src/settings.rs`:

- Two new keys and specs, kind `SettingKind::DataFusionConfig` (`as_str` `"datafusion_config"`),
  default `"{}"`, `env_var: None` (like `retention_by_stream_tag`):
  - `datafusion_config_query` — "DataFusion
    config overrides for every FlightSQL query session, e.g.
    `{"datafusion.execution.target_partitions": 2}`."
  - `datafusion_config_maintenance` — same, for
    the maintenance daemon's materialization and merge sessions.
  Both descriptions name the pinned keys and note that merges keep their own forced optimizer
  settings.
- `SettingSpec::validate` for `DataFusionConfig` calls `parse_strict` — the write path rejects
  any bad entry with DataFusion's message.
- `DbOverrides` gains `datafusion_config_query` / `datafusion_config_maintenance:
  Option<Arc<DataFusionConfigOverrides>>`. In `from_rows`, these kinds use `parse_lenient`
  instead of `spec.validate`: a stored entry that a different DataFusion version rejects (a key
  removed or renamed by an upgrade, or written by a newer replica during a rolling upgrade) is
  skipped with a `warn!` naming the key, rather than failing the whole settings load — which would
  take isolation and self-service down with it. A non-object still fails the load.
  `stored_keys` lists both. `EnvFallbacks` is unchanged.
- `pub enum ServiceRole { Query, Maintenance }` and
  `SettingsSource::datafusion_config(&self, role) -> Result<Arc<DataFusionConfigOverrides>>`
  resolving the stored value, else empty overrides.
- `pub struct SettingsDataFusionConfig { settings: Arc<SettingsSource>, role: ServiceRole,
  last_logged: Mutex<Option<Arc<DataFusionConfigOverrides>>> }` implementing
  `DataFusionConfigSource`. It logs
  `info!("datafusion config ({role}): {overrides}")` on the first session and on every change
  (by value) of the resolved overrides.

Resolution failure (cold start with an unreadable settings table) fails the session build,
matching how `isolation_config` already fails FlightSQL queries with a retryable error; after the
first load, `SnapshotSource` serves the stale snapshot. Falling back to no overrides silently
would undo exactly the protection a quota'd deployment configured.

### Wiring

```
FlightSqlServer::build_and_serve           daemon(lakehouse, view_registry, settings, ..)
  settings = resolve_settings(..)            lakehouse = lakehouse.with_datafusion_config(
  lakehouse = lakehouse.with_datafusion_config(   SettingsDataFusionConfig(settings, Maintenance))
      SettingsDataFusionConfig(settings, Query))   -> EveryDay/EveryHour/EveryMinute tasks
  -> FlightSqlServiceImpl (per query: with_runtime keeps it)
```

- FlightSQL builder (`flight_sql_server.rs`, right after `settings` is created at `:299`): wrap
  `lakehouse` before it is moved into `FlightSqlServiceImpl::new`. Covers standalone
  `flight-sql-srv` and the monolith's flightsql role.
- `daemon` (`maintenance.rs:435`): wrap `lakehouse` on entry, before it is cloned into the cron
  tasks. Covers `telemetry-maintenance-srv` and the monolith's maintenance role with one edit.
- `with_isolation_config` / `SettingsSource::pinned` (`flight_sql_server.rs:64-77`, `:190`) also
  turns off the DB-backed DataFusion overrides; the rustdoc on those must say so.
- Admin `check_view_merge` runs inside FlightSQL and therefore plans with the query config.
  The merge-pinned optimizer settings decide whether a merge streams, and those are forced in both
  services, so the check's verdict does not depend on which config it planned with.

### Admin UI

- `settings-api.ts`: add `'datafusion_config'` to `SettingKind`.
- `SettingsPage.tsx`:
  - `ValueView`: render as a key → value list (like `tag_horizons`), `{}` shown as "none".
  - `EditDialog`: a key/value row editor, structured like the `tag_horizons` rows — a key text
    input, a value text input, a remove button, and "Add option". `pendingValue` builds
    `{key: value}` with values sent as strings; client-side validity only requires non-empty,
    unique keys and non-empty values. Everything else is the server's job: a 400 from the PUT
    shows DataFusion's message in the existing error slot.
  - A short hint under the editor linking to the docs section that lists the candidate keys.

## Implementation Steps

1. **Analytics overrides module.** Add `rust/analytics/src/lakehouse/datafusion_config.rs`
   (overrides type, both parse modes, `apply`, `PINNED_KEYS`, trait, `NoDataFusionOverrides`);
   register it in `lakehouse/mod.rs`.
2. **LakehouseContext.** Add the field, `with_datafusion_config`, `session_config()`
   (`lakehouse_context.rs`).
3. **Session sites.** Switch `make_session_context`, `query_partitions(_context)` (new
   signature) to `session_config()`; update the
   `query_partitions` callers in `jit_partitions.rs` and `partition_source_data.rs`.
4. **Settings.** In `rust/public/src/settings.rs`: the two keys/specs, `SettingKind::
   DataFusionConfig`, `validate`, `DbOverrides` fields + lenient `from_rows` branch +
   `stored_keys`, `ServiceRole`, `SettingsSource::datafusion_config`,
   `SettingsDataFusionConfig`. Update the docs on `SettingSpec::validate` (`settings.rs:132`) and
   `from_rows` (`settings.rs:260-263`); `from_rows` must skip `validate` for `DataFusionConfig`
   (it currently calls `spec.validate` unconditionally before the match, ~`:272-273`).
5. **Wiring.** FlightSQL builder and `daemon` wrap their lakehouse; update the rustdoc on
   `with_isolation_config` / `SettingsSource::pinned` to say they also disable the DataFusion
   overrides.
6. **Web app.** `settings-api.ts` kind, and widen `SettingValue` (`settings-api.ts:10`) with
   `Record<string, string>`; `SettingsPage.tsx` view + editor.
7. **Docs and CHANGELOG** (see below).
8. **Tests** (see Testing Strategy).

## Files to Modify

- `rust/analytics/src/lakehouse/datafusion_config.rs` (new)
- `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/src/lakehouse/lakehouse_context.rs`
- `rust/analytics/src/lakehouse/query.rs`
- `rust/analytics/src/lakehouse/jit_partitions.rs`
- `rust/analytics/src/lakehouse/partition_source_data.rs`
- `rust/public/src/settings.rs`
- `rust/public/src/servers/flight_sql_server.rs`
- `rust/public/src/servers/maintenance.rs`
- `analytics-web-app/src/lib/settings-api.ts`
- `analytics-web-app/src/routes/SettingsPage.tsx`
- `rust/analytics/tests/datafusion_config_tests.rs` (new)
- `rust/analytics/tests/merge_scan_partitioning_tests.rs`
- `rust/public/tests/settings_tests.rs`
- `rust/analytics-web-srv/tests/settings_tests.rs`
- `analytics-web-app/src/routes/__tests__/SettingsPage.test.tsx`
- `mkdocs/docs/admin/authorization.md`, `mkdocs/docs/admin/flight-sql.md`,
  `mkdocs/docs/admin/maintenance.md`, `mkdocs/docs/admin/monolith.md`,
  `mkdocs/docs/admin/web-app.md`
- `CHANGELOG.md`

## Trade-offs

- **Two keys vs. one object keyed by service.** Two keys reuse the registry as-is: each gets its
  own row, Reset button and audit entry. A single
  `{"query": {...}, "maintenance": {...}}` object would need a nested editor and makes a Reset
  wipe both services.
- **Carrying the source on `LakehouseContext` vs. a new `make_session_context` parameter.** The
  lakehouse already reaches every execution session, including the per-query copy and the
  `query_partitions` callers, and the role is a per-service fact like the deny list. A parameter
  would touch every `make_session_context` call site (a dozen, mostly planning-only) to pass the
  same value. Cost: a future execution path that builds a fresh `LakehouseContext::new` would
  silently get no overrides; today only construction-time planning paths do that.
- **Process-global config vs. per-lakehouse.** A global would be simpler but breaks the monolith,
  where both roles share a process.
- **Reject merge-pinned keys vs. apply-then-force.** Rejecting would forbid
  `repartition_file_scans` / `repartition_aggregations` for interactive queries, which are among
  the most useful knobs on a few-core host. Only the keys that are forced in *every* session are
  rejected.
- **Log-on-change vs. process property / effective config on the admin page.** Process properties
  are fixed before the DB is reachable. The admin page runs in `analytics-web-srv`, on a different
  host from either service, so it can't compute host-dependent defaults like `target_partitions`;
  it shows the stored overrides, and each process logs what it actually applies.

## Decisions

- User call: separate settings for the query (FlightSQL) and maintenance services, since they
  don't necessarily run on the same hardware.
- User call: the FlightSQL service's setting is named after its role, `query`, not after its
  protocol (`flightsql`) and not `analytics` (ambiguous with `analytics-web-srv`):
  `datafusion_config_query`, `ServiceRole::Query`.
- User call: no env-var fallback for either setting; the stored value or nothing.
- Lenient loader; write path stays strict, so leniency only matters across a DataFusion version change.

## Documentation

- `mkdocs/docs/admin/authorization.md#settings`: list the two keys in the settings section; note
  that a stored entry DataFusion rejects after an upgrade is skipped with a warning instead of
  failing the load.
- `mkdocs/docs/admin/flight-sql.md` and `mkdocs/docs/admin/maintenance.md`: a "DataFusion session
  config" section — the setting key, value shape, pinned keys, that merges keep their
  forced optimizer settings, pickup within one cache TTL, the `datafusion config (<role>)` log
  line, the candidate-keys table from the issue, and a quota'd-container note pairing
  `datafusion.execution.target_partitions` with `TOKIO_WORKER_THREADS`. One page holds the table
  and the other links to it.
- `mkdocs/docs/admin/flight-sql.md`: also note that a pinned settings source
  (`with_isolation_config`) disables the DB-backed DataFusion overrides.
- `mkdocs/docs/admin/monolith.md`: one line saying each role reads its own key.
- `mkdocs/docs/admin/web-app.md` (~:136-138): add `datafusion_config_query` and
  `datafusion_config_maintenance` to the list of editable settings.
- `CHANGELOG.md` (Unreleased): new settings entry. **Minor breaking change:** `query_partitions`
  and `query_partitions_context` take `&LakehouseContext` instead of runtime / reader factory /
  object store; `LakehouseContext` gains a field; `DbOverrides` and `SettingKind`
  gain members. SQL surface unchanged.

## Testing Strategy

All no-DB unit tests.

- `rust/analytics/tests/datafusion_config_tests.rs`:
  - `parse_strict` accepts string, number and boolean values and stringifies them; rejects a
    non-object, null/array/object values, an unknown key, an unparsable value (e.g.
    `target_partitions: "abc"`), each `PINNED_KEYS` entry, and a `datafusion.runtime.*` key;
    accepts `repartition_file_scans`.
  - `parse_lenient` skips the bad entries above and keeps the good ones; errors on a non-object.
  - `apply` sets the values on a `ConfigOptions`.
  - Copy `make_offline_lakehouse_context` from `merge_scan_partitioning_tests.rs` into
    `datafusion_config_tests.rs` (`connect_lazy` pool, in-memory store) with a fixed `DataFusionConfigSource` stub:
    `make_session_context` reflects `target_partitions = 1` and `batch_size = 4096`, still has
    `enable_page_index = false` / `collect_spans = true`; a `with_runtime` copy keeps the
    overrides; `query_partitions` reflects them; a source returning `Err` fails
    `make_session_context`.
- `rust/analytics/tests/merge_scan_partitioning_tests.rs`: with an override of
  `repartition_file_scans = true` and `enable_round_robin_repartition = true`,
  `make_merge_session_context` + `plan_sorted_merge` still force them off — the precedence rule
  that keeps merges streaming.
- `rust/public/tests/settings_tests.rs`: both specs validate strictly; `from_rows` with a stored
  object containing one unknown key loads the rest and does not fail the parse, while a
  non-object fails it; `SettingsSource::with_overrides` resolves the stored value, else empty, per role, and
  the two roles resolve independently; `SettingsDataFusionConfig` logs once per distinct value
  (assert on the `datafusion config (<role>)` line via `init_in_memory_tracing` / `InMemorySink`,
  `#[serial]`).
- `rust/analytics-web-srv/tests/settings_tests.rs`: `build_entries` lists the two keys with kind
  `datafusion_config` and default `{}`; the PUT path rejects an unknown DataFusion key with a
  400 carrying the key name (through the router, like `invalid_value_is_400_before_any_db_access`).
- `analytics-web-app/src/routes/__tests__/SettingsPage.test.tsx`: the `datafusion_config` editor
  adds/removes rows and submits `{key: value}`; duplicate or empty keys disable Save; a server
  400 message is shown.

## Manual Verification

End-to-end wiring (that each service actually installs its role's source) is plumbing whose
breakage is obvious on first use, so it is checked by hand rather than with a live-DB test.

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`.
2. In Admin → Settings, set `datafusion_config_query` to
   `{"datafusion.execution.target_partitions": "1"}` and `datafusion_config_maintenance` to
   `{"datafusion.execution.target_partitions": "3"}`.
3. After ~60 s, `micromegas-query "SELECT value FROM information_schema.df_settings WHERE name =
   'datafusion.execution.target_partitions'"` → `1`.
4. `micromegas-query "SELECT msg FROM log_entries WHERE msg LIKE 'datafusion config%'" --begin 10m`
   → one `(query)` line with `target_partitions: 1` and, after the next maintenance tick, one
   `(maintenance)` line with `3`.
