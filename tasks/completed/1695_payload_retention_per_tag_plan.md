# Payload Retention per Stream Tag Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1695

## Overview

Raw payload retention (rows in `blocks` and their objects under `blobs/`) uses one global horizon:
`--retention-days` / `MICROMEGAS_RETENTION_DAYS`, read once at startup by the maintenance daemon.
Some streams need a different horizon. The control-plane audit stream from #1606 (tagged
`["log", "audit"]`) must be kept forever, and high-volume debug streams may only need a few days.
This plan adds two settings to the DB-backed settings store from #1592: `retention_days`, the
default horizon (with `MICROMEGAS_RETENTION_DAYS` and the flag as its fallback), and
`retention_by_stream_tag`, a map from stream tag to a horizon in days or `"forever"`. The hourly
maintenance task reads both on every pass. It runs one block-deletion pass for the default horizon
that skips overridden streams, and one pass for each distinct override horizon. A GIN index on
`streams.tags` lets the override passes find their streams without scanning `streams`.

## Current State

- **Deletion:** `delete_old_data(lake, min_days_old: i32)` (`rust/analytics/src/delete.rs:152`)
  computes one `expiration = now - days` and runs, in order:
  - `delete_expired_blocks` (`:52`): loops `delete_expired_blocks_batch` (`:13`), which deletes up
    to 1000 `blocks` rows with `insert_time <= $1`, deletes their blobs
    (`blobs/{process_id}/{stream_id}/{block_id}`), then commits.
  - `delete_empty_streams` (`:98`) and `delete_empty_processes` (`:145`): delete rows older than
    the cutoff that have no children left. A stream that still has blocks survives, and so does its
    process.
  - `retire_expired_partitions` (`lakehouse/write_partition.rs`): retires lakehouse partitions
    older than the same cutoff.
- **Caller:** `EveryHourTask { retention_days: i32, .. }` (`rust/public/src/servers/maintenance.rs:216-228`)
  calls `delete_old_data` first in its `run`, and a failure aborts the rest of the hourly task.
  `daemon(lakehouse, view_registry, retention_days, shutdown, grace)` (`:429`) builds the task.
  The CronTask runs at HH:10 (`period = 1h`, `offset = 10m`; first run is the next hour boundary
  plus the offset, `cron_task.rs:35`).
- **Flag:** `retention_days: i32` with `default_value = "90", env = "MICROMEGAS_RETENTION_DAYS"`
  in `rust/telemetry-maintenance-srv/src/main.rs:29-31` and `rust/monolith/src/main.rs:168-170`
  (passed to `daemon` at `:354,377`). Any `i32` is accepted. A negative value makes
  `checked_sub_days` fail and every hourly pass error out.
