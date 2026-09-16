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
`default_view_factory` drops its construction step (`view_factory.rs:344-352`) — the seeded
definition doubles as the validator's calibration case (see Testing Strategy).

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
(`rust/analytics/src/lakehouse/ownership_rewrite.rs:351-434`) picks a per-scan audience predicate by
schema introspection: an `audience` field → direct column filter; else a `process_id` field →
semi-join against `__processes__partitions`; else two name-keyed arms; **else
`Err(DataFusionError::Plan("no audience rule defined for view set '...'"))`**. This rule is
constructed for every caller whose `ReadScope != All`.

**Admin gating.** `query.rs:204-246` registers the eight admin-gated lakehouse UDTFs/UDFs only when
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
CREATE [OR REPLACE] MATERIALIZED VIEW <name> WITH (
  extract_query = $$
    SELECT date_bin('1 minute', time) as time_bin, target, count(*) as count,
           arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
      FROM log_entries
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'
     GROUP BY time_bin, target, audience
  $$,
  count_src_query = $$
    SELECT sum(nb_objects) as count FROM blocks
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'
  $$,
  merge_partitions_query = $$
    SELECT time_bin, target, sum(count) as count, audience
      FROM {source} GROUP BY time_bin, target, audience
  $$,
  update_group           = 4000,
  time_column            = 'time_bin',
  source_partition_delta = '1 day',
  merge_partition_delta  = '1 day',
  merge_sort_order       = 'time_bin, target'
)

DROP MATERIALIZED VIEW [IF EXISTS] <name>
```

All three queries are `WITH` options, so none of them is privileged: the extract query is a knob
like the other two rather than the statement body. That requires parsing the statement directly
instead of going through `sqlparser`'s `parse_create_view`, which calls
`self.expect_keyword_is(Keyword::AS)?` unconditionally (`sqlparser-0.62.0/src/parser/mod.rs:6578`)
and parses a query immediately after, so a `CREATE MATERIALIZED VIEW ... WITH (...)` with no `AS`
body cannot be parsed by it at all.

`parse_view_ddl` therefore hand-rolls the parse off a single `Parser::try_with_sql(sql)?`: `CREATE`,
an optional `OR REPLACE`, `MATERIALIZED`, `VIEW`, `parse_object_name(false)`,
`parse_options(Keyword::WITH)` (`:9968-9977`, which parses `WITH ( k = v, ... )` into
`Vec<SqlOption>`), then consume any trailing `Token::SemiColon` and reject with a named error unless
`parser.peek_token_ref()` is `Token::EOF`. The `DROP` branch is `DROP`, `MATERIALIZED`, `VIEW`, an
optional `IF EXISTS`, one `parse_object_name(false)`, then the same `;`/EOF requirement. Requiring EOF
preserves the single-statement rule `SessionContext::sql`'s `sql_to_statement` enforces for every
non-DDL query (datafusion-54.1.0 `src/execution/session_state.rs:454-458`); intercepting DDL ahead
of `ctx.sql` would otherwise bypass that guard and silently execute only the first of several
statements.

An option value is an `Expr` (`SqlOption::KeyValue { key: Ident, value: Expr }`,
`sqlparser-0.62.0/src/ast/mod.rs:8808-8813`), so each query's text is just the tokenizer's value for
a string literal and `ViewDefinition.extract_query` is that option's value verbatim. The three query
options accept either a dollar-quoted (`$$...$$`) or a single-quoted literal — dollar-quoted strings
parse as expression values under `GenericDialect`, which this repo already parses with
(`parse_value` at `sqlparser-0.62.0/src/parser/mod.rs:12011`, fed by the tokenizer at
`tokenizer.rs:1929` → `Expr::Value(Value::DollarQuotedString(..))`).
Dollar-quoting is the documented form because these queries contain single quotes (`'log'`,
`'{begin}'`, `'Dictionary(Int32, Utf8)'`) that a single-quoted literal would have to double.

Option semantics:

| option | required | maps to |
|---|---|---|
| `extract_query` | yes | `SqlBatchView::new`'s `extract_query` |
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
  view_options           TEXT NOT NULL DEFAULT '{}',
  created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by             TEXT
);
```

The same migration seeds `log_stats`:

```sql
INSERT INTO lakehouse_view_set_definitions (...) VALUES ('log_stats', ...)
ON CONFLICT (view_set_name) DO NOTHING;
```

Its text comes from a `log_stats` `ViewDefinition` fn in a new
`rust/analytics/src/lakehouse/builtin_view_definitions.rs` — moved out of
`log_stats_view.rs`, which the migration and the tests then share as one source of truth.
`ON CONFLICT DO NOTHING` keeps the migration idempotent.

`definition_sql` (`TEXT NOT NULL`, the verbatim DDL text) has no equivalent in
`log_stats_view.rs` today, since `log_stats` has never been expressed as DDL. A function in
`builtin_view_definitions.rs` assembles the equivalent `CREATE MATERIALIZED VIEW log_stats WITH (...)`
text from the `log_stats` `ViewDefinition` fn, wrapping each query in `$$...$$` — none of them contains a
`$$`, so no quote-escaping helper is needed. It seeds this column and doubles as the parser
round-trip fixture in the Testing Strategy, which asserts the full parsed `ViewDefinition` — every
option, not just the three queries — equals the fn's returned value exactly: inside `$$...$$` the text is taken
as-is, so the consts need no whitespace or trailing-`;` grooming for that equality to hold.

The `upsert` behind `CREATE OR REPLACE` is an `ON CONFLICT (view_set_name) DO UPDATE` that sets
`updated_at = now()` and `updated_by` explicitly on the `DO UPDATE` branch — the column defaults only
fire on `INSERT`, and `reload()`'s digest is over `(view_set_name, updated_at)`, so an update that
left it unset would go live only on the replacing node while every other flight-sql replica and the
maintenance daemon kept serving and materializing the old definition.

`view_options` holds the non-query options other than `update_group` (the time columns, the two
deltas, `merge_sort_order`), serialized with `serde_json::to_string`/`from_str` and stored as `TEXT`
rather than `JSONB` — the workspace `sqlx` dependency (`rust/Cargo.toml:85`) has no `json` feature,
and nothing else in the repo binds a Postgres column to `serde_json::Value`; `TEXT` needs no new
feature and `serde_json` is already a dependency (`rust/analytics/Cargo.toml:42`). `update_group` is
its own column because `reload()` sorts and §8 projects on it. `definition_sql` is the verbatim DDL
text, kept for display and audit only — never re-parsed.

See `## Decisions` for why `file_schema`/`schema_version` columns are absent from
`lakehouse_view_set_definitions`.

### 3. What a redefinition means

`file_schema_hash` stays purely schema-derived: `SqlBatchView::get_file_schema_hash` is left
unchanged, hashing the inferred Arrow schema and nothing else. There is no definition hash.

A `CREATE OR REPLACE` whose **output schema changes** therefore self-invalidates through the
existing mechanism — the newly inferred schema hashes differently and the query-side partition
provider stops reading the old partitions
(`rust/analytics/src/lakehouse/partition_cache.rs:386,420` filter on `file_schema_hash`).

A `CREATE OR REPLACE` that changes **content but not the output schema** (a widened filter, a
different source view, a changed `date_bin` interval) leaves the existing partitions readable, so
the view serves a mix of old- and new-definition data until an admin reclaims them.

The remedy is explicit and admin-driven: `retire_partitions(...)` over the affected range (or
`micromegas.admin.retire_incompatible_partitions` for the schema-changed case), followed by
`SELECT * FROM materialize_partitions('<name>', <begin>, <end>, <delta_secs>)` to rebuild. The
daemon only fills forward (2 days / 2 hours / 2 minutes back from `now`), so backfilling an older
range is that same manual call either way.

