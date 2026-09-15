# DDL-Defined Eagerly Materialized Views (Issue #835) Plan

## Overview

Let an admin define a new lakehouse **view set** at runtime with a SQL DDL statement executed over
FlightSQL, instead of editing `default_view_factory()` and redeploying. The definition is stored in
Postgres; a shared `ViewRegistry` rebuilds an `Arc<ViewFactory>` from it on a timer, so
`telemetry-maintenance-srv` starts eagerly materializing the view set's global instance and
`flight-sql-srv` starts answering queries against it, both without a restart.

The unit being defined is exactly what `SqlBatchView` already is — a definition (`extract_query`), a
freshness probe (`count_src_query`), and a composition (`merge_partitions_query`) — so this feature
adds no new materialization machinery. What it adds is persistence, a DDL front end, validation of
the parts that fail *silently* when wrong, and a reload seam. Per-process / per-stream instances of
a DDL-defined view set (`view_instance('...', process_id)`) stay out of scope; only the `'global'`
instance is created.

**`log_stats` becomes data.** It is a `SqlBatchView` whose only distinction from a DDL-defined view
is that its SQL lives in Rust, so the migration seeds it into `lakehouse_view_set_definitions` and
`default_view_factory` drops its construction step (`view_factory.rs:344-352`). This is the point of
the feature rather than a bonus: it makes the DDL path carry a view every deployment materializes on
every tick, instead of one only opt-in operators run, and the seeded definition is the validator's
calibration case — it must pass every check in §4 unmodified, and if it doesn't the check is wrong,
not the view.

## Current State

**The view model.** `ViewFactory` (`rust/analytics/src/lakehouse/view_factory.rs:250-295`) holds
`global_views: Vec<Arc<dyn View>>` (implicitly available as SQL tables) and `view_sets:
HashMap<String, Arc<dyn ViewMaker>>` (reachable only via `view_instance(...)`). It is `Clone` (a
shallow clone of a `Vec`/`HashMap` of `Arc`s) but has no interior mutability: every holder treats
`Arc<ViewFactory>` as frozen after construction.

**`SqlBatchView`** (`rust/analytics/src/lakehouse/sql_batch_view.rs:70-143`) is already the
SQL-defined view. `new()` is async and does real planning at construction: it builds a
`make_session_context` under `CallerContext::maintenance()` over the `view_factory` it is handed,
substitutes `Utc::now()` for `{begin}`/`{end}` in `extract_query`, plans it, and **takes the
resulting Arrow schema as the view's file schema** (`:111-116`). `register_table`
(`:311-334`) registers the `MaterializedView` as `__<name>__partitions` and then registers
`merge_partitions_query` with `{source}` → that name as the user-visible table. `log_stats_view.rs`
is the canonical instance.

**SQL-based vs block-based views.** Of the built-ins, exactly three are `SqlBatchView`s —
`processes` (`processes_view.rs:12-89`, group 2000), `streams` (`streams_view.rs:12-75`, group 2000)
and `log_stats` (`log_stats_view.rs:12-98`, group 3000). The rest are block-based: `blocks`
(`BlocksView`), `log_entries`/`measures` (`LogViewMaker`/`MetricsViewMaker`), and the five
instance-only sets, which decode binary payloads and have no SQL definition to store. Only
`log_stats` is in scope for seeding — see `## Decisions` for why `processes`/`streams` stay in code.

**Who builds the factory.** `default_view_factory` (`view_factory.rs:297-376`) builds the built-ins
in a clone-and-extend chain; `make_log_stats_view` is handed an `Arc<ViewFactory>` snapshot
containing everything its SQL reads. Startup sites: `flight_sql_server.rs:254-264` (moved into
`FlightSqlServiceImpl`), `telemetry-maintenance-srv/src/main.rs:32-54` and `monolith/src/main.rs:348-356`
(both reduce it immediately to `get_global_views_with_update_group(&factory)` and drop the factory).

**The daemon.** `daemon(lakehouse, views_to_update: Vec<Arc<dyn View>>, ...)`
(`rust/public/src/servers/maintenance.rs:408-493`) sorts the views by `update_group` once, wraps them
in `Views = Arc<Vec<Arc<dyn View>>>` (`:26`), and hands that same `Arc` to four of the five
`CronTask`s (`EveryDayTask`, `EveryHourTask`, `EveryMinuteTask`, `EverySecondTask` — `PgStatsTask`
holds no `views` field). The view
list is fixed for the process's lifetime. `materialize_all_views` (`:44-105`) walks the sorted list,
refetching the partition cache at each group boundary — the documented contract (`:59-70`) is that a
view reading another view must sit in a *strictly later* update group.

**Freshness and invalidation.** `verify_overlapping_partitions` (`batch_update.rs:23-100`) filters
existing partitions by exact `file_schema_hash` and compares the summed `source_data_hash` (an i64
row count) against the spec's. `SqlBatchView::get_file_schema_hash` (`:279-283`) is a `DefaultHasher`
over the Arrow schema **and nothing else** — so changing a `SqlBatchView`'s SQL without changing its
output schema leaves every existing partition valid and the daemon does nothing.

**Read authorization.** `OwnershipRewrite::predicate_for`
(`rust/analytics/src/lakehouse/ownership_rewrite.rs:366-434`) picks a per-scan audience predicate by
schema introspection: an `audience` field → direct column filter; else a `process_id` field →
semi-join against `__processes__partitions`; else two name-keyed arms; **else
`Err(DataFusionError::Plan("no audience rule defined for view set '...'"))`**. This rule is
constructed for every caller whose `ReadScope != All`.

**Admin gating.** `query.rs:204-246` registers the eight mutating lakehouse UDTFs/UDFs only when
`caller.is_admin`. `rust/analytics/tests/lakehouse_admin_gate_test.rs` is the offline (no-DB)
harness for that gate: a `connect_lazy` pool, an in-memory object store, `NullPartitionProvider`,
planning-only assertions.

**Reload precedent.** `QueryDenyList` (`rust/analytics/src/lakehouse/query_deny_list.rs`) already
implements exactly the shape this feature needs: a Postgres-backed store, an immutable compiled
snapshot behind `RwLock`, a `refresh()` that logs and meters its own failures, and
`spawn_refresh_task(shutdown)` (`:682-698`) wired in at `flight_sql_server.rs:406`.

**Migrations.** `LATEST_LAKEHOUSE_SCHEMA_VERSION = 9` (`migration.rs:8`); the ladder in
`execute_lakehouse_migration` (`:53-120`) applies one `upgrade_vN_to_vN+1` per step inside its own
transaction, each ending with `UPDATE lakehouse_migration SET version=N+1`. `upgrade_v8_to_v9`
(`:546-567`, creating `query_deny_list`) is the new-table template.

**There is no DDL path at all today.** `execute_query`
(`flight_sql_service_impl.rs:639-949`) hands the SQL straight to `ctx.sql(sql)`.

## Design

### 1. DDL surface