- **Settings store (#1592):** `rust/public/src/settings.rs`. `SETTINGS` is the registry of
  `SettingSpec { key, env_var, kind, default, description }`, with `SettingKind::{Bool, Count, NameList}`
  and `validate` shared by the PUT handler and the loader. `DbOverrides::from_rows` parses the table
  into typed `Option` fields. An invalid known key fails the whole load, and `SnapshotSource` then
  keeps serving the last good snapshot, or fails with `ProviderUnavailable` on a cold start.
  `EnvFallbacks::from_env` resolves the env half once. `SettingsSource` exposes typed accessors
  (`isolation_config`, `self_service`) and `warn_on_shadowed_env`. The maintenance daemon does not
  use it yet.
- **Settings UI/REST:** `rust/analytics-web-srv/src/settings.rs` lists every `SETTINGS` entry
  (`env_var` is serialized at `:189,207`) and validates PUTs with `spec.validate`. Every change is
  audited as `set_setting` / `reset_setting` with `value` / `previous_value`.
  `analytics-web-app/src/routes/SettingsPage.tsx` groups entries by key prefix (`GROUPS`, `:26`),
  renders values per kind (`ValueView`, `:42`), and has one editor per kind (`EditDialog`, `:69`).
  It always shows `env fallback: <env_var>` (`:151,371`).
- **Index:** `streams` has indexes on `stream_id`, `process_id` and `insert_time`
  (`rust/ingestion/src/sql_telemetry_db.rs:61-63`) and a unique index on `stream_id` (v3), none on
  `tags TEXT[]`. Schema is v12 (`rust/ingestion/src/sql_migration.rs:8`). v3 sets the precedent for
  `CREATE INDEX CONCURRENTLY` outside a transaction, followed by `check_index_is_valid` (`:462`).
- **Partition retention** for materialized views is a separate per-view option in the #1606 plan.
  This plan does not change it: partitions keep following the default horizon.

## Design

### Retention policy type: `micromegas-analytics` (`rust/analytics/src/delete.rs`)

`delete.rs` consumes the policy, and `micromegas` (`rust/public`), where settings live, already
depends on `micromegas-analytics`, so the type lives here:

```rust
pub const MAX_RETENTION_DAYS: u32 = 36_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionHorizon { Days(u32), Forever }   // derived Ord: Days(a) < Days(b) < Forever

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadRetention {
    pub default_days: u32,
    pub by_stream_tag: BTreeMap<String, RetentionHorizon>,
}

/// One bounded block-deletion pass.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockDeletePass {
    pub cutoff: DateTime<Utc>,
    /// `None`: every stream (the default pass). `Some(tags)`: only streams carrying one of them.
    pub include_tags: Option<Vec<String>>,
    /// Streams carrying any of these are left alone by this pass.
    pub exclude_tags: Vec<String>,
}

impl PayloadRetention {
    pub fn default_cutoff(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>>;
    pub fn block_delete_passes(&self, now: DateTime<Utc>) -> Result<Vec<BlockDeletePass>>;
}
```

**Semantics.** A stream's horizon is the longest horizon among its tags that have an override. If
none of its tags has one, it uses the default. An override can be shorter than the default (the
debug case) or longer (the audit case). When a stream matches several overrides, the longest one
wins, because deletion can't be undone: keeping too much is the recoverable mistake.

**Pass planning** (`block_delete_passes`, a pure function):

- Default pass: `cutoff = now - default_days`, `include_tags = None`, `exclude_tags =` every
  overridden tag.
- For each distinct finite horizon `h` among the overrides: `cutoff = now - h`,
  `include_tags =` the tags whose horizon is `h`, `exclude_tags =` the tags whose horizon is longer
  than `h`, `Forever` included. That second list is what makes the longest horizon win.
- `Forever` tags get no pass of their own. They only appear in other passes' `exclude_tags`.
- With no overrides, the result is a single default pass with an empty exclude list, which is
  exactly today's behavior.

**SQL.** `delete_expired_blocks_batch(lake, pass: &BlockDeletePass)` keeps its batch, blob-delete
and commit shape. Only the inner `SELECT` changes:

```sql
-- default pass (include_tags = None); $3 = exclude_tags
SELECT b.block_id FROM blocks b
WHERE  b.insert_time <= $1
AND    NOT EXISTS (SELECT 1 FROM streams s WHERE s.stream_id = b.stream_id AND s.tags && $3)
LIMIT  $2

-- override pass; $3 = include_tags, $4 = exclude_tags
SELECT b.block_id FROM streams s JOIN blocks b ON b.stream_id = s.stream_id
WHERE  s.tags && $3 AND NOT (s.tags && $4) AND b.insert_time <= $1
LIMIT  $2
```

The default pass keeps today's handling of blocks with no `streams` row and of streams with
`NULL` tags (`NULL && x` is `NULL`, so `NOT EXISTS` holds and the block is deleted), and with an
empty `$3` it is equivalent to today's query. The override pass reaches its streams through the
new GIN index and their blocks through `block_stream_id`.

**`delete_old_data(lake, retention: &PayloadRetention)`** runs every pass from
`block_delete_passes(now)` through `delete_expired_blocks`, then `delete_empty_streams`,
`delete_empty_processes` and `retire_expired_partitions` with `default_cutoff(now)`, unchanged. The
existing "no children left" rule keeps the stream and process rows of a long-horizon stream alive
while it has blocks. A short-horizon stream's rows wait for the default horizon, which costs
nothing that matters.

### Settings: `rust/public/src/settings.rs`

Two new registry entries, grouped under the `retention_` prefix:

| key | kind | env fallback | default | value |
|---|---|---|---|---|
| `retention_days` | `Days` | `MICROMEGAS_RETENTION_DAYS` | `90` | JSON integer in `1..=MAX_RETENTION_DAYS` |
| `retention_by_stream_tag` | `TagHorizons` | none | `{}` | JSON object `{tag: days \| "forever"}` |

- **`SettingKind`** gains `Days` (`as_str` `"days"`) and `TagHorizons` (`"tag_horizons"`).
  `Days` rejects `0`: in a form-edited setting, a `0` would delete every payload on the next hourly
  pass. The upper bound keeps the date arithmetic in range. `TagHorizons` requires an object
  whose keys are non-empty and trimmed, and whose values are either a `Days` integer or the string
  `"forever"`. A `parse_tag_horizons(key, value) -> Result<BTreeMap<String, RetentionHorizon>>`
  helper is shared by `validate` and `from_rows`, the way `name_list` is.
- **`SettingSpec::env_var`** becomes `Option<&'static str>`. The overrides have no env fallback
  (the issue's point is not to add one more env var), and `None` says so. `shadowed` skips specs
  with `None`, and `EnvFallbacks::from_env`'s `spec(..)` helper unwraps. `warn_on_shadowed_env`
  (`:427-430`) formats `spec.env_var` with `{}`, so it unwraps too (it is only reached through
  `shadowed`). `EnvGuard::set` in `rust/public/tests/settings_tests.rs:223` collects
  `.map(|s| s.env_var)` into `Vec<&'static str>` and switches to `filter_map`.
- **`DbOverrides`** gains `retention_days: Option<u32>` and
  `retention_by_stream_tag: Option<BTreeMap<String, RetentionHorizon>>`, handled in `from_rows` and
  `stored_keys`.
- **`EnvFallbacks`** gains `retention_days: u32`. `from_env` and `pinned` set 90;
  maintenance overwrites it with the clap value.
- **`SettingsSource::payload_retention(&self) -> Result<PayloadRetention>`**: `default_days` comes
  from the DB row, then the env value. `by_stream_tag` comes from the DB row, else empty.

### Maintenance wiring

- `daemon(lakehouse, view_registry, settings: Arc<SettingsSource>, shutdown, grace)` replaces
  `retention_days`. `EveryHourTask` holds `settings`. Its `run` starts with
  `self.settings.payload_retention().await`. On `Ok` it calls `delete_old_data(lake, &retention)`.
  **If the settings can't be loaded, the error is logged, `delete_old_data` is skipped, and the rest
  of the hourly task (temporary-file cleanup, mismatch metrics, `materialize_all_views`) still
  runs.** After a first successful load,
  `SnapshotSource` keeps serving the last good snapshot. Falling back to the default horizon instead
  would delete blocks the override was keeping.
- `telemetry-maintenance-srv` and the monolith's maintenance role build
  `SettingsSource::new(EnvFallbacks { retention_days: args.retention_days, ..EnvFallbacks::from_env()? }, lake.db_pool.clone(), settings_cache_ttl())`
  and call `warn_on_shadowed_env` once at startup. The lake pool is used as is: the read happens
  once an hour, off any request path, so the small dedicated pool FlightSQL uses for auth isn't
  needed.
- The `--retention-days` flag stays as the fallback (clap resolves flag, then env, then 90). Its
  type becomes `u32` with `value_parser!(u32).range(1..=micromegas::analytics::delete::MAX_RETENTION_DAYS as i64)` in both
  mains (only the frontend keeps a literal), so the flag, env and DB all accept
  the same range. `warn_on_shadowed_env` only looks at the env var, so a stored value shadowing an
  explicit `--retention-days` is not reported. The docs say so.
- Both `--retention-days` doc comments (`telemetry-maintenance-srv/src/main.rs:28`,
  `monolith/src/main.rs:168`) are reworded: the flag is the fallback for the `retention_days`
  setting when no value is stored, and tag overrides can change it per stream.

### Index: data-lake migration v13 (`rust/ingestion/src/sql_migration.rs`)

Follow the v3 shape. Outside a transaction, run
`CREATE INDEX CONCURRENTLY IF NOT EXISTS stream_tags ON streams USING GIN (tags);`, then
`check_index_is_valid(&pool, "stream_tags")`. If `check_index_is_valid` returns false (it has already dropped the index), bail so the next
startup retries. Then bump to 13 in a transaction (`upgrade_data_lake_schema_v13`).
`CONCURRENTLY` keeps ingestion writing to `streams` while the index builds. The default GIN
`array_ops` opclass supports `&&`. A fresh DB gets the index through the same chain.
`LATEST_DATA_LAKE_SCHEMA_VERSION` becomes 13. A v12 schema still works, because the index only
affects performance.

Update `warn_if_data_lake_schema_stale` (`:29-55`) too: services that don't migrate (flight-sql-srv,
analytics-web-srv) call it, and it must not warn falsely on a v12 database. Gate the existing
zero-rows sentence on `version < 9`, and add a clause that a schema older than v13 only lacks the
`stream_tags` index (payload deletion performance).

### Web server and frontend

- `rust/analytics-web-srv/src/settings.rs`: the entry's `env_var` becomes `Option<&'static str>`
  and serializes as `null`. No other change: GET, PUT validation and audit already cover every
  registered key.
- `analytics-web-app/src/lib/settings-api.ts`: `SettingKind` adds `'days' | 'tag_horizons'`,
  `SettingValue` adds `Record<string, number | 'forever'>`, and `env_var` becomes `string | null`.
- `analytics-web-app/src/routes/SettingsPage.tsx`:
  - `GROUPS` adds `{ prefix: 'retention_', title: 'Data retention' }`.
  - `ValueView` renders `tag_horizons` as chips (`audit: forever`, `debug: 3 d`, or "no overrides").
  - `EditDialog`: `days` reuses the count number input with a `1..=36500` check. `tag_horizons` is
    a row editor (tag, days, a Forever checkbox that disables days, remove) with an "Add tag"
    button. The submit check requires non-empty, unique trimmed tags and valid days. A fixed notice
    says that removing or shortening an override lets older blocks be deleted at the next hourly
    pass and can't be undone.
  - A `null` `env_var` shows "no env fallback" in the row and the source pill "Default" when no
    value is stored. The dialog's pre-fill note (`:149-153`) drops the env sentence for a null
    `env_var` and gains the `--retention-days` flag tier for `retention_days`. The Reset confirmation
    (`:333`) and page banner (`:314-317`) stop saying "environment variable, then default"
    unconditionally: no env var for `retention_by_stream_tag`, plus the flag tier for `retention_days`.
  - `pendingValue` (`:100-106`) and `canSubmit` (`:108`) fall through to the name-list path for every
    non-`bool`/`count` kind, and the pre-fill effect (`:81-84`) seeds only `countText` and `names`.
    Add explicit `days` and `tag_horizons` branches to all three so `days` is not submitted as a
    name list and both editors are seeded.
  - `settings-api.ts` doc comments at `:18` and `:76` repeat the "env var, then default" wording;
    update them.

## Mockups

- `tasks/1695_payload_retention_mockups/settings-retention.html`: the Settings page with the new
  "Data retention" group and the `retention_by_stream_tag` row editor open.

## Implementation Steps

1. **Policy and deletion** (`rust/analytics/src/delete.rs`): add `MAX_RETENTION_DAYS`,
   `RetentionHorizon`, `PayloadRetention`, `BlockDeletePass` and `block_delete_passes` /
   `default_cutoff`. Make `delete_expired_blocks(_batch)` take a `&BlockDeletePass`, and
   `delete_old_data` take `&PayloadRetention`. Add `rust/analytics/tests/payload_retention_tests.rs`.
2. **Index**: add the v13 step and `upgrade_data_lake_schema_v13` to `sql_migration.rs`.
3. **Settings** (`rust/public/src/settings.rs`): add the two kinds, `parse_tag_horizons`, the two
   specs, `env_var: Option`, the new `DbOverrides` / `EnvFallbacks` fields and `payload_retention()`.
   Update the `EnvFallbacks { .. }` / `DbOverrides { .. }` literals in `rust/public/tests/`
   (`settings_tests.rs`, `read_policy_threading_tests.rs`) and `rust/analytics-web-srv/tests/`
   (`routing_tests.rs`, `audience_grants_tests.rs`, `ingestion_keys_tests.rs`,
   `mutation_audit_tests.rs`). Most `DbOverrides` literals already end in `..Default::default()`.
   The `EnvFallbacks` ones need the new field.
4. **Maintenance**: change `daemon` / `EveryHourTask` in `rust/public/src/servers/maintenance.rs`,
   and the flag type and construction in `rust/telemetry-maintenance-srv/src/main.rs` and
   `rust/monolith/src/main.rs`.
5. **Web**: switch `env_var` to `Option` in `rust/analytics-web-srv/src/settings.rs`. Update
   `settings-api.ts` and `SettingsPage.tsx` with their tests.
6. **Docs and changelog**: see Documentation.

## Files to Modify

- `rust/analytics/src/delete.rs`, `rust/analytics/tests/payload_retention_tests.rs` (new)
- `rust/ingestion/src/sql_migration.rs`
- `rust/public/src/settings.rs`, `rust/public/src/servers/maintenance.rs`
- `rust/public/tests/settings_tests.rs`, `rust/public/tests/read_policy_threading_tests.rs`
- `rust/telemetry-maintenance-srv/src/main.rs`, `rust/monolith/src/main.rs`
- `rust/analytics-web-srv/src/settings.rs`, `rust/analytics-web-srv/tests/{settings_tests,routing_tests,audience_grants_tests,ingestion_keys_tests,mutation_audit_tests}.rs`
- `analytics-web-app/src/lib/settings-api.ts`, `analytics-web-app/src/routes/SettingsPage.tsx`,
  plus `src/lib/__tests__/settings-api.test.ts` and `src/routes/__tests__/SettingsPage.test.tsx`
- `mkdocs/docs/admin/maintenance.md`, `mkdocs/docs/admin/web-app.md`, `mkdocs/docs/admin/monolith.md`,
  `mkdocs/docs/admin/authorization.md`, `mkdocs/docs/when-to-use/index.md`,
  `welcome/public/llms.txt`
- `tasks/backlog/1606_transactional_audit_write_plan.md`
- `tasks/backlog/1245_partition_blocks_by_insert_time/plan.md`,
  `tasks/backlog/1245_partition_blocks_by_insert_time/rollback.md`
- `CHANGELOG.md`

## Trade-offs

- **Two settings rather than one `{default, overrides}` object.** The default has an env and flag
  fallback and the overrides don't. One key would force all-or-nothing storage: storing only the
  overrides would also pin the default and silently shadow `MICROMEGAS_RETENTION_DAYS`.
- **Known cost of the default pass:** blocks of long-horizon streams older than the default cutoff
  stay at the head of the `block_insert_time` index, and every default batch walks past them
  through the `NOT EXISTS` probe. That is fine for low-volume streams like audit. A high-volume
  stream with a long override would need a different approach (e.g. a per-stream cutoff column),
  which is not built until someone needs it.

## Decisions

- Overrides use one pass per distinct horizon, not a per-row cutoff, which would defeat the `insert_time` index.
- Partition retirement keeps using the default horizon. Partition retention per view is the #1606
  plan's per-view `retention` option, whose "absent" case now means `retention_days`.
- `telemetry-maintenance-srv` and the monolith maintenance role now fail at startup on a malformed
  `MICROMEGAS_PUBLIC_VIEW_SETS`, because they build `EnvFallbacks::from_env()`. Accepted, following
  the same choice for `analytics-web-srv` in #1592.
- `--retention-days 0` and negative values are now rejected at startup. A negative value already
  broke every hourly pass, and `0` deleted every payload hourly.
- Accepted risk: on a schema behind v12 the settings load fails, so payload deletion is skipped (logged
  each hour; the rest of the hourly task still runs) until ingestion (or the monolith) runs the migration. Upgrade order is the same as for #1592.

## Documentation

- `mkdocs/docs/admin/maintenance.md`: rewrite the `### Retention` section (`:92-98`, including its "Deletes lake data older than the
  retention horizon" bullet, which stops being accurate with overrides) and the horizon paragraph at
  the end of `### Integrity counts` (`:146-156`) around the two settings. Cover the resolution order (stored, then `--retention-days` / `MICROMEGAS_RETENTION_DAYS`,
  then 90), the override format with an `{"audit": "forever", "debug": 3}` example, longest-wins
  matching, the fact that overrides cover raw payloads only (partitions follow the default), so blocks
  kept past the default horizon can't be queried until the `blocks` view, then its dependent views, are
  rematerialized for that range, that removing an override can't be undone, propagation within one cache TTL plus the next HH:10 pass,
  and that payload deletion is skipped (with the rest of the hourly task still running) while settings can't be loaded. Note that a stored `retention_days` silently shadows an explicit
  `--retention-days` flag (no startup warning), and that infrastructure-as-code users can write the overrides with
  `PUT {base}/api/settings/retention_by_stream_tag`. Update the flag table row (`:46`)
  with the new range.
- `mkdocs/docs/admin/web-app.md:133-140`: add the two settings to the Settings page's list, and fix
  the rest of the paragraph: the source pill is no longer always "Env / default", the editors are no
  longer only "a toggle, a number, or a list of names" (add the tag-map editor), **Reset** reverts to
  "its environment variable or default" (not for `retention_by_stream_tag`, which has no env var),
  and the page shows the variable name only for settings that have one.
- `mkdocs/docs/admin/monolith.md`: the CLI flag table (`:73-83`) is missing `--retention-days`
  today. Add it, described as the fallback for the `retention_days` setting (maintenance role),
  with a link to `maintenance.md`. Also add a `MICROMEGAS_RETENTION_DAYS` row to its env-var
  table (`:40-59`). Update the "five [settings]" env-fallback sentence (`:67-69`): the count becomes
  six and the list gains `MICROMEGAS_RETENTION_DAYS`.
- `mkdocs/docs/admin/authorization.md`: in the Settings section, update `:18-19` ("a value stored in the
  database wins, then the variable, then the default") for the flag tier and the no-env-var setting. Update `:49-51`
  (invalid stored value) to say the maintenance daemon now reads settings and skips payload deletion, and
  that a malformed `retention_by_stream_tag` fails the settings load for every service. Also add `MICROMEGAS_RETENTION_DAYS`
  -> `retention_days` to the env-var table (`:20-29`), update the "five variables" count
  (`:33`), and mention `retention_by_stream_tag` as a setting with no env var. Also update `:37-47`: the
  resolution order gains the `--retention-days` flag tier for `retention_days` and the no-env-var case, Reset
  reverts to the env var, flag or default only where one exists, and the shadow-warning text notes that a stored
  `retention_days` shadowing `--retention-days` is not reported.
- `mkdocs/docs/when-to-use/index.md:16`: reword "every event keeps full resolution until the retention
  horizon, 90 days by default" so that query access to full resolution still ends at the default
  horizon (90 days) and only raw payload storage is adjustable per stream tag.
- `welcome/public/llms.txt:12`: same rewording of "raw events keep full resolution for the whole
  retention period".
- `tasks/backlog/1606_transactional_audit_write_plan.md`: the "Retention of the raw blocks" section
  becomes "set `retention_by_stream_tag` to `{"audit": "forever"}`", and the per-view table's
  "absent" row says `retention_days`. Also rewrite the manual step at `:566` so it no longer
  uses `--retention-days 0` (now rejected; store a short `retention_days` or a short tag override
  instead), and update `:492` (per-view `retention` overrides `MICROMEGAS_RETENTION_DAYS`) and
  `:116` (Current State cutoff) to name the `retention_days` setting.
- `tasks/backlog/1245_partition_blocks_by_insert_time/plan.md`: add a note at `:681-696` and
  `:780-781` that the partition-drop deletion replacing `delete_expired_blocks` must keep the
  blocks of streams whose tag override horizon is longer than the partition's age.
- `tasks/backlog/1245_partition_blocks_by_insert_time/rollback.md:128`: change "restore
  `delete_expired_blocks`" to restore the tag-aware `BlockDeletePass`-based passes.
- `CHANGELOG.md` (Unreleased): the feature (#1695) and data-lake schema v13 (GIN index on
  `streams.tags`). **Minor breaking change:** `delete_old_data` takes a `&PayloadRetention`,
  `delete_expired_blocks(_batch)` take a `&BlockDeletePass`, `daemon` takes an
  `Arc<SettingsSource>` instead of `retention_days`, `SettingSpec::env_var` is an `Option`, and
  `EnvFallbacks` / `DbOverrides` gain retention fields. `--retention-days` now only accepts
  `1..=36500`.

## Testing Strategy

Per `CONTRIBUTING.md` and the project rule on live-DB tests, all automated tests run without a
database.

- `rust/analytics/tests/payload_retention_tests.rs` (`block_delete_passes`, fixed `now`):
  - No overrides: one default pass with an empty exclude list.
  - `{audit: forever}`: one default pass excluding `audit`, and no `audit` pass.
  - `{debug: 3, trace: 3}`: one 3-day pass including both tags.
  - `{a: 3, b: 30, c: forever}`: the 3-day pass excludes `b` and `c`, the 30-day pass excludes
    `c`, and the default pass excludes all three. That is what makes the longest horizon win.
  - An override longer than the default and one shorter both get their own cutoff.
  - `default_cutoff` is `now - default_days`.
- `rust/public/tests/settings_tests.rs`:
  - `validate`: `Days` accepts 1 and 36500 and rejects 0, 36501, `"90"` and 1.5. `TagHorizons`
    accepts `{}` and `{"audit": "forever", "debug": 3}`, and rejects a non-object, an empty or
    untrimmed tag, `0`, `"never"` and `null`.
  - `from_rows` fills both new fields, and an invalid `retention_by_stream_tag` fails the whole
    parse.
  - `payload_retention()` resolution: DB `retention_days` beats env, env beats the default, and
    the overrides come from the DB or are empty.
  - `shadowed` never reports a spec with no env var.
  - Add `retention_days` (against `env.retention_days`) to the key list of
    `env_fallbacks_default_to_the_registry_defaults`, and assert that the `retention_by_stream_tag`
    default `{}` validates and parses via `parse_tag_horizons` to an empty map (it has no
    `EnvFallbacks` field).
- `rust/public/tests/read_policy_threading_tests.rs` already builds a DB-backed source over an
  unreachable lazy pool. Add one assertion that `payload_retention()` errors on that cold start. The existing
  `SettingsSource` is moved into `start_server` (`:414`), so the assertion builds its own
  `SettingsSource::new(..)` over the same unreachable lazy pool.
  The no-deletion-on-failure guarantee relies on that error.
- `rust/analytics-web-srv/tests/settings_tests.rs`: the GET entry builder emits
  `env_var: null` for `retention_by_stream_tag`, and a PUT with an invalid tag map returns 400
  before any DB access (lazy pool).
- Frontend (vitest): the `tag_horizons` editor submits `{"audit": "forever", "debug": 3}` from its
  rows, Forever disables the days input, duplicate or empty tags block Save, the `days` editor
  rejects 0, and a `null` `env_var` renders "no env fallback" and the "Default" pill.

## Manual Verification

The deletion SQL and the GIN index run against real Postgres and the object store. Per the
project rule, these get no automated live-DB test until a bug shows up in the wild. Deleting the
wrong blocks would be silent, so this check runs before merge.

1. `python3 local_test_env/ai_scripts/start_services_with_oidc.py`, plus `analytics-web-srv` as in
   the #1592 plan. Confirm `/tmp/ingestion.log` shows the migration reaching v13, and that
   `psql "$MICROMEGAS_SQL_CONNECTION_STRING" -c "\d streams"` lists `stream_tags` as a `gin` index.
2. Let the services run a few minutes so they ingest their own telemetry (streams tagged `log`,
   `metrics`, `cpu`).
3. On **Admin → Settings**, set `retention_by_stream_tag` to `{"log": "forever", "metrics": 120}`
   and leave `retention_days` unset (90). This comes before back-dating so that an HH:10 pass in
   between cannot apply the default horizon without the overrides.
4. Back-date everything: `UPDATE blocks SET insert_time = insert_time - interval '100 days';` and
   the same for `streams` and `processes`.
5. Note block counts per tag, restricted to back-dated rows (the live services keep writing
   new blocks to the same streams):
   `SELECT s.tags, count(*) FROM blocks b JOIN streams s USING (stream_id) WHERE b.insert_time < now() - interval '50 days' GROUP BY 1;`.
   This is the baseline.
6. Wait for the next HH:10 hourly pass (see `/tmp/daemon.log`). Rerun the step 5 query. Back-dated `log` and
   `metrics` blocks remain and back-dated `cpu` blocks are gone. Their objects under the local
   object store's `blobs/<process>/<stream>/` are gone too. Processes that still own a `log`
   stream remain.
7. Set `retention_by_stream_tag` to `{"metrics": 30}`, wait for the next pass, and confirm that
   the back-dated `log` and `metrics` blocks are now deleted.
8. Run `SET enable_seqscan = off;`, then `EXPLAIN` the override-pass query with `s.tags && ARRAY['metrics']` and confirm it uses
   `stream_tags`.
9. Store an invalid value directly
   (`UPDATE settings SET value = '{"x": 0}' WHERE key = 'retention_by_stream_tag';`), restart the
   maintenance daemon, and confirm the next pass logs the settings error, deletes nothing, and still materializes views.

## Open Questions

- None blocking. Possible later work, not needed for the audit case: per-process-property or
  per-audience retention, and a cutoff column if a high-volume stream ever needs a long override.