Unlike the `DROP` case (§6), a content-only `REPLACE` leaves the view set's rows in place while a
lagging daemon replica is still running the *old* definition, so the admin must wait out
`MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` between the `REPLACE` and the `retire_partitions` +
`materialize_partitions` sequence — otherwise a replica still on the old definition can write, into
the range just retired, a partition that hashes identically (`get_file_schema_hash` is schema-only)
and is then read forever as if it were new-definition data, with no error anywhere.

### 4. Validation at `CREATE` time

Every check runs before the row is written, in one `validate_view_definition` function shared by the
DDL executor and the registry loader — so a definition that the loader would skip can never be
accepted in the first place.

Constructing the `SqlBatchView` *is* most of the validation: it plans the extract query (catching
syntax errors, unknown tables, unknown columns) and yields the schema. All three queries arrive as
plain strings — they are option values, so none of them is pre-parsed by the DDL parse — and every
check below that inspects a query's structure parses and plans each of the three uniformly, with the
same walk applied to each. On top of that:

1. **Name.** Matches `^[a-z_][a-z0-9_]{0,254}$` and does not start with `__`; not a code-driven view
   set (`get_global_view` / `get_view_sets` on the base factory, which after this change means
   `blocks`, `processes`, `streams`, `log_entries`, `measures` and the five instance-only sets — but
   *not* `log_stats`, so the seeded row stays replaceable); not `source`. The name is interpolated
   into `__<name>__partitions` and into table registrations, so this is a correctness *and* an
   injection guard: without the `__` exclusion, a
   name like `__log_stats__partitions` would collide with that view's own `__<name>__partitions`
   internal registration and hard-error `make_session_context` for every query in the deployment, not
   just ones touching that view. `source` is excluded for the same reason: `SqlBatchView::new`
   substitutes `{source}` for the literal name (`sql_batch_view.rs:117`, and `:177` in
   `with_merge_sort_order`), and `QueryMerger::execute_merge_query` (`merge.rs:322-327`) registers
   that literal name via `register_table` on a session context that
   `make_merge_session_context` (`merge.rs:49-73`) has already populated with every global view —
   DataFusion errors on a duplicate registration, so a view set named `source` would break every other
   view's merge on every tick. Also reject if `ctx.table_exist(name)` is true against the validation
   session context built for this definition — that context has already run `configurator.configure`,
   so this one probe additionally catches a collision with a static table the `SessionConfigurator`
   registers, which the `get_global_view`/`get_view_sets` probe above cannot see and which
   otherwise fails silently the other way: `StaticTablesConfigurator::configure` runs last in
   `make_session_context`, after every global view is registered, so its own `register_table` call
   for the colliding name fails and is only `warn!`ed about — the static table silently disappears
   from every query in the deployment, not the DDL view.
2. **Audience reachability.** The inferred schema must contain an `audience` field or a `process_id`
   field, and that field's type must be `Utf8` or `Dictionary(_, Utf8)`. Without one, or with the
   wrong type, `OwnershipRewrite::audience_column_predicate`/the `process_id` arm
   (`ownership_rewrite.rs:234-260`, `:390-401`) still plans successfully — each `cast`s the column to
   `Utf8` before comparing — but matches nothing, so a non-string `audience`/`process_id` column
   serves an empty table to every non-admin caller with no error anywhere, the same silent-failure
   class check 9 already closes for the time columns. The error message names the requirement and
   points at `max(audience)`-in-`GROUP BY` as the fix. This mirrors the existing branch table exactly;
   the branch logic is unchanged, but `ownership_rewrite.rs`'s module doc — the six-view table, the
   claim that the column is always `Dictionary(Int32, Utf8)`, and the "non-null by construction …
   no unstamped case either way" claim — is reworded to cover DDL-defined view sets and the
   documented NULL obligation below.
3. **Merge query plans, and agrees.** Register an empty table carrying the inferred schema under the
   substituted `{source}` name, plan `merge_partitions_query`, and require its output schema to equal
   the extract query's over field names, data types and order only — nullability and field metadata
   are deliberately excluded from the comparison. `log_stats_view.rs` projects `count(*) as count` in
   the extract query (non-nullable) and `sum(count) as count` in the merge query (nullable), so a
   literal field-by-field equality would reject the seeded calibration case (§ Testing Strategy) on
   nullability alone. Today a bad merge query is not detected until the daemon's first merge, and a
   *mismatched* one is not detected at all — yet the
   merge query is the read path for any query spanning more than one partition
   (`sql_batch_view.rs:311-334`), so a disagreement means the table a user sees does not match the
   partitions it is built from.
4. **Count query shape.** `fetch_sql_partition_spec` (`sql_partition_spec.rs:196-203`) requires one
   batch, one row, one `Int64` column literally named `count`. Check the planned schema for that
   single `count: Int64` column and reject otherwise, rather than failing on the daemon's first tick.