```sql
CREATE [OR REPLACE] MATERIALIZED VIEW <name>
WITH (
  update_group             = 4000,
  time_column              = 'time_bin',
  count_src_query          = 'SELECT sum(nb_objects) as count FROM blocks WHERE ... AND insert_time >= ''{begin}'' AND insert_time < ''{end}''',
  merge_partitions_query   = 'SELECT time_bin, target, sum(count) as count, audience FROM {source} GROUP BY time_bin, target, audience',
  source_partition_delta   = '1 day',
  merge_partition_delta    = '1 day',
  merge_sort_order         = 'time_bin, target'
)
AS
SELECT date_bin('1 minute', time) as time_bin, target, count(*) as count,
       arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
  FROM log_entries
 WHERE insert_time >= '{begin}' AND insert_time < '{end}'
 GROUP BY time_bin, target, audience
 ORDER BY time_bin, target;

DROP MATERIALIZED VIEW [IF EXISTS] <name>;
```

The **extract query is the statement body**, not a `WITH` option. `sqlparser`'s
`parse_create_view` (`sqlparser-0.62.0/src/parser/mod.rs:6517-6607`) requires `AS <query>` and
accepts a `WITH (...)` option list immediately before it, so this is the only shape of the issue's
3-query model that is real SQL. It also reads better: the body is the view, the options are knobs.
`parse_create_view` hands back a parsed `Query` AST, not source text, so `ViewDefinition.extract_query`
is populated by slicing the verbatim source text of the `AS` body out of the input SQL — never
`Query::to_string()`. This is what makes the definition hash (§3) sensitive to a purely cosmetic edit
(whitespace, a comment): a re-rendered AST would normalize both away, and it is also the text every
later materialization executes, so a stored `extract_query` must be what the author wrote, not a
reformatted equivalent.

Option semantics:

| option | required | maps to |
|---|---|---|
| `update_group` | yes | `SqlBatchView::new`'s `update_group`. Must be strictly greater than the group of every view set the definition reads (enforced, §4 check 7); the daemon's group ordering is the only dependency mechanism |
| `time_column` | yes | both `min_event_time_column` and `max_event_time_column` |
| `min_time_column` / `max_time_column` | no | override `time_column` individually |
| `count_src_query` | yes | `SqlBatchView::new`'s `count_src_query` |
| `merge_partitions_query` | yes | `SqlBatchView::new`'s `merge_partitions_query` |
| `source_partition_delta` | no, default `'1 day'` | `max_partition_delta_from_source`; parsed as a `TimeDelta` from a `<n> <unit>` string |
| `merge_partition_delta` | no, defaults to `source_partition_delta` | `max_partition_delta_from_merge` |
| `merge_sort_order` | no | comma-separated column list → `with_merge_sort_order` |

Anything else is a hard error — an unknown option is far more likely a typo than an extension point.

### 2. Persistence

Lakehouse migration **v9 → v10**, following `upgrade_v8_to_v9`:

```sql
CREATE TABLE lakehouse_view_set_definitions (
  view_set_name          VARCHAR(255) PRIMARY KEY,
  definition_sql         TEXT NOT NULL,
  extract_query          TEXT NOT NULL,
  count_src_query        TEXT NOT NULL,
  merge_partitions_query TEXT NOT NULL,
  update_group           INTEGER NOT NULL,
  view_options           JSONB NOT NULL DEFAULT '{}',
  definition_hash_enabled BOOLEAN NOT NULL DEFAULT true,
  created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by             TEXT
);
```

The same migration seeds `log_stats` with `definition_hash_enabled = false`:

```sql
INSERT INTO lakehouse_view_set_definitions (...) VALUES ('log_stats', ...)
ON CONFLICT (view_set_name) DO NOTHING;
```

Its text comes from `const`s in a new
`rust/analytics/src/lakehouse/builtin_view_definitions.rs` — moved out of
`log_stats_view.rs`, which the migration and the tests then share as one source of truth.
`ON CONFLICT DO NOTHING` keeps the migration idempotent.

The `upsert` behind `CREATE OR REPLACE` is an `ON CONFLICT (view_set_name) DO UPDATE` that sets
`updated_at = now()` and `updated_by` explicitly on the `DO UPDATE` branch — the column defaults only
fire on `INSERT`, and `reload()`'s digest is over `(view_set_name, updated_at)`, so an update that
left it unset would go live only on the replacing node while every other flight-sql replica and the
maintenance daemon kept serving and materializing the old definition.

`definition_hash_enabled` is the one flag, and it exists solely to keep the seeded row's
`file_schema_hash` byte-identical to the compiled view's (§3). A `CREATE OR REPLACE` of `log_stats`
sets it `true`, because at that point the definition genuinely differs from the shipped one and
invalidation is correct.

Seeded `log_stats` otherwise gets no special treatment: it can be replaced or dropped like any other
row, and a definition that fails to build is skipped with a `warn!` like any other. The consequence
of a skip is a clean `table not found` for `log_stats` queries — a visible error, not wrong data —
which is why it needs no fail-fast carve-out.

`view_options` holds the non-query options other than `update_group` (the time columns, the two
deltas, `merge_sort_order`); `update_group` is its own column because `reload()` sorts and §8
projects on it. `definition_sql` is the verbatim DDL text, kept for display and audit only — never
re-parsed.

Deliberately **absent** from the issue's proposed table:

- **`file_schema BYTEA`** — the schema is derived by planning `extract_query`, exactly as
  `SqlBatchView::new` already does. A stored copy is a second source of truth that drifts the
  moment a source view's schema changes underneath it.
- **`schema_version INTEGER`** — superseded by the definition hash below, which cannot be
  forgotten and cannot disagree with the SQL it describes.

### 3. Definition hash → `file_schema_hash`

`SqlBatchView::get_file_schema_hash` hashes only the inferred schema, so a `CREATE OR REPLACE` that
changes the SQL but not the output schema (a widened filter, a different source view, a fixed
`date_bin` interval) leaves stale partitions valid and silently serves the old answer forever.

Add an optional definition hash to `SqlBatchView`:

```rust
/// Mixed into `get_file_schema_hash` so a definition change with an unchanged output schema still
/// invalidates existing partitions. `None` for the built-in views, whose definitions only change
/// with a code change.
pub fn with_definition_hash(mut self, hash: u64) -> Self
```

```rust
fn get_file_schema_hash(&self) -> Vec<u8> {
    let mut hasher = DefaultHasher::new();
    self.schema.hash(&mut hasher);
    if let Some(h) = self.definition_hash {
        h.hash(&mut hasher);
    }
    hasher.finish().to_le_bytes().to_vec()
}
```

The `None` path must stay byte-identical to today's. The CI guard for that is a no-DB unit test
(§ Testing Strategy) pinning `get_file_schema_hash()` for a `definition_hash: None` `SqlBatchView` to
an exact byte value, built on the `lakehouse_admin_gate_test.rs` offline harness — not
`rust/analytics/tests/sql_view_test.rs:226`, whose matching assertion is `#[ignore]`d behind a live
`MICROMEGAS_SQL_CONNECTION_STRING` and so never runs in CI. A change to the `None` path would
invalidate every `SqlBatchView` partition in every existing deployment.