5. **Placeholders.** `count_src_query` must contain `{begin}` and `{end}`;
   `merge_partitions_query` must contain `{source}`. The probe without a range returns a constant,
   so freshness is never detected — stale forever. The `{source}` requirement is mechanical: the
   merge query is unrunnable without it. `{begin}`/`{end}` are substituted with `insert_range.begin/end`
   (insert-time bounds, `sql_batch_view.rs:249-257`) in `count_src_query` regardless of what
   `extract_query` reads, so `count_src_query` must always count a raw source table carrying
   `insert_time` — in practice `blocks`, the way `log_stats` counts `blocks` while its `extract_query`
   reads `log_entries` (`log_stats_view.rs:18-26`) — even when `extract_query` reads another
   materialized view instead of a raw source. A materialized view carries no `insert_time` column
   (§4's obligation paragraph below), so a `count_src_query` filtering that view's event-time column
   with insert-time bounds would mis-detect freshness silently; counting `blocks` instead couples a
   dependent's freshness to its upstream's ingestion rate, not its upstream's own materialization
   cadence, which is the accepted trade-off.
6. **No volatile or stable functions.** Parse each of the three query texts with `ctx.sql(...)` and
   walk the resulting pre-optimization `LogicalPlan`s (`DataFrame::logical_plan()`) — never an
   optimized or physical plan: `SimplifyExpressions`'s `ConstEvaluator` treats `Stable` the same as
   `Immutable`, so it folds a call like `now()` into a literal before an optimized-plan walk would
   ever see the `ScalarUDF` node, silently defeating this check for every `Stable` function while
   leaving `Volatile` ones (which the optimizer does not fold) looking caught. The merge and
   count-source plans are already built this same unoptimized way by checks 3 and 4, so the extra
   walk is free — rejecting any `ScalarUDF` whose `signature().volatility` is not
   `Immutable` (`now()`, `random()`, `current_timestamp`). A volatile call in the extract or merge query is
   frozen into a partition at materialization time and then served to every later reader — wrong
   data, no error, indefinitely; in `count_src_query` it instead corrupts freshness detection
   (`verify_overlapping_partitions` compares row counts), causing perpetual re-materialization.

7. **No mutating table function, and `update_group` strictly after every view set the
   definition reads.** Walk all three of the same pre-optimization `LogicalPlan`s check 6 walks, for
   every `TableScan`, resolving each one's `table_name` against the factory the definition was built
   for — recursing into `TableSource::get_logical_plan()` when the scan is a `ViewTable` (which is how
   `SqlBatchView::register_table` exposes the user-visible name; a downcast to `MaterializedView`
   alone misses it and would only catch `__<name>__partitions` scans). Reject outright any scan whose
   `TableSource` resolves to one of the four mutating admin-gated table functions
   (`query.rs:204-246` — `retire_partitions`, `materialize_partitions`, `regenerate_partitions`,
   `deny_queries`) **or** to `view_instance(...)` — table functions are the only ones of the eight
   admin-gated items a `TableScan` can resolve to: check 6 only walks `ScalarUDF` expressions, so a
   UDTF call such as `SELECT * FROM retire_partitions(...)` is invisible to it and would otherwise be
   stored and then re-executed under `CallerContext::maintenance()` on every daemon tick.
   `view_instance(...)` is not admin-gated (it is registered unconditionally in
   `register_lakehouse_functions`) but is just as mutating: `MaterializedView::scan` calls
   `jit_update(...)` before fetching partitions, so a stored `view_instance(...)` scan would also
   write JIT partitions on every daemon tick and during validation itself. Other read-only table
   functions — including `list_query_denials`, `list_partitions`, `list_view_sets`,
   `list_audience_grants` and `list_view_definitions` (§8), and the unconditionally-registered
   `perfetto_trace_chunks`, `parse_block` and `process_spans` — resolve the same way but are not
   rejected: none calls `jit_update`/writes a partition, so each is a live, non-time-ranged metadata
   or decode source, and a scan against one is frozen into a partition as of materialization time
   rather than kept current, the same class of staleness check 7 otherwise polices, but with no
   mutation and no error to force closing it here. The remaining three admin-gated items —
   `retire_partition_by_file`, `retire_partition_by_metadata`, `remove_query_denial` — are scalar UDFs
   and, being `Volatility::Volatile`, are already caught by check 6. For every other scan,
   collect the matched view's `get_update_group()`. A `TableScan` reached through the `ViewTable`
   recursion names the underlying `__<name>__partitions` table, not the view set, so it is resolved by
   stripping the `__..__partitions` affix or by downcasting its `TableSource` to `MaterializedView` and
   reading `get_view().get_view_set_name()`; a scan matching neither form, and not one of the table
   functions named above, is ignored. Require `update_group >` the maximum found. Because §5 step 3 validates
   only against the base plus definitions in a strictly lower `update_group`, this ordering half of
   the check can only ever fire for a definition reading a base view; a DDL-on-DDL reference placed in
   the wrong group instead fails to resolve as a table name during planning, since a same-or-higher-
   group DDL view is not part of the factory it is validated against. It still turns
   `materialize_all_views`'s documented ordering obligation (`maintenance.rs:59-70`) into a check for
   the base-view case, and it costs nothing: the plan is already built by step 3.
8. **`merge_sort_order`, if given, plans.** The declared columns are *applied*, not demanded: they
   become a logical-plan `DataFrame::sort` on the extract query before its physical plan is built,
   exactly as `QueryMerger::execute_sorted_merge` (`merge.rs:210-233`) already does for the merge
   query. Neither query then carries an author-written `ORDER BY`, and a missing or mismatched one is
   not representable on either side. This needs one change to existing code:
   `execute_extract_query` (`sql_partition_spec.rs:79-115`) today only *asserts* the ordering,
   erroring with "Check for a missing or mismatched top-level ORDER BY" — the same guarantee
   enforced by blame on one side of the option and by construction on the other. Build the extract
   query's physical plan — separately from, and solely for, this check; it is never the plan checks 6
   and 7 walk — using the same `{begin}`/`{end}` substitution `SqlBatchView::new` already does
   (`sql_batch_view.rs:110-114`: both placeholders replaced with one `Utc::now()` timestamp) so an
   extract query filtering on `insert_time` plans instead of failing `TypeCoercion`/
   `ConstEvaluator` on the unsubstituted literal. That build is what this check buys: an extract
   query that cannot be planned at all is rejected at `CREATE` rather than on the daemon's first
   tick. This changes the ordering guarantee for every declared-sort view, not only DDL ones; step 4
   lists the existing offline tests and rationale comments this breaks and how each is updated.
9. **Time columns exist and are nanosecond timestamps.** The resolved `min_event_time_column` and
   `max_event_time_column` (from `time_column`, or `min_time_column`/`max_time_column` if given)
   must each name a field of the extract query's inferred schema, and that field's type must be
   `Timestamp(Nanosecond, _)`. Neither is checked today: `NamedColumnsTimeBounds::get_time_bounds`
   (`dataframe_time_bounds.rs:36-57`) downcasts to `TimestampNanosecondArray` and
   `SqlBatchView::make_time_filter` (`sql_batch_view.rs:297-301`) builds
   `col(&*self.min_event_time_column)`, so a typo'd or non-nanosecond column is otherwise accepted at
   `CREATE` and only fails on the daemon's first tick and on every ranged query.

Checks 2, 3, 5, 6 and 7 are the ones that earn their keep: each covers a failure that produces wrong
or stale *numbers* rather than an error. Check 8's payoff is different: it moves a hard failure —
an extract query that cannot be planned at all — from the daemon's first tick to `CREATE` time. What
stays an **unchecked author obligation**, documented
and not enforced: `extract_query` need not carry `{begin}`/`{end}` (the extract session context is
already range-scoped by `filter_insert_range` on the partition provider,
`sql_batch_view.rs:238`), but because that filter matches by *overlap*, a partition wider than the
bucket being materialized is scanned whole — so the query must either be idempotent under seeing
extra rows (`GROUP BY` with `first_value`/`max`) or filter the range explicitly (as `log_stats`
must, since `count(*)` would double-count); that any range predicate is on `insert_time` (not event
time); that the merge query's aggregates are composable over already-aggregated rows
(`sum(count)`, never `count(*)`, no bare `avg`); and that every row's `audience`/`process_id` (check
2) must be non-NULL — `OwnershipRewrite::audience_column_predicate` and the `process_id` arm
(`ownership_rewrite.rs:234-260`) both evaluate to NULL, i.e. drop the row, for a NULL value, so a
NULL is fail-closed and silently hides that row from every non-admin caller. Only the merge-aggregate
one is already spelled out for hand-written views, by `with_merge_sort_order`'s doc comment
(`sql_batch_view.rs:145-163`); none of the four is decidable from a logical plan.

The insert-time half of that obligation does not apply to `extract_query` when it reads another
materialized view rather than a raw source table: a materialized view's schema is whatever its own
`extract_query` projects, and carries no `insert_time` column at all — only `time_column`'s
event-time column is guaranteed to exist. A definition `b` reading definition `a` therefore filters
`extract_query` on `a`'s event-time column instead; the consequence, also documented rather than
enforced, is that a row arriving late into `a`'s own partitions after `b` has already covered that
time range is not picked up by `b`. `count_src_query` is unaffected by this — it is always written
against a raw source carrying `insert_time` (check 5), never against `a` itself.

### 4b. Dependents survive a `DROP` or a `REPLACE`

Cross-DDL references make one new failure reachable: dropping `a`, or replacing it with a definition
that no longer projects a column `b` reads, leaves `b` unable to plan. `b` would then be skipped by
the registry loader (§7) with only a `warn!`, so a user's table would quietly vanish.