The DDL loader computes the hash over the normalized definition: `extract_query`,
`count_src_query`, `merge_partitions_query`, and the two time columns — the fields that determine
partition *content*. `update_group`, `source_partition_delta`/`merge_partition_delta`, and
`merge_sort_order` are deliberately excluded: `update_group` only orders `materialize_all_views`'s
scheduling (`maintenance.rs:59-70`) and does not touch what a partition contains; the two deltas only
set the width of *future* partitions, and `verify_overlapping_partitions` already tolerates mixed
widths (`batch_update.rs:23-100`); and `merge_sort_order` only affects the sort guarantee recorded in
partition metadata (`Partition::certifies_sort_order`, `partition.rs`), which the schema
hash plays no part in. It is derived, never stored, so there is nothing to keep in sync.

Consequence, which must be documented rather than engineered around: a redefinition **resets the
view's materialized history**. The daemon only fills forward (2 days / 2 hours / 2 minutes back from
`now`); backfilling the rest is a manual
`SELECT * FROM materialize_partitions('<name>', <begin>, <end>, <delta_secs>)`.

### 4. Validation at `CREATE` time

Every check runs before the row is written, in one `validate_view_definition` function shared by the
DDL executor and the registry loader — so a definition that the loader would skip can never be
accepted in the first place.

Constructing the `SqlBatchView` *is* most of the validation: it plans the extract query (catching
syntax errors, unknown tables, unknown columns) and yields the schema. On top of that:

1. **Name.** Matches `^[a-z_][a-z0-9_]{0,254}$` and does not start with `__`; not a code-driven view
   set (`get_global_view` / `get_view_sets` on the base factory, which after this change means
   `blocks`, `processes`, `streams`, `log_entries`, `measures` and the five instance-only sets — but
   *not* `log_stats`, so the seeded row stays replaceable); not `source`. The name is interpolated
   into `__<name>__partitions` and into table registrations, so this is a correctness *and* an
   injection guard: without the `__` exclusion, a
   name like `__log_stats__partitions` would collide with that view's own `__<name>__partitions`
   internal registration and hard-error `make_session_context` for every query in the deployment, not
   just ones touching that view. `source` is excluded for the same reason: `SqlBatchView::new`
   registers every view's merge query under the literal name `source` (the `{source}` substitution,
   `sql_batch_view.rs:298`), and DataFusion errors on a duplicate registration, so a view set named
   `source` would break every other view's merge on every tick.
2. **Audience reachability.** The inferred schema must contain an `audience` field or a `process_id`
   field. Without one, `OwnershipRewrite::predicate_for` returns `Err` for every non-admin caller
   and the view set is unqueryable in any deployment with auth on. The error message names the
   requirement and points at `max(audience)`-in-`GROUP BY` as the fix. This mirrors the existing
   branch table exactly; `ownership_rewrite.rs` needs no edit.
3. **Merge query plans, and agrees.** Register an empty table carrying the inferred schema under the
   substituted `{source}` name, plan `merge_partitions_query`, and require its output schema to
   equal the extract query's (field names, types, and order). Today a bad merge query is not
   detected until the daemon's first merge, and a *mismatched* one is not detected at all — yet the
   merge query is the read path for any query spanning more than one partition
   (`sql_batch_view.rs:311-334`), so a disagreement means the table a user sees does not match the
   partitions it is built from.
4. **Count query shape.** `fetch_sql_partition_spec` (`sql_partition_spec.rs:196-203`) requires one
   batch, one row, one `Int64` column literally named `count`. Check the planned schema for that
   single `count: Int64` column and reject otherwise, rather than failing on the daemon's first tick.
5. **Placeholders.** `count_src_query` must contain `{begin}` and `{end}`;
   `merge_partitions_query` must contain `{source}`. The probe without a range returns a constant,
   so freshness is never detected — stale forever. The `{source}` requirement is mechanical: the
   merge query is unrunnable without it.
6. **No volatile or stable functions.** Walk the planned extract query's `LogicalPlan` expressions
   and reject any `ScalarUDF` whose `signature().volatility` is not `Immutable` (`now()`,
   `random()`, `current_timestamp`). Their value is frozen into a partition at materialization
   time and then served to every later reader — wrong data, no error, indefinitely.

7. **`update_group` strictly after every view set the definition reads.** Walk the planned extract
   and count-source `LogicalPlan`s for every `TableScan`, resolving each one's `table_name` against
   the factory the definition was built for — recursing into `TableSource::get_logical_plan()` when
   the scan is a `ViewTable` (which is how `SqlBatchView::register_table` exposes the user-visible
   name; a downcast to `MaterializedView` alone misses it and would only catch `__<name>__partitions`
   scans) — and collecting each matched view's `get_update_group()`. A `TableScan` reached through
   that recursion names the underlying `__<name>__partitions` table, not the view set, so it is
   resolved by stripping the `__..__partitions` affix or by downcasting its `TableSource` to
   `MaterializedView` and reading `get_view().get_view_set_name()`; a scan matching neither form is
   ignored. Require `update_group >` the maximum found. Since DDL views may read other DDL views,
   this is what keeps that safe: `materialize_all_views`'s group ordering (`maintenance.rs:59-70`) is
   the *only* dependency mechanism, and a definition in the wrong group reads its source's previous-tick state forever —
   stale numbers, no error. Enforcing it turns the documented obligation into a check, and it costs
   nothing: the plan is already built by step 3.
8. **`merge_sort_order`, if given, matches the extract query's actual ordering.** Build the extract
   query's physical plan and run the existing `assert_single_partition`/`assert_ordering_satisfied`
   helpers (`partitioned_execution_plan.rs:217-265`; called from `sql_partition_spec.rs:79-115`)
   against it at `CREATE` time. `with_merge_sort_order`
   itself only checks the columns exist in the schema; the real enforcement is at write time, where a
   mismatched top-level `ORDER BY` makes `execute_extract_query` error on every daemon tick with an
   empty table in the meantime. Running the same assertions at `CREATE` turns that into a rejection
   up front.

Checks 2, 3, 5, 6, 7 and 8 are the ones that earn their keep: each covers a failure that produces wrong
or stale *numbers* rather than an error. What stays an **unchecked author obligation**, documented
and not enforced: `extract_query` need not carry `{begin}`/`{end}` (the extract session context is
already range-scoped by `filter_insert_range` on the partition provider,
`sql_batch_view.rs:238`), but because that filter matches by *overlap*, a partition wider than the
bucket being materialized is scanned whole — so the query must either be idempotent under seeing
extra rows (`GROUP BY` with `first_value`/`max`) or filter the range explicitly (as `log_stats`
must, since `count(*)` would double-count); that any range predicate is on `insert_time` (not event
time); and that the merge query's aggregates are composable over already-aggregated rows
(`sum(count)`, never `count(*)`, no bare `avg`). All three are exactly the contract
`with_merge_sort_order`'s doc comment (`sql_batch_view.rs:145-163`) already spells out for
hand-written views, and none is decidable from a logical plan.

The insert-time half of that obligation does not apply when `extract_query` reads another
materialized view rather than a raw source table: a materialized view's schema is whatever its own
`extract_query` projects, and carries no `insert_time` column at all — only `time_column`'s
event-time column is guaranteed to exist. A definition `b` reading definition `a` therefore filters
on `a`'s event-time column instead; the consequence, also documented rather than enforced, is that a
row arriving late into `a`'s own partitions after `b` has already covered that time range is not
picked up by `b`.

### 4b. Dependents survive a `DROP` or a `REPLACE`

Cross-DDL references make one new failure reachable: dropping `a`, or replacing it with a definition
that no longer projects a column `b` reads, leaves `b` unable to plan. `b` would then be skipped by
the registry loader (§7) with only a `warn!`, so a user's table would quietly vanish.

The mutation is therefore validated against the *resulting* definition set, not just its own row:
`execute_view_ddl`'s transaction first calls `acquire_lock(&mut tx, <dedicated key>)`
(`remote_data_lake.rs:13-19`, `pg_advisory_xact_lock`), serializing every view-definition mutation
cluster-wide so two replicas can't each validate against a row set that omits the other's in-flight
write. After the upsert/delete, it calls `ViewDefinitionStore`'s transaction-scoped `list_tx(&mut tx)`
(§7) to re-read the post-mutation rows and run them through `ViewRegistry`'s rows-in build seam (§7),
refusing the statement if any **other** definition that built before now fails to build. The error
names the broken dependents. This is exact rather than textual — it uses the real planner, so it
catches a removed column as readily as a removed view set — and it reuses the loader wholesale.

No `CASCADE` in v1: "drop it and everything downstream" is a second, riskier statement, and the
refusal already tells the admin exactly which views to drop first.

### 5. DDL execution path

New module `rust/public/src/servers/view_ddl.rs`:

```rust
pub enum ViewDdl {
    Create { name: String, or_replace: bool, definition: ViewDefinition, sql: String },
    Drop   { name: String, if_exists: bool },
}

/// `Ok(None)` when `sql` is not view DDL -- the overwhelmingly common case, decided by a cheap
/// keyword peek before any full parse.
pub fn parse_view_ddl(sql: &str) -> Result<Option<ViewDdl>, DdlError>;
```

In `FlightSqlServiceImpl::execute_query`, inserted between the resolved `caller` and
`make_session_context`:

```rust
if let Some(ddl) = parse_view_ddl(sql).map_err(|e| audit_state.fail(client_input_error!("...", e)))? {
    return self.execute_view_ddl(ddl, &caller, audit_state).await;
}
```

`execute_view_ddl`:

1. `if !caller.is_admin { return Err(Status::permission_denied(...)) }` — the same gate as the eight
   mutating lakehouse functions. It has to be an explicit check rather than a registration gate,
   because this path never builds a caller session context. It is also load-bearing beyond the usual
   reason: a DDL view's queries are planned and materialized under `CallerContext::maintenance()`,
   i.e. `ReadScope::All`, so an author sees every audience regardless of their own read scope.
2. Open the transaction and call `acquire_lock(&mut tx, <dedicated key>)` first (§4b), serializing
   steps 3-5 cluster-wide.
3. `CREATE`: validate (§4) against the factory the loader would build for it — the base plus every
   definition in a lower `update_group`, so a definition reading another DDL view validates against
   the real thing. Reject an existing name unless `or_replace`; on a replace whose definition hash
   changed, retire the old partitions (§6) and `upsert` the row via `registry.store()`'s
   transaction-scoped `upsert_tx(&mut tx, ...)`, all in the same transaction.
4. `DROP`: reject a built-in name; `delete` the row via `registry.store()`'s transaction-scoped
   `delete_tx(&mut tx, ...)` and retire the partitions in one transaction; honour `IF EXISTS`.
5. Either mutation then re-reads the post-mutation row set via `registry.store()`'s transaction-scoped
   `list_tx(&mut tx)` (§4b) and calls `registry.build_from_rows(&rows)` (§7), inside the same
   transaction, rolling back if it broke a dependent. `registry.store()` is how `execute_view_ddl`
   reaches the definition store to run these calls on its own open transaction, alongside
   `retire_partitions`.
6. `registry.reload()` inline, so the statement's own connection can query the new view set
   immediately instead of waiting out the interval.
7. Return a one-row, two-column result (`view_set_name: Utf8`, `status: Utf8` ∈
   `created | replaced | dropped | not_found`) so `client.query("CREATE ...")` yields a DataFrame
   and the FlightSQL stream shape is unchanged.

`do_action_create_prepared_statement` (`flight_sql_service_impl.rs:1332-1375`) rejects a statement
that `parse_view_ddl` matches, with a message saying DDL must be executed directly. Nothing in the
repo prepares DDL, and planning it there would fail with an opaque DataFusion error.

### 6. Retiring a view set's partitions

`retire_partitions` (`write_partition.rs:183-383`) is keyed on `(view_set_name, view_instance_id)`
and is deliberately hash-agnostic, but needs an explicit insert-time range. Resolve the exact range
first and pass it:

```sql
SELECT min(begin_insert_time), max(end_insert_time)
  FROM lakehouse_partitions WHERE view_set_name = $1 AND view_instance_id = 'global';
```

`NULL` (no partitions) skips the call. This reuses the existing containment path — files land in
`temporary_files` and the hourly `delete_expired_temporary_files` collects them — and needs no new
"retire an entire view set" primitive.

### 7. `ViewRegistry`

New `rust/analytics/src/lakehouse/view_registry.rs`, shaped after `QueryDenyList`:

```rust
pub struct ViewRegistry {
    base: Arc<ViewFactory>,                 // the code-driven views; never reloaded
    store: Arc<dyn ViewDefinitionStore>,    // Postgres in production, a fake in tests
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    session_configurator: Arc<dyn SessionConfigurator>, // both maintenance entry points and
                                             // flight-sql resolve the same `StaticTablesConfigurator`
    current: RwLock<Arc<ViewFactory>>,      // base + DDL-defined global views
    loaded_digest: RwLock<Option<u64>>,     // hash of the loaded (name, updated_at) tuples
    failed_view_sets: RwLock<Vec<String>>,  // names skipped by the last build
    reload_mutex: tokio::sync::Mutex<()>,   // serializes reload against DDL-triggered reload
}

impl ViewRegistry {
    pub fn new(base: Arc<ViewFactory>, store: Arc<dyn ViewDefinitionStore>, ...) -> Self;
    pub fn base(&self) -> Arc<ViewFactory>;
    pub fn current(&self) -> Arc<ViewFactory>;
    /// The definition store, so `execute_view_ddl` (§5) can run its own `list_tx`/`upsert_tx`/
    /// `delete_tx` on it alongside `retire_partitions`, in the same transaction as the mutation it is
    /// executing.
    pub fn store(&self) -> Arc<dyn ViewDefinitionStore>;
    pub async fn reload(&self) -> Result<()>;
    /// Thin wrapper over `build_factory` supplying `self`'s `base`/`runtime`/`lake`/
    /// `session_configurator`, so a caller holding `Arc<ViewRegistry>` (§5 step 5) doesn't need those
    /// three pieces separately.
    pub async fn build_from_rows(&self, rows: &[ViewDefinitionRow]) -> Result<(Arc<ViewFactory>, Vec<String>)>;
    pub fn spawn_refresh_task(self: Arc<Self>, shutdown: impl Future<Output = ()> + Send + 'static);
}

/// Builds a factory from an explicit row set instead of `store.list()`, so a caller already holding
/// rows inside an open transaction (§4b, §5 step 5) can validate against them without a second,
/// pool-backed read that would not see its own uncommitted write. `reload()` calls this too, after
/// its own `store.list()`. `async` because building each row's `SqlBatchView` (`SqlBatchView::new`)
/// is itself `async` and needs `runtime`, `lake` and `session_configurator` to plan the extract
/// query.
async fn build_factory(
    base: &Arc<ViewFactory>,
    rows: &[ViewDefinitionRow],
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<(Arc<ViewFactory>, Vec<String>)>;
```