The mutation is therefore validated against the *resulting* definition set, not just its own row:
`execute_view_ddl`'s transaction first calls `acquire_lock(&mut tx, VIEW_DDL_ADVISORY_LOCK_KEY)`
(`remote_data_lake.rs:13-19`, `pg_advisory_xact_lock`) — a new constant, `2`, alongside the DDL
executor (`migrate_db` already uses `0`, `migrate_lakehouse` uses `1`; reusing either would serialize
every DDL statement against an unrelated migration lock) — serializing every view-definition mutation
cluster-wide so two replicas can't each validate against a row set that omits the other's in-flight
write. Still inside that transaction, before applying the mutation, it reads the pre-mutation rows via
the transaction-scoped free function `view_definition_store::list_tx(&mut tx)` (§7), which returns
rows in the same `(update_group, view_set_name)` order as `store.list()`, since `build_factory` relies
on that ordering regardless of which one fed it. After the
upsert/delete, it re-reads the post-mutation rows with another `list_tx(&mut tx)`. Both row sets are
handed to
`ViewRegistry::check_dependents_survive(pre_rows, post_rows)` (§7, next to `build_from_rows`), which
builds a factory from each via the same rows-in seam and refuses with a named error if any definition
— including the row just written — that is *not* in the pre-mutation failed set fails to build against
the post-mutation rows. The error names the broken definition, whether a downstream dependent or the
mutated row itself. This is exact rather than textual — it uses the real planner, so it catches a
removed column as readily as a removed view set — and it reuses the loader wholesale. The baseline is computed transactionally, from the same rows the mutation is
validated against, rather than from `reload()`'s last periodic snapshot: that snapshot is up to
`MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` stale and differs per replica, which would make the
refusal nondeterministic.

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
/// keyword peek before any full parse. `Err` when the hand-rolled parse (§1) does not reach EOF
/// right after `<name> WITH (...)` (or, on `DROP`, right after the single name): an unmodelled
/// clause, an `AS` body, a second `DROP` name, or a second statement (matching
/// `SessionContext::sql`'s single-statement rule) each would otherwise silently do less than the
/// statement says.
pub fn parse_view_ddl(sql: &str) -> Result<Option<ViewDdl>, DdlError>;

/// The same gate as the eight admin-gated lakehouse UDTFs/UDFs (`query.rs:204-246`), pulled
/// out as a standalone function so it is unit-testable without a full `execute_view_ddl` call.
pub fn authorize_view_ddl(caller: &CallerContext) -> Result<(), Status>;
```

In `FlightSqlServiceImpl::execute_query`, inserted between the resolved `caller` and
`make_session_context`:

```rust
if let Some(ddl) = parse_view_ddl(sql).map_err(|e| audit_state.fail(client_input_error!("...", e)))? {
    return self.execute_view_ddl(ddl, &caller, audit_state).await;
}
```

`execute_view_ddl`:

1. `authorize_view_ddl(&caller)?` (§5's `view_ddl.rs`) — wraps
   `if !caller.is_admin { return Err(Status::permission_denied(...)) }`, the same gate as the eight
   admin-gated lakehouse functions. It has to be an explicit check rather than a registration gate,
   because this path never builds a caller session context. It is also load-bearing beyond the usual
   reason: a DDL view's queries are planned and materialized under `CallerContext::maintenance()`,
   i.e. `ReadScope::All`, so an author sees every audience regardless of their own read scope.
2. Open the transaction on `self.lakehouse.lake().db_pool` (which `FlightSqlServiceImpl` already
   owns), call `acquire_lock(&mut tx, VIEW_DDL_ADVISORY_LOCK_KEY)` first (§4b) to serialize steps
   3-5 cluster-wide, then read the pre-mutation rows via `view_definition_store::list_tx(&mut tx)`
   (§4b, §7).
3. `CREATE`: validate (§4) against the factory the loader would build for this row — the base plus
   every definition, from the pre-mutation rows read in step 2 with this row's own name excluded
   (identical to the post-mutation set for a single-row mutation), in a lower
   `update_group` — so a definition reading another DDL view validates against the real thing, and a
   `CREATE OR REPLACE` that raises its own `update_group` cannot validate against a since-superseded
   copy of itself. Reject an existing name unless `or_replace`; then `upsert` the row via
   `view_definition_store::upsert_tx(&mut tx, ...)`, all in the same transaction. A replace retires
   nothing (§3): partitions whose schema still matches stay readable, and reclaiming them is the
   admin's explicit `retire_partitions` call.
4. `DROP`: reject a built-in name; `delete` the row via `view_definition_store::delete_tx(&mut tx,
   ...)` and retire the partitions (§6) in one transaction; honour `IF EXISTS`.
5. Either mutation then re-reads the post-mutation row set via `view_definition_store::list_tx(&mut
   tx)` and calls `registry.check_dependents_survive(&pre_rows, &post_rows)` (§4b, §7) with the
   pre-mutation rows read in step 2, inside the same transaction, rolling back if it broke a
   dependent or failed to build the mutated row itself.
6. After the transaction (steps 2–5) commits and its advisory lock releases, call `registry.reload()`
   inline, so the statement's own connection can query the new view set immediately instead of
   waiting out the interval. It must run after the commit: `reload()` starts with the pool-backed
   `store.list()` (§7), a second, uncommitted-write-blind connection, so calling it any earlier would
   have it read the pre-mutation rows and swap in the old factory.
7. Return a one-row, two-column result (`view_set_name: Utf8`, `status: Utf8` ∈
   `created | replaced | dropped | not_found`), wrapped in `CompletionTrackedStream::new(...,
   audit_state)` exactly like every other `execute_query` return, so `client.query("CREATE ...")`
   yields a DataFrame, the FlightSQL stream shape is unchanged, and the accepted statement — not just
   a rejected one — is recorded in `flightsql_query_audit`.

`do_action_create_prepared_statement` (`flight_sql_service_impl.rs:1332-1375`) rejects a statement
that `parse_view_ddl` matches, with a message saying DDL must be executed directly. Nothing in the
repo prepares DDL, and planning it there would fail with an opaque DataFusion error.

### 6. Retiring a view set's partitions

A `DROP` retires the view set's partitions. `retire_partitions`
(`write_partition.rs:183-383`) is keyed on `(view_set_name, view_instance_id)` and is deliberately
hash-agnostic, but needs an explicit insert-time range. Resolve the exact range
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
    reload_mutex: tokio::sync::Mutex<()>,   // held across all of reload() (list -> build -> swap),
                                             // not just the swap; see reload() step 0
}

impl ViewRegistry {
    pub fn new(base: Arc<ViewFactory>, store: Arc<dyn ViewDefinitionStore>, ...) -> Self;
    pub fn current(&self) -> Arc<ViewFactory>;
    pub async fn reload(&self) -> Result<()>;
    /// Thin wrapper over `build_factory` supplying `self`'s `base`/`runtime`/`lake`/
    /// `session_configurator`, so a caller holding `Arc<ViewRegistry>` (§5 step 5) doesn't need those
    /// four pieces separately.
    pub async fn build_from_rows(&self, rows: &[ViewDefinitionRow]) -> Result<(Arc<ViewFactory>, Vec<String>)>;
    /// §4b's dependent-protection refusal, factored out as a plain rows-in/rows-out function (no
    /// `sqlx::Transaction`) so it is a no-DB unit test target: builds a factory from `pre_rows` and
    /// one from `post_rows` via `build_from_rows`, then refuses with a named error if any definition
    /// present in `post_rows` — including the row just written — both fails to build against
    /// `post_rows` and was not already in the pre-mutation failed set.
    pub async fn check_dependents_survive(
        &self,
        pre_rows: &[ViewDefinitionRow],
        post_rows: &[ViewDefinitionRow],
    ) -> Result<()>;
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

0. Acquire `reload_mutex` and hold it through steps 1-5, including their `await`s — not just the
   final swap. This matches `QueryDenyList::write_lock`'s rationale (`query_deny_list.rs:392-403`):
   without it, a periodic reload that listed rows before a DDL-triggered reload could block behind the
   DDL's own reload and then swap the pre-mutation factory back in over §5 step 6's post-commit one, a
   lost update that would stay live for up to `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`.
1. `store.list()` → rows ordered by `(update_group, view_set_name)`.
2. Hash the `(view_set_name, updated_at)` tuples; if unchanged from `loaded_digest` **and** the
   previous build's failed set was empty, return — the steady-state reload is one `SELECT` and no planning,
   which is what makes a 60 s interval affordable. A digest match after a failed build does *not*
   short-circuit, so a row that failed for a transient reason (a source view momentarily unplannable
   mid-rollout, an object-store hiccup) is retried every tick rather than stuck until its row next
   changes or the process restarts.
3. `build_factory(&self.base, &rows, self.runtime.clone(), self.lake.clone(),
   self.session_configurator.clone()).await`: `let mut factory = (*self.base).clone();` then, per row in
   order, build the `SqlBatchView` against `Arc::new(factory.clone())`, run it through the same
   `validate_view_definition` (§4) the DDL executor uses — against that same factory-so-far, so check
   7's `update_group` ordering sees only the rows already folded in — and only then
   `factory.add_global_view(...)`. Each definition therefore sees the built-ins plus every DDL view
   with a lower `update_group` — the same incremental clone-and-extend chain `default_view_factory`
   uses, and the same ordering rule `materialize_all_views` already documents. Check 1 (§4) already
   rejects a name that resolves via `get_global_view`/`get_view_sets` on the base factory, so until
   step 15 removes `log_stats`'s compiled construction from `default_view_factory`, the seeded
   `log_stats` row simply fails validation and is skipped like any other invalid row.
4. A row that fails to **build or validate** is **skipped**, not fatal: `warn!` plus
   `imetric!("view_definition_load_failure", "count", tags, 1)` tagged with the view set name, and
   `build_factory` collects its name into the returned failed-set. One broken definition must not take
   out every other view set, or the whole lakehouse — and running validation here, not just
   construction, is what keeps a definition whose merge query cannot plan, whose audience column was
   lost to source drift, or whose `CREATE OR REPLACE` raised its own `update_group` past a dependent's
   from ever reaching `current()`.
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
  and is handed the snapshot — no churn there. `flight_sql_server.rs` builds the registry, awaits `registry.reload()` before
  `serve()` (failing startup on error, the way `migrate_lakehouse` already does), and only then calls
  `spawn_refresh_task(fanout.subscribe())` next to the existing `query_denials` one (`:406`) —
  otherwise every restart has a window where `log_stats` and every DDL-defined view are absent from
  `current()`.
- `maintenance.rs`: `Views` (only its use on the four view-carrying `CronTask` structs) becomes
  `Arc<ViewRegistry>`;
  each `run()` computes `get_global_views_with_update_group(&registry.current())`, sorts by
  `update_group`, and passes the resulting `Arc<Vec<Arc<dyn View>>>` to `materialize_all_views`, whose
  `views: Arc<Vec<Arc<dyn View>>>` parameter stays spelled out (not the `Views` alias) and unchanged.
  `daemon()` takes
  `Arc<ViewRegistry>` instead of `Vec<Arc<dyn View>>`, awaits `registry.reload()` before spawning the
  cron tasks (failing startup on error, the way `migrate_lakehouse` already does), and then spawns the
  refresh task itself. The sort moves out of `daemon` into a helper the four view-carrying tasks call
  per tick. `materialize_all_views`'s
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

### Milestone 1 — persistence, validation, DDL

1. New `rust/analytics/src/lakehouse/view_definition.rs` — `ViewDefinition` (the normalized option
   set + three queries), `parse_time_delta`, the name charset check, and
   `build_sql_batch_view(&ViewDefinition, Arc<ViewFactory>, ...) -> Result<SqlBatchView>`. First
   because step 2's `log_stats` `ViewDefinition` fn and step 3's migration seed both need this type to exist.
2. New `rust/analytics/src/lakehouse/builtin_view_definitions.rs` — `log_stats` exposed as a
   `fn` returning its `ViewDefinition` (the three queries plus every option: `update_group`, `time_column`, the
   two deltas, `merge_sort_order`), lifted out of `log_stats_view.rs` verbatim — its transform
   query keeps its `ORDER BY` until step 4 adds the sort-applying path that makes it redundant —
   plus a function
   assembling it into the equivalent `CREATE MATERIALIZED VIEW log_stats ...` DDL text for the seed's
   `definition_sql`; consumed by the migration's seed (which serializes the fn's returned options via
   `serde_json` for `view_options`) and by the parser round-trip test. Registered in
   `rust/analytics/src/lakehouse/mod.rs`.
3. `rust/analytics/src/lakehouse/migration.rs` — bump `LATEST_LAKEHOUSE_SCHEMA_VERSION` to `10`,
   append the `9 == current_version` block, add `upgrade_v9_to_v10` creating
   `lakehouse_view_set_definitions`, seeding `log_stats` from `builtin_view_definitions`'s
   `log_stats` `ViewDefinition` fn (`view_options` serialized with `serde_json::to_string`, `definition_sql`
   from the assembled DDL text), and ending with `UPDATE lakehouse_migration SET version=10`.
4. `rust/analytics/src/lakehouse/view_definition.rs` (same module as step 1) —
   `validate_view_definition`: §4 checks 1–9, on top of the `SqlBatchView` built in step 1 (the
   charset half of check 1 may additionally run in the parser). Check 7 needs the referenced view
   sets' `update_group`s, so it takes the factory the definition was built against. Check 8 builds
   its own physical plan from the extract query with `{begin}`/`{end}` substituted, per §4. Also
   here: `sql_partition_spec.rs`'s `execute_extract_query` applies the declared `sort_order` as a
   `DataFrame::sort` before planning, mirroring `merge.rs:210-233`, and `with_merge_sort_order`'s
   doc comment loses item 3's top-level-`ORDER BY` requirement. Two more comments describing the old
   verify-the-`ORDER BY` semantics are updated in the same step: `SqlPartitionSpec::sort_order`'s
   field doc (`sql_partition_spec.rs:40-44`, "When set, `write` verifies the extract query's physical
   plan actually satisfies it") and `execute_extract_query`'s own doc comment (`:72-78`, "verifies …
   that its output ordering satisfies the declared columns"). This is what makes dropping the
   author-written `ORDER BY` safe, so it is also where `log_stats`'s transform query's `ORDER BY`
   (lifted, still present, in step 2) is removed — a projection-identical change, so the seeded
   row's `file_schema_hash` is unaffected. Update all the tests and comments this enables in the same
   step: `sql_partition_spec_sort_order_tests.rs`'s `extract_query_missing_order_by_fails_the_write` to
   assert the extract query's physical plan, built through this sort-applying path, satisfies the
   declared order — mirroring `extract_query_matching_order_by_passes_the_ordering_check` — instead
   of asserting `write()`'s `expect_err`, and that file's module header (today stating the removed
   "refuses to record a false sort_order guarantee ... e.g. a missing top-level `ORDER BY`" contract
   verbatim), rewritten to describe the sort-applying path; `log_stats_ordering_tests.rs`'s
   `log_stats_extract_query_satisfies_its_declared_sort_order` (and its header comment pinning
   "`ORDER BY time_bin, process_id, level, target`") to match the removed `ORDER BY`, planning
   through this same sort-applying path rather than the raw SQL text; and
   `ordered_aggregation_spike_tests.rs`'s `cte_internal_order_by_is_discarded_by_a_later_join` and
   `top_level_order_by_satisfies_the_declared_columns`, whose rationale comments cite the same removed
   contract ("SqlPartitionSpec::write's declared-path plan verification relies on"/"plan verification
   relies on to accept a fresh extract query") though their assertions are unaffected and keep passing
   — reworded to describe the sort-applying path instead.
5. New `rust/analytics/src/lakehouse/view_definition_store.rs` — the `ViewDefinitionStore` trait,
   reduced to the single pool-backed `list()` method `reload()` needs (its only implementors are the
   Postgres impl and a test fake), plus free functions `list_tx`/`upsert_tx`/`delete_tx` and
   `partition_insert_range`, each taking a `&mut sqlx::Transaction` directly — so the DDL path (§4b,
   §5), including its existence check behind `or_replace`, runs entirely on `execute_view_ddl`'s own
   open transaction alongside `retire_partitions`, with no trait indirection for methods a test fake
   could never implement.
6. New `rust/analytics/src/lakehouse/view_registry.rs` — `ViewRegistry`, the shared `build_factory`
   rows-in seam (building and validating each row via `validate_view_definition`), `reload` (ordered
   incremental build, skip-on-failure, digest short-circuit that also bypasses on a prior failure),
   `current()`, `spawn_refresh_task`, and the `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` knob
   (default 60). Lands in this milestone rather than the next because of the ordered build (step 10's
   validation against lower-group definitions) and §4b's post-mutation rebuild via `build_factory`.
7. New `rust/public/src/servers/view_ddl.rs` — `parse_view_ddl`, hand-rolling the parse over
   `datafusion::sql::sqlparser` (`Parser::try_with_sql`, the keyword sequence, `parse_object_name(false)`,
   `parse_options(Keyword::WITH)`, then the `;`/EOF requirement) into `ViewDdl`; option extraction
   out of each `SqlOption::KeyValue`'s `Expr` value — the three query texts are the string-literal
   values of their own options — and its error type; rejecting, with a named error, anything
   trailing the option list, including an `AS` body, a second `DROP` name and a second statement
   (§1); `authorize_view_ddl(&CallerContext) -> Result<(), Status>`,
   the standalone admin gate that `execute_view_ddl` step 1 calls.
8. `rust/public/src/servers/flight_sql_service_impl.rs` — `view_factory: Arc<ViewFactory>` field
   becomes `view_registry: Arc<ViewRegistry>`; `execute_query`, `do_get_tables`, and
   `do_action_create_prepared_statement` call `current()`. This breaks
   `rust/public/tests/read_policy_threading_tests.rs`'s `FlightSqlServiceImpl::new` call, updated to
   construct a `ViewRegistry` over its fixture factory using the Postgres-backed `ViewDefinitionStore`
   over the test's existing `connect_lazy` pool (`:55-56`) — the test never calls `reload()`, so the
   store is never actually queried. Moved ahead of step 10 because `execute_view_ddl` (step 10) is a method on
   `FlightSqlServiceImpl` that calls `registry.reload()`/`build_from_rows()` and so needs the field to
   already exist.
9. `rust/public/src/servers/flight_sql_server.rs` — construct the registry, await
   `registry.reload()` before `serve()` (failing startup on error, like `migrate_lakehouse`), then
   call `spawn_refresh_task(fanout.subscribe())`; `ViewFactoryFn` keeps producing the *base* factory.
   Moved ahead of step 10 for the same reason as step 8: `execute_view_ddl` needs a constructed
   registry to call.
10. `rust/public/src/servers/flight_sql_service_impl.rs` — add the `parse_view_ddl` branch, inserted
    between the resolved `caller` and `make_session_context`, and `execute_view_ddl` (admin gate,
    validate, upsert/delete, retire on `DROP`, §4b rebuild-or-rollback, inline reload, one-row
    answer).
    Reject DDL in `do_action_create_prepared_statement`.
11. `rust/analytics/src/lakehouse/mod.rs` / `rust/public/src/servers/mod.rs` — register the remaining
    new modules with their one-line doc comments.

### Milestone 2 — daemon pickup

12. `rust/public/src/servers/maintenance.rs` — `Views` → `Arc<ViewRegistry>` on the four view-carrying
    task structs, per-tick view resolution + sort helper, empty-list early return in
    `materialize_all_views`, `daemon`'s signature change, its awaited `registry.reload()` before
    spawning the cron tasks (failing startup on error), and its `spawn_refresh_task` call. Update
    `daemon`'s rustdoc `views_to_update` argument entry (`:404`) to match the new `Arc<ViewRegistry>`
    parameter.
13. `rust/telemetry-maintenance-srv/src/main.rs` and `rust/monolith/src/main.rs` — build the
    registry from `default_view_factory` and hand it to `daemon`, resolving the same
    `StaticTablesConfigurator::from_env("MICROMEGAS_STATIC_TABLES_URL", ...)` the FlightSQL builder
    uses (`flight_sql_server.rs:275-283`) as `ViewRegistry::new`'s `session_configurator`, instead of
    a no-op one — a DDL view reading a static table must build the same way in both services. Also
    update `local_test_env/ai_scripts/start_services.py` to export a low
    `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` (e.g. `5`) for the flight-sql and maintenance
    services it starts, so the Milestone 3 e2e test's daemon-pickup assertion (Testing Strategy) can
    use a timeout on the order of seconds instead of the 60 s production default.

### Milestone 3 — introspection and `log_stats` cutover

14. New `rust/analytics/src/lakehouse/list_view_definitions_table_function.rs`, registered in
    `rust/analytics/src/lakehouse/mod.rs` and in `query.rs`'s `if lakehouse_admin` block. Update
    `rust/analytics/tests/lakehouse_admin_gate_test.rs:2-5`'s header from "eight" to "nine" and add
    `list_view_definitions` to its inline enumeration.
15. `rust/analytics/src/lakehouse/view_factory.rs` — drop the `log_stats` construction step
    (`:344-352`); `default_view_factory` now returns the base. Update its module doc comment
    (`:303`) from "the six global views" to "the five global views". `log_stats_view.rs`'s
    `make_log_stats_view` is kept but reduced to `build_sql_batch_view` over the `log_stats`
    `ViewDefinition` fn (step 1, step 2), so the tests below still have a compiled `log_stats` to
    build.
    `rust/analytics/tests/audience_mismatch_skip_db_test.rs` builds its `log_stats` view via
    `make_log_stats_view` and `add_global_view`s it onto its own clone of `default_view_factory`,
    instead of pulling `log_stats` from `default_view_factory` directly — the
    test both looks the view up via `view_factory.get_global_view("log_stats")` and runs `SELECT ...
    FROM log_stats ...` through that same factory, so both need a factory that still carries it.
    Deferred to the end of this milestone (and behind the daemon's Milestone 2 pickup) because the
    daemon (step 12) and `FlightSqlServiceImpl` (step 8) must already be reading `registry.current()`
    before `log_stats` is dropped from the base factory, or the view goes unmaterialized and
    unqueryable in between.
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
- `rust/analytics/src/lakehouse/migration.rs`, `query.rs`, `mod.rs`, `view_factory.rs`,
  `log_stats_view.rs`, `sql_partition_spec.rs`, `sql_batch_view.rs`, `ownership_rewrite.rs`
- `rust/analytics/tests/audience_mismatch_skip_db_test.rs`,
  `ownership_rewrite_public_view_set_tests.rs`, `lakehouse_admin_gate_test.rs`,
  `sql_partition_spec_sort_order_tests.rs`, `log_stats_ordering_tests.rs`,
  `ordered_aggregation_spike_tests.rs`
- `rust/public/tests/read_policy_threading_tests.rs`
- `rust/public/src/servers/maintenance.rs`, `flight_sql_service_impl.rs`, `flight_sql_server.rs`, `mod.rs`
- `rust/telemetry-maintenance-srv/src/main.rs`, `rust/monolith/src/main.rs`
- `local_test_env/ai_scripts/start_services.py`
- `mkdocs/docs/admin/functions-reference.md`, `maintenance.md`, `flight-sql.md`, `authorization.md`,
  `authentication.md`
- `mkdocs/docs/query-guide/schema-reference.md`, `mkdocs/mkdocs.yml`, `mkdocs/docs/grafana/usage.md`
- `CHANGELOG.md`

## Trade-offs

**DDL interception vs. a `create_materialized_view(...)` UDTF.** A UDTF would need no parser work
and would inherit the existing `is_admin` registration gate for free. It was rejected because the
issue asks for DDL, and because `CREATE OR REPLACE`/`DROP` is the statement pair an admin expects
for defining and removing a view, with the name in the statement rather than in an argument list.
The cost is a hand-rolled parse step in front of `ctx.sql`.

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

## Decisions

- Validation rejects a definition whose merge-query output schema differs from its extract-query
  output schema, rather than warning — a disagreement means the user-visible table and the
  partitions backing it have different shapes, with no error at query time.
- No `file_schema`/`schema_version` columns in `lakehouse_view_set_definitions` — both are derived by
  planning `extract_query`.
- A definition that fails to load is skipped with a `warn!` and a metric; the registry still swaps
  in every definition that did load. One bad row must not take the lakehouse down.
- The insert-time-vs-event-time obligation and the merge-aggregate composability obligation are
  documented author contracts, not enforced checks — neither is decidable from a logical plan.
- `merge_sort_order` applies the sort to both queries instead of requiring a top-level `ORDER BY` in
  the extract query, which changes `execute_extract_query` for every declared-sort view, not only
  DDL ones. The alternative is a DDL option that silently demands an `ORDER BY` the author cannot
  see is missing until the daemon's first tick fails.
  Accepted risk, and the same one hand-written `SqlBatchView`s already carry.
- `MICROMEGAS_PUBLIC_VIEW_SETS` can name a DDL-defined view set, which disables its audience filter
  entirely. That is the existing operator knob behaving as designed; no extra guard.
- `update_group` is a **required** option with no default. There is no dependency inference here —
  the author states the order, and a definition reading another materialized view has to place
  itself after it. A default would be silently wrong for exactly that case.
- A DDL-defined view set **may read another one**, as a first-class use case; enforced by §4 check 7
  (`update_group` ordering) and §4b (dependents block a `DROP`/`REPLACE`). Its `count_src_query` must
  still count a raw source carrying `insert_time` (in practice `blocks`), never the upstream view
  itself, since `{begin}`/`{end}` there are always insert-time bounds (§4 check 5).
- No `CASCADE` on `DROP` in v1.
- `log_stats` is seeded into `lakehouse_view_set_definitions` and removed from
  `default_view_factory`; `blocks`, `processes`, `streams`, `log_entries` and `measures` stay
  code-driven — seeding `processes`/`streams` too would put audience resolution itself behind a
  database row, so a failed load or an operator `DROP` would break every non-admin query, unlike
  `log_stats`, whose worst failure is a clean `table not found`.
- There is **no definition hash**: `file_schema_hash` stays purely schema-derived, so a
  content-only `CREATE OR REPLACE` leaves stale partitions readable until the admin retires and
  re-materializes them (§3) — accepted, since auto-invalidating a view's whole materialized history
  is too harsh for an admin-driven DDL statement.
- The seeded `log_stats` row is a pure data move: its `file_schema_hash` cannot change, so existing
  partitions survive the upgrade. It gets no other special casing — droppable, replaceable, and
  skipped-with-a-warning on a load failure like any other row.
- `ON CONFLICT DO NOTHING` freezes the seeded `log_stats` text at v10: a later release changing
  `log_stats`'s shipped SQL must ship its own migration step that **upserts** the row (accepting that
  it overwrites an operator's `CREATE OR REPLACE`), not just edit the `builtin_view_definitions`
  consts.
- The `extract_query` is **not** required to contain `{begin}`/`{end}`. `processes` and `streams`
  carry no such predicate today and are correct, because `make_batch_partition_spec` scopes the
  scan through the partition provider (`sql_batch_view.rs:238`). Only `count_src_query`'s
  placeholders are enforced. The residual idempotence obligation is documented, not checked.
- No cap on the number of DDL-defined definitions, unlike `QueryDenyList`'s
  `MICROMEGAS_QUERY_DENY_MAX_RULES`. Each definition adds one `ctx.sql(...)` plan build to every
  query's `make_session_context`; an O(N²) cost to every reload, because `build_factory`'s
  incremental clone-and-extend chain replans every prior definition on each rebuild; and one
  `count_src_query` execution per definition on every daemon tick (second/minute/hour/day, via
  `get_global_views_with_update_group` feeding all four view-carrying cron tasks), independent of
  query traffic. Accepted for v1.
- A `DROP`'s `retire_partitions` races a lagging daemon replica for up to
  `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`: orphan partitions it writes after the retire are
  reclaimed by retention, not immediately; a same-schema `CREATE` re-using the name within that
  window should be followed by an explicit `retire_partitions` call. Accepted risk.
- A content-only `CREATE OR REPLACE`'s `retire_partitions` + `materialize_partitions` sequence races
  a lagging daemon replica the same way, but silently, with no error anywhere — see §3 for the
  mechanism. Accepted risk.

## Documentation

- **New** `mkdocs/docs/admin/materialized-views.md` — the DDL reference (§1), the redefinition/`DROP`
  lifecycle and the two accepted-risk races (§3), the author obligations (§4's obligation paragraph),
  and the accepted risks recorded in `## Decisions`. The page states its content in self-contained
  prose — it must not cite this plan's section numbers or decision entries directly. Added to
  `mkdocs/mkdocs.yml`'s nav.