`reload()`:

1. `store.list()` → rows ordered by `(update_group, view_set_name)`.
2. Hash the `(view_set_name, updated_at)` tuples; if unchanged from `loaded_digest` **and** the
   previous build's failed set was empty, return — the steady-state reload is one `SELECT` and no planning,
   which is what makes a 60 s interval affordable. A digest match after a failed build does *not*
   short-circuit, so a row that failed for a transient reason (a source view momentarily unplannable
   mid-rollout, an object-store hiccup) is retried every tick rather than stuck until its row next
   changes or the process restarts.
3. `build_factory(&self.base, &rows, self.runtime.clone(), self.lake.clone(),
   self.session_configurator.clone()).await`: `let mut factory = (*self.base).clone();` then, per row in
   order, build the `SqlBatchView` against `Arc::new(factory.clone())` and
   `factory.add_global_view(...)`. Each definition therefore sees the built-ins plus every DDL view
   with a lower `update_group` — the same incremental clone-and-extend chain `default_view_factory`
   uses, and the same ordering rule `materialize_all_views` already documents. A row whose name already
   resolves via `base.get_global_view(...)` is skipped with a `warn!` instead of being built: until
   step 16 removes `log_stats`'s compiled construction from `default_view_factory`, the seeded
   `log_stats` row would otherwise collide with the base factory's own `log_stats`, and
   `ViewFactory::add_global_view`/`make_session_context`'s registration has no overwrite semantics —
   a duplicate global-view name hard-errors every query, not just ones touching that view.
4. A row that fails to build is **skipped**, not fatal: `warn!` plus
   `imetric!("view_definition_load_failure", "count", tags, 1)` tagged with the view set name, and
   `build_factory` collects its name into the returned failed-set. One broken definition must not take
   out every other view set, or the whole lakehouse.
5. Swap `current`, update `loaded_digest` and `failed_view_sets`.

Reload interval: `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`, default 60 s. The digest
short-circuit keeps the steady-state cost at one `SELECT` whenever the last build was fully healthy,
and `execute_view_ddl` reloads inline on its own node, so the interval only bounds how long a
*different* node lags a definition change (or a retry of a transiently-failed one).

**Consumers.**

- `FlightSqlServiceImpl` holds `Arc<ViewRegistry>` in place of `Arc<ViewFactory>`; `execute_query`,
  `do_get_tables`, and `do_action_create_prepared_statement` (which builds its own
  `make_session_context`) each call `registry.current()` — not `base()`, or a prepared statement over
  a DDL-defined view set would fail to plan even though direct queries succeed. Every downstream
  signature (`make_session_context`, the UDTFs, `MaterializedView`) keeps taking `Arc<ViewFactory>`
  and is handed the snapshot — no churn there. `flight_sql_server.rs` builds the registry and calls
  `spawn_refresh_task(fanout.subscribe())` next to the existing `query_denials` one (`:406`).
- `maintenance.rs`: `Views` (only its use on the four view-carrying `CronTask` structs) becomes
  `Arc<ViewRegistry>`;
  each `run()` computes `get_global_views_with_update_group(&registry.current())`, sorts by
  `update_group`, and passes the resulting `Arc<Vec<Arc<dyn View>>>` to `materialize_all_views`, whose
  `views: Arc<Vec<Arc<dyn View>>>` parameter stays spelled out (not the `Views` alias) and unchanged.
  `daemon()` takes
  `Arc<ViewRegistry>` instead of `Vec<Arc<dyn View>>` and spawns the refresh task itself. The sort
  moves out of `daemon` into a helper both the daemon and the tasks call. `materialize_all_views`'s
  `views.first().unwrap()` (`:51`) gains an empty-list early return, now that the list is computed
  per tick rather than once at startup.

```
   Postgres: lakehouse_view_set_definitions
                     |
                     | reload() every MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS
                     v
              ViewRegistry  (base ViewFactory + N SqlBatchViews)
                /                            \
     current()  |                             | current()
                v                             v
     flight-sql-srv                 telemetry-maintenance-srv
     make_session_context           materialize_all_views (per cron tick)
     (per query)
```

### 8. Introspection

DDL-defined view sets appear in `list_view_sets()` for free (it walks
`ViewFactory::get_global_views`). Add one read-only, admin-gated UDTF
`list_view_definitions()` over `lakehouse_view_set_definitions` — `view_set_name`,
`definition_sql`, `update_group`, `updated_at`, `updated_by` — so an admin can see a definition that
exists on disk but failed to load (present here, absent from `list_view_sets()`).

## Implementation Steps

### Milestone 1 — persistence, definition hash, validation, DDL

1. New `rust/analytics/src/lakehouse/builtin_view_definitions.rs` — `log_stats`'s three queries and
   options as `const`s, lifted verbatim out of `log_stats_view.rs`; consumed by the migration's seed
   and by the hash-parity test. Registered in `rust/analytics/src/lakehouse/mod.rs`. First because
   step 2's migration seed needs these `const`s to exist.
2. `rust/analytics/src/lakehouse/migration.rs` — bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to `10`,
   append the `9 == current_version` block, add `upgrade_v9_to_v10` creating
   `lakehouse_view_set_definitions`, seeding `log_stats` from
   `builtin_view_definitions`, and ending with `UPDATE lakehouse_migration SET version=10`.
3. `rust/analytics/src/lakehouse/sql_batch_view.rs` — add the `definition_hash: Option<u64>` field,
   `with_definition_hash`, and the `get_file_schema_hash` change; confirm the `None` path is
   byte-identical.
4. New `rust/analytics/src/lakehouse/view_definition.rs` — `ViewDefinition` (the normalized option
   set + three queries), its stable hash, `parse_time_delta`, the name charset check, and
   `build_sql_batch_view(&ViewDefinition, Arc<ViewFactory>, ...) -> Result<SqlBatchView>`.
5. Same module — `validate_view_definition`: §4 checks 2–8, on top of the `SqlBatchView` built in
   step 4. Check 7 needs the referenced view sets' `update_group`s, so it takes the factory the
   definition was built against.