- `mkdocs/docs/admin/functions-reference.md` — `list_view_definitions()`, and a pointer to the page
  above from the admin-function list.
- `mkdocs/docs/admin/maintenance.md` — `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` in the env-var
  table, and that the daemon now picks up view sets without a restart; also add
  `MICROMEGAS_STATIC_TABLES_URL` to the same table, next to the `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`
  entry, with the same "set it identically on every role" note the table already carries for
  `MICROMEGAS_DEFAULT_AUDIENCE` (`:18`) — step 13 makes the maintenance daemon resolve this variable
  for the first time.
- `mkdocs/docs/admin/flight-sql.md` — the same env var, and that DDL is admin-gated.
- `mkdocs/docs/admin/authorization.md` — a sentence that DDL-defined view sets must carry `audience`
  or `process_id` and are filtered by the same two `OwnershipRewrite` branches as the code-driven
  views already listed there; also update its admin-gated-functions section (`:174`, `:186`) from
  eight to nine functions, adding `list_view_definitions()` to the enumeration, and note that view
  DDL (`CREATE`/`DROP`, §5) is gated by the same admin check.
- `mkdocs/docs/admin/authentication.md` — its cross-reference (`:579`) to "the eight gated SQL
  functions" becomes nine, matching `authorization.md`'s updated count.