6. New `rust/analytics/src/lakehouse/view_definition_store.rs` — the `ViewDefinitionStore` trait
   (pool-backed `list`, for `reload()`'s periodic use, plus the transaction-scoped
   `list_tx`/`upsert_tx`/`delete_tx`, each taking a `&mut sqlx::Transaction` — so the DDL path (§4b,
   §5), including its existence check behind `or_replace`, runs entirely on its own open transaction
   alongside `retire_partitions`), its Postgres impl, and `partition_insert_range(view_set_name)` for
   §6.
7. New `rust/analytics/src/lakehouse/view_registry.rs` — `ViewRegistry`, the shared `build_factory`
   rows-in seam, `reload` (ordered incremental build, skip-on-failure, digest short-circuit that also
   bypasses on a prior failure), `base()`/`current()`, `spawn_refresh_task`, and the
   `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` knob (default 60). Lands in this milestone rather
   than the next because the DDL path itself needs `base()`, the ordered build (step 11's validation
   against lower-group definitions), and §4b's post-mutation rebuild via `build_factory`.
8. New `rust/public/src/servers/view_ddl.rs` — `parse_view_ddl` over
   `datafusion::sql::sqlparser`, mapping `Statement::CreateView`/`Statement::Drop` to `ViewDdl`;
   option extraction and its error type. `extract_query` is sliced as verbatim source text from the
   `AS` body's span in the input SQL, not rendered from the parsed `Query` AST.
9. `rust/public/src/servers/flight_sql_service_impl.rs` — `view_factory: Arc<ViewFactory>` field
   becomes `view_registry: Arc<ViewRegistry>`; `execute_query`, `do_get_tables`, and
   `do_action_create_prepared_statement` call `current()`. This breaks
   `rust/public/tests/read_policy_threading_tests.rs`'s `FlightSqlServiceImpl::new` call, updated to
   construct a `ViewRegistry` over its fixture factory via the fake `ViewDefinitionStore` the Testing
   Strategy introduces. Moved ahead of step 11 because `execute_view_ddl` (step 11) is a method on
   `FlightSqlServiceImpl` that calls `registry.reload()`/`build_from_rows()` and so needs the field to
   already exist.
10. `rust/public/src/servers/flight_sql_server.rs` — construct the registry, call
    `spawn_refresh_task(fanout.subscribe())`; `ViewFactoryFn` keeps producing the *base* factory. Moved
    ahead of step 11 for the same reason as step 9: `execute_view_ddl` needs a constructed registry to
    call.
11. `rust/public/src/servers/flight_sql_service_impl.rs` — add the `parse_view_ddl` branch, inserted
    between the resolved `caller` and `make_session_context`, and `execute_view_ddl` (admin gate,
    validate, upsert/delete + retire, §4b rebuild-or-rollback, inline reload, one-row answer).
    Reject DDL in `do_action_create_prepared_statement`.
12. `rust/analytics/src/lakehouse/mod.rs` / `rust/public/src/servers/mod.rs` — register the remaining
    new modules with their one-line doc comments.

### Milestone 2 — daemon pickup

13. `rust/public/src/servers/maintenance.rs` — `Views` → `Arc<ViewRegistry>` on the four view-carrying
    task structs, per-tick view resolution + sort helper, empty-list early return in
    `materialize_all_views`, `daemon`'s signature change and its `spawn_refresh_task` call.
14. `rust/telemetry-maintenance-srv/src/main.rs` and `rust/monolith/src/main.rs` — build the
    registry from `default_view_factory` and hand it to `daemon`, resolving the same
    `StaticTablesConfigurator::from_env("MICROMEGAS_STATIC_TABLES_URL", ...)` the FlightSQL builder
    uses (`flight_sql_server.rs:275-283`) as `ViewRegistry::new`'s `session_configurator`, instead of
    a no-op one — a DDL view reading a static table must build the same way in both services.

### Milestone 3 — introspection and `log_stats` cutover

15. New `rust/analytics/src/lakehouse/list_view_definitions_table_function.rs`, registered in
    `query.rs`'s `if lakehouse_admin` block.
16. `rust/analytics/src/lakehouse/view_factory.rs` — drop the `log_stats` construction step
    (`:344-352`); `default_view_factory` now returns the base. `log_stats_view.rs`'s
    `make_log_stats_view` is kept but reduced to building from the `const`s, so the hash-parity test
    has something to compare against. `rust/analytics/tests/audience_mismatch_skip_db_test.rs` builds
    its `log_stats` view via `make_log_stats_view` and `add_global_view`s it onto its own clone of
    `default_view_factory`, instead of pulling `log_stats` from `default_view_factory` directly — the
    test both looks the view up via `view_factory.get_global_view("log_stats")` and runs `SELECT ...
    FROM log_stats ...` through that same factory, so both need a factory that still carries it.
    Deferred to the end of this milestone (and behind the daemon's Milestone 2 pickup) because the
    daemon (step 13) and `FlightSqlServiceImpl` (step 9) must already be reading `registry.current()`
    before `log_stats` is dropped from the base factory, or the view goes unmaterialized and
    unqueryable in between.
    Until this step runs, the base factory and the seeded row both carry a `log_stats`; §7 step 3's
    name-collision skip is what keeps every query from hard-erroring during that window, with the
    seeded row inert (skipped) until this step removes the compiled one.
    `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`'s
    `real_view_factory_covers_every_registered_view_set` derives its inventory from
    `default_view_factory().get_global_views()`; give it the same `make_log_stats_view` +
    `add_global_view` treatment onto its own factory clone so it keeps covering `log_stats`'s
    audience branch instead of silently losing that coverage.

## Files to Modify

Created:
- `rust/analytics/src/lakehouse/view_definition.rs`
- `rust/analytics/src/lakehouse/builtin_view_definitions.rs`
- `rust/analytics/src/lakehouse/view_definition_store.rs`
- `rust/analytics/src/lakehouse/view_registry.rs`
- `rust/analytics/src/lakehouse/list_view_definitions_table_function.rs`
- `rust/public/src/servers/view_ddl.rs`
- `rust/public/tests/view_ddl_parse_tests.rs`
- `rust/analytics/tests/view_definition_validation_tests.rs`, `view_registry_tests.rs`
- `python/micromegas/tests/test_ddl_materialized_view.py`
- `mkdocs/docs/admin/materialized-views.md`

Modified:
- `rust/analytics/src/lakehouse/migration.rs`, `sql_batch_view.rs`, `query.rs`, `mod.rs`,
  `view_factory.rs`, `log_stats_view.rs`
- `rust/analytics/tests/audience_mismatch_skip_db_test.rs`,
  `ownership_rewrite_public_view_set_tests.rs`
- `rust/public/tests/read_policy_threading_tests.rs`
- `rust/public/src/servers/maintenance.rs`, `flight_sql_service_impl.rs`, `flight_sql_server.rs`, `mod.rs`
- `rust/telemetry-maintenance-srv/src/main.rs`, `rust/monolith/src/main.rs`
- `mkdocs/docs/admin/functions-reference.md`, `maintenance.md`, `flight-sql.md`, `authorization.md`
- `mkdocs/docs/query-guide/schema-reference.md`, `mkdocs/mkdocs.yml`
- `CHANGELOG.md`

## Trade-offs

**DDL interception vs. a `create_materialized_view(...)` UDTF.** A UDTF would need no parser work
and would inherit the existing `is_admin` registration gate for free. It was rejected because the
issue asks for DDL and because a UDTF call cannot carry a multi-line SQL body without quote-escaping
it — the exact ergonomic problem the DDL form exists to avoid. The cost is a hand-rolled parse step
in front of `ctx.sql`.

**A shared `ViewRegistry` vs. separate reload paths per service.** The issue describes the daemon
pickup and the flight-sql reload as two mechanisms. They are the same mechanism with two consumers,
and building one seam means a definition can never be live in one service and not the other. It does
force `daemon`'s signature to change and the `Views` alias to be reworked; that is cheap, and the
Rust API is explicitly not a stability surface.

**Requiring `audience`-or-`process_id` vs. adding a DDL-view arm to `OwnershipRewrite`.** A new arm
would mean a name-keyed special case in a module whose entire design is to be schema-keyed and to
`Err` rather than silently leave a scan unfiltered. Pushing the requirement to `CREATE` time keeps
`ownership_rewrite.rs` untouched and turns a plan-time error every non-admin caller would hit into a
single error the author sees once.

**No per-instance (`view_instance`) support.** A DDL-defined view set gets only its `'global'`
instance. JIT instances need a `ViewMaker` and an id-scoped rewrite of the definition's predicates,
which is a second design; the issue already scopes it out.

## Decisions

- Validation rejects a definition whose merge-query output schema differs from its extract-query
  output schema, rather than warning — a disagreement means the user-visible table and the
  partitions backing it have different shapes, with no error at query time.
- `CREATE OR REPLACE` with a changed definition hash retires the old partitions immediately instead
  of leaving them for retention. They are already unreachable (the query-side partition provider
  filters on `file_schema_hash`), so keeping them buys no rollback, only storage.
- A definition that fails to load is skipped with a `warn!` and a metric; the registry still swaps
  in every definition that did load. One bad row must not take the lakehouse down.
- The insert-time-vs-event-time obligation and the merge-aggregate composability obligation are
  documented author contracts, not enforced checks — neither is decidable from a logical plan.
  Accepted risk, and the same one hand-written `SqlBatchView`s already carry.
- `MICROMEGAS_PUBLIC_VIEW_SETS` can name a DDL-defined view set, which disables its audience filter
  entirely. That is the existing operator knob behaving as designed; no extra guard.
- `update_group` is a **required** option with no default. There is no dependency inference here —
  the author states the order, and a definition reading another materialized view has to place
  itself after it. A default would be silently wrong for exactly that case.
- A DDL-defined view set **may read another one**, as a first-class use case; enforced by §4 check 7
  (`update_group` ordering) and §4b (dependents block a `DROP`/`REPLACE`).
- No `CASCADE` on `DROP` in v1.
- `log_stats` is seeded into `lakehouse_view_set_definitions` and removed from
  `default_view_factory`; `blocks`, `processes`, `streams`, `log_entries` and `measures` stay
  code-driven — seeding `processes`/`streams` too would put audience resolution itself behind a
  database row, so a failed load or an operator `DROP` would break every non-admin query, unlike
  `log_stats`, whose worst failure is a clean `table not found`.
- The definition hash (§3) is derived from the definition text rather than a stored
  `schema_version` column, so a purely cosmetic edit (whitespace, a comment) also invalidates
  existing partitions — accepted, since re-materialization is the daemon's normal steady-state work.
- The seeded `log_stats` row carries `definition_hash_enabled = false` so its `file_schema_hash`
  stays byte-identical and existing partitions survive the upgrade. It gets no other special
  casing — droppable, replaceable, and skipped-with-a-warning on a load failure like any other row.
- The `extract_query` is **not** required to contain `{begin}`/`{end}`. `processes` and `streams`
  carry no such predicate today and are correct, because `make_batch_partition_spec` scopes the
  scan through the partition provider (`sql_batch_view.rs:238`). Only `count_src_query`'s
  placeholders are enforced. The residual idempotence obligation is documented, not checked.

## Documentation

- **New** `mkdocs/docs/admin/materialized-views.md` — the DDL reference (both statements, every
  option), the three-part model, the author obligations (insert-time filtering — or, for a definition
  reading another materialized view, filtering on that view's event-time column instead, since a
  materialized view's schema carries no `insert_time` — `audience` in the projection and the
  `GROUP BY`, composable merge aggregates, `update_group` ordering), the
  backfill-is-manual fact and the `materialize_partitions` recipe, and the redefinition/`DROP`
  lifecycle. Added to `mkdocs/mkdocs.yml`'s nav.
- `mkdocs/docs/admin/functions-reference.md` — `list_view_definitions()`, and a pointer to the page
  above from the admin-function list.
- `mkdocs/docs/admin/maintenance.md` — `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` in the env-var
  table, and that the daemon now picks up view sets without a restart.
- `mkdocs/docs/admin/flight-sql.md` — the same env var, and that DDL is admin-gated.
- `mkdocs/docs/admin/authorization.md` — a sentence that DDL-defined view sets must carry `audience`
  or `process_id` and are filtered by the same two `OwnershipRewrite` branches as the code-driven
  views already listed there.
- `mkdocs/docs/query-guide/schema-reference.md` — one paragraph saying `list_view_sets()` includes
  DDL-defined view sets and that their schemas are deployment-specific, plus a note that
  `log_stats` is now a seeded definition an operator may extend or replace (its documented schema
  is the shipped default, not a guarantee).
- `view_factory.rs`'s module rustdoc (`:1-64`) — keep the `## log_stats` schema table but note it is
  now a seeded definition, not one `default_view_factory` builds, matching the `schema-reference.md`
  note.
- `CHANGELOG.md` — one entry, covering the v10 lakehouse migration and that `log_stats` is now a
  seeded definition rather than a compiled view (identical SQL surface and identical
  `file_schema_hash`, so no rebuild and no dashboard change), with the **Minor breaking change**
  clause for `daemon`'s signature, `FlightSqlServiceImpl::new`'s `view_factory` → `view_registry`
  parameter, `Views`, and `default_view_factory` no longer returning `log_stats`.

## Testing Strategy

Everything below is a no-DB unit test unless stated. The offline harness from
`rust/analytics/tests/lakehouse_admin_gate_test.rs` (lazy pool, in-memory object store,
`NullPartitionProvider`) supports planning-only assertions without touching Postgres, and
`ViewDefinitionStore` is a trait so the registry can be driven from canned rows.

**`rust/public/tests/view_ddl_parse_tests.rs`** — in the `micromegas` (public) crate, since
`parse_view_ddl` and `authorize_view_ddl` live there. `parse_view_ddl` over: each valid form;
`CREATE` without `OR REPLACE`; `DROP` with and without `IF EXISTS`; a plain `SELECT` and a
non-materialized `CREATE VIEW` both returning `Ok(None)`; a missing required option, an unknown
option, a malformed `source_partition_delta`, and a bad `merge_sort_order`, each a named error; a
name failing the charset check; option values containing escaped quotes and newlines.

**`view_definition_validation_tests.rs`** — against a fixture factory carrying a fake source view
with a fixed schema: a definition whose schema has neither `audience` nor `process_id` is rejected;
one with `audience` is accepted; one with `process_id` only is accepted; a merge query that does not
plan is rejected; a merge query that plans but whose output schema differs is rejected, and the
error names the differing field; a count query without a single `count: Int64` column is rejected; a
query missing `{begin}`/`{end}`/`{source}` is rejected; `now()` and `random()` in the extract query
are rejected and `date_bin` is not; a name colliding with a built-in view set is rejected. Ordering
(check 7): a definition reading a view set in group 3000 is rejected at 3000 and at 2000, accepted
at 3001; a definition reading nothing is accepted at any group; a definition reading *two* view sets
is measured against the higher of the two.

**Dependent protection** (§4b, in `view_registry_tests.rs` against the fake store) — dropping a
definition another one reads is refused and the error names the dependent; replacing it with a
definition that still projects the read columns is accepted; replacing it with one that drops a
column the dependent reads is refused; dropping a definition nothing reads is accepted; a definition
that was *already* failing to build does not by itself block an unrelated `DROP`.

**Hash parity for the seeded `log_stats`** (`view_definition_validation_tests.rs`) — build
`make_log_stats_view`'s `SqlBatchView` and the one the registry builds from the seeded definition
with `definition_hash_enabled = false`, and assert their `get_file_schema_hash()` and
`get_file_schema()` are equal. This is the test that protects every existing deployment's
`log_stats` partitions from the upgrade, and the failure it guards is silent: a mismatch empties the
view rather than erroring. Paired with a case asserting the same definition *with*
`definition_hash_enabled = true` produces a *different* hash, so the flag is doing real work.

**Seeded definition passes validation** (same file) — run the full §4 check set over the seeded
`log_stats` definition and assert it passes unmodified. It is the validator's calibration case; a
failure here means a check is miscalibrated, not that the view is wrong.

**Definition hash** (in `view_definition_validation_tests.rs`) — identical definitions hash equal;
changing each hashed field (`extract_query`, `count_src_query`, `merge_partitions_query`, either time
column) in turn changes the hash; changing `update_group`, either delta, or `merge_sort_order` alone
does not; and, as the CI regression guard for the `None` path
(the existing `sql_view_test.rs:226` assertion is `#[ignore]`d and does not run in CI), a new no-DB
test builds a `definition_hash: None` `SqlBatchView` on the `lakehouse_admin_gate_test.rs` offline
harness and pins `get_file_schema_hash()` to its current exact byte value.

**`view_registry_tests.rs`** — with a fake `ViewDefinitionStore`: definitions are built in
`update_group` order and a higher-group definition can read a lower-group one; a definition that
fails to build is skipped while the rest load, and the failure is reported; an unchanged
`(name, updated_at)` set short-circuits without rebuilding; `current()` returns the base factory
before the first reload; a `DROP`ped definition disappears from the swapped factory.

**Admin gate** — the gate is a standalone `authorize_view_ddl(&CallerContext) -> Result<(), Status>`,
unit-tested directly in `rust/public/tests/view_ddl_parse_tests.rs` (same crate as the gate), plus a
case in `lakehouse_admin_gate_test.rs` asserting `list_view_definitions()` is registered only for an
admin caller.

**`python/micromegas/tests/test_ddl_materialized_view.py`** — the end-to-end tier, against the local
test env, following `test_log_stats_integration.py` / `test_query_deny_list.py`: `CREATE OR REPLACE`
a small view over `log_entries`, assert it appears in `list_view_sets()` and
`list_view_definitions()`, `materialize_partitions` a known range, `SELECT` from it, `REPLACE` it
with a changed definition and assert the old partitions are gone, then `DROP` and assert it is gone
from both listings. This covers the wiring no unit test reaches — the FlightSQL round trip, the real
migration, the real `ViewRegistry` → daemon → `lakehouse_partitions` chain — and the failure it
guards is silent (a view set that loads but never materializes looks like an empty table, not an
error).

No new `#[ignore]` live-DB Rust test: per `CONTRIBUTING.md` those are reserved for pinning a bug
witnessed in the wild, and this is new-feature acceptance.

## Manual Verification

Each step below needs a running split-mode stack and is checking something no unit test can reach.

1. **Migration on an existing lake.** Point `MICROMEGAS_SQL_CONNECTION_STRING` at a v9 database and
   run `python3 local_test_env/ai_scripts/start_services.py`. Expect `upgrade lakehouse schema to
   v10` in `/tmp/analytics.log` and `SELECT version FROM lakehouse_migration` = 10. Not automated
   because it exercises a real pre-existing schema state, not a freshly created one.
2. **`log_stats` partitions survive the migration.** Before upgrading, record
   `SELECT encode(file_schema_hash,'hex'), count(*) FROM lakehouse_partitions WHERE view_set_name =
   'log_stats' GROUP BY 1`; after, re-run it and expect an identical hash and count, and
   `SELECT count(*) FROM log_stats` over a historical range to return the same rows as before. The
   unit test pins the hash function against the seeded text; this pins that a real deployment's
   already-written partitions still match it.
3. **Non-admin rejection.** With auth enabled, run a `CREATE MATERIALIZED VIEW` as a non-admin:
   `micromegas-query "CREATE MATERIALIZED VIEW t WITH (...) AS SELECT ..."`. Expect
   `PERMISSION_DENIED` and no new row in `lakehouse_view_set_definitions`. Exercises the real auth
   stack end to end rather than a constructed `CallerContext`.
4. **Reload without a restart.** With both services already running, create a view set, then poll
   `micromegas-query "SELECT * FROM list_view_sets() WHERE view_set_name = '<name>'"` from a
   *second* client. Expect it to appear within the refresh interval, and
   `/tmp/daemon.log` to show the daemon materializing it on the next minute tick — the thing the
   whole feature exists to do, and the one that cannot be observed in-process.
5. **Backfill.** `micromegas-query "SELECT * FROM materialize_partitions('<name>', TIMESTAMP '...',
   TIMESTAMP '...', 3600)"` and confirm `list_partitions()` shows the filled range. Confirms the
   manual backfill path the docs promise actually works for a DDL-defined view set.
6. **A DDL view reading another DDL view.** Create `a` over `log_entries` at group 4000, then `b`
   over `a` at group 4001; `materialize_partitions` both over the same range and confirm `b`'s rows
   are consistent with `a`'s. Then attempt `DROP MATERIALIZED VIEW a` and expect a refusal naming
   `b`. The group-ordering half is what unit tests cannot reach — it needs two real daemon passes
   over real partitions to show `b` is not reading `a`'s previous-tick state.