- `mkdocs/docs/grafana/usage.md` — its "Materialized Views" pointer (`:133`) currently links to
  `../admin/maintenance.md`; repoint it at `../admin/materialized-views.md`.
- `mkdocs/docs/query-guide/schema-reference.md` — one paragraph saying `list_view_sets()` includes
  DDL-defined view sets and that their schemas are deployment-specific, plus a note that
  `log_stats` is now a seeded definition an operator may extend or replace (its documented schema
  is the shipped default, not a guarantee).
- `view_factory.rs`'s module rustdoc (`:1-64`) — keep the `## log_stats` schema table but note it is
  now a seeded definition, not one `default_view_factory` builds, matching the `schema-reference.md`
  note.
- `CHANGELOG.md` — one entry, covering the new `CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP
  MATERIALIZED VIEW` DDL surface, the `list_view_definitions()` UDTF, the
  `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` reload-interval knob, the v10 lakehouse migration, and
  that `log_stats` is now a seeded definition rather than a compiled view (identical SQL surface and
  identical `file_schema_hash`, so no rebuild and no dashboard change), with the **Minor breaking
  change** clause for `daemon`'s signature, `FlightSqlServiceImpl::new`'s `view_factory` →
  `view_registry` parameter, `Views`, `default_view_factory` no longer returning `log_stats`, and
  `with_merge_sort_order` no longer requiring a top-level `ORDER BY` in the extract query (the sort
  is now applied for every declared-sort view, hand-written or DDL).

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
name failing the charset check; an unmodelled clause on `CREATE` (a view column list,
`IF NOT EXISTS`) and on `DROP` (`CASCADE`, a second name), each a named error; an `AS`-body form,
now invalid, rejected; and a trailing second statement (`CREATE ...; DROP ...`) rejected,
pinning the single-statement rule. Round-trip: each of the three query options parses back to
exactly the submitted text, with a `$$`-quoted body carrying single quotes, `{begin}`/`{end}`
placeholders, newlines and mixed casing, plus the single-quoted-literal form for the same query.
Against the seeded `log_stats` DDL text specifically, the parsed `ViewDefinition` — every option,
not just the three queries — must equal `builtin_view_definitions`'s `log_stats` `ViewDefinition` fn's return value exactly.

**`view_definition_validation_tests.rs`** — against a fixture factory carrying a fake source view
with a fixed schema: a definition whose schema has neither `audience` nor `process_id` is rejected;
one with `audience` is accepted; one with `process_id` only is accepted; one whose `audience` column
is not `Utf8`/`Dictionary(_, Utf8)` (e.g. an integer or boolean) is rejected; one whose `process_id`
column is not `Utf8`/`Dictionary(_, Utf8)` is rejected; a merge query that does not
plan is rejected; a merge query that plans but whose output schema differs is rejected, and the
error names the differing field; a count query without a single `count: Int64` column is rejected; a
query missing `{begin}`/`{end}`/`{source}` is rejected; `now()` and `random()` in the extract, merge
and count-source query are each rejected and `date_bin` is not; a `time_column` naming a field
absent from the extract query's schema is rejected; one naming a field that is not
`Timestamp(Nanosecond, _)` is rejected; a name colliding with a built-in view set is rejected. Ordering
(check 7): a definition reading `log_entries` (a base view set, update_group 2000) is rejected at
2000, accepted at 2001; a definition reading nothing is accepted at any group; a definition reading
two base view sets is measured against the higher of the two; a definition scanning
`retire_partitions(...)` (or another admin-gated mutating UDTF) is rejected regardless of group; a
definition scanning `view_instance(...)` is rejected regardless of group.

**Dependent protection** (§4b, in `view_registry_tests.rs`, calling
`ViewRegistry::check_dependents_survive` directly with canned pre/post row slices — no transaction or
live store needed) — dropping a definition another one reads is refused and the error names the
dependent; replacing it with a definition that still projects the read columns is accepted; replacing
it with one that drops a column the dependent reads is refused; dropping a definition nothing reads is
accepted; a definition that was *already* failing to build does not by itself block an unrelated
`DROP`.

**Seeded definition passes validation** (`view_definition_validation_tests.rs`) — run the full §4
check set over the seeded `log_stats` definition and assert it passes unmodified. It is the
validator's calibration case; a failure here means a check is miscalibrated, not that the view is
wrong. In particular this is what exercises check 3's nullability exclusion: `count(*)`
(non-nullable) in the extract query vs. `sum(count)` (nullable) in the merge query. The same test
module also asserts `build_sql_batch_view(<seeded log_stats ViewDefinition>)`'s inferred schema
against an explicit expected field list (names, types, nullability) captured from today's compiled
`log_stats` view — not a comparison against `make_log_stats_view(...)`'s own hash, which is derived
from the same `ViewDefinition` and so cannot catch a divergence — a no-DB check
that the migration's seeded definition infers the identical Arrow schema as the compiled view it
replaces, since a divergent schema would make every pre-upgrade `log_stats` partition unreadable
(`partition_cache.rs:386,420` filter on exact `file_schema_hash`) with no error anywhere.

**`view_registry_tests.rs`** — with a fake `ViewDefinitionStore`: definitions are built in
`update_group` order and a higher-group definition can read a lower-group one; a definition that
fails to build is skipped while the rest load, and the failure is reported; an unchanged
`(name, updated_at)` set short-circuits without rebuilding; `current()` returns the base factory
before the first reload; a `DROP`ped definition disappears from the swapped factory.

**Admin gate** — the gate is a standalone `authorize_view_ddl(&CallerContext) -> Result<(), Status>`,
unit-tested directly in `rust/public/tests/view_ddl_parse_tests.rs` (same crate as the gate), plus a
case in `lakehouse_admin_gate_test.rs` asserting `list_view_definitions()` is registered only for an
admin caller. The wiring — that `execute_query` actually routes DDL through the gate before
`make_session_context`, not just that the gate function itself is correct — is covered by a new
`read_policy_threading_tests.rs` case mirroring `bulk_ingest_denies_non_admin_caller` (`:479-510`):
send a `CREATE MATERIALIZED VIEW ...` statement through the real `AuthService`/tonic stack as the
`ApiKeyAuthProvider` (always non-admin) caller and assert `Code::PermissionDenied`. This is the
same offline harness step 8 already edits to construct a `ViewRegistry`.

**`python/micromegas/tests/test_ddl_materialized_view.py`** — the end-to-end tier, against the local
test env, following `test_log_stats_integration.py` / `test_query_deny_list.py`, and relying on
`start_services.py` (step 13) exporting a low `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` so the
daemon-pickup assertion below runs in seconds rather than minutes: `CREATE OR REPLACE`
a small view over `log_entries`, assert it appears in `list_view_sets()` and
`list_view_definitions()`; `assert_eventually` (timeout sized off that lowered refresh interval plus
one daemon minute tick, rather than the 60 s production default) that `list_partitions()` shows rows
for it *without* calling `materialize_partitions` first, confirming the daemon picks up a new view
set without a restart; create `a` over `log_entries`
at `update_group` 4000 (`count_src_query` counting `blocks`, per check 5) and `b` over `a` at 4001
(`extract_query` reading `a` and filtering on `a`'s event-time column; `count_src_query` also
counting `blocks`, not `a`), `materialize_partitions` both over the same range in
that order, and assert `b`'s rows equal a re-aggregation of `a`'s; `materialize_partitions` a known
range for the original view, `SELECT` from it, `REPLACE` it with a definition whose output schema
changes and assert the old partitions are no longer read (§3), then `DROP` and assert it is gone from
both listings. This covers the wiring no unit test reaches — the FlightSQL round trip, the real
migration, the real `ViewRegistry` → daemon → `lakehouse_partitions` chain — and the failures it
guards are silent (a view set that loads but never materializes looks like an empty table, not an
error; wrong numbers in a view reading another DDL view produce no error anywhere).

No new `#[ignore]` live-DB Rust test: per `CONTRIBUTING.md` those are reserved for pinning a bug
witnessed in the wild, and this is new-feature acceptance.

## Manual Verification

Each step below needs a running split-mode stack and is checking something no unit test can reach.

1. **Pre-upgrade `log_stats` partitions stay readable through the seeded definition.** Point
   `MICROMEGAS_SQL_CONNECTION_STRING` at a v9 database that already has materialized `log_stats`
   partitions and run `python3 local_test_env/ai_scripts/start_services.py`. Expect `upgrade
   lakehouse schema to v10` in `/tmp/analytics.log` and `SELECT version FROM lakehouse_migration` =
   10, then query `log_stats` for a time range predating the upgrade and confirm those partitions
   are still returned. Not automated because it needs a real pre-existing v9 database with
   materialized data, which the Milestone 3 e2e stack (a freshly created lake) does not have — it is
   the one property a fresh lake cannot exercise.
