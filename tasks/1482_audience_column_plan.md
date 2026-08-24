# First-class `audience` column on global-instance views Plan (#1482 — AbAC "physical boundary")

## Overview

Promote `micromegas.audience` from a process **property** resolved at query time to a physical,
**non-nullable**, dictionary-cheap **column** materialized on every lakehouse view that has a
`global` instance (`blocks`, `processes`, `streams`, `log_entries`, `measures`, `log_stats`). The
audience is extracted exactly once per partition write — from Postgres, at the `blocks` view's
materialization — and then propagates structurally into every downstream view, the same way
`processes.properties` already does today. `OwnershipRewrite` (Prong A) then filters those six
views with a direct, prune-friendly `audience IN (...)` predicate on their own column instead of
injecting a `process_id IN (SELECT ... FROM __processes__partitions ...)` semi-join whose subquery
runs `property_get(properties, 'micromegas.audience')` over dictionary-encoded JSONB for every
process row.

The column can be non-nullable because this plan also closes the last source of processes with
*no* audience: **every process gets one, always.** A new write-side knob,
`MICROMEGAS_DEFAULT_AUDIENCE` (default `public`), is stamped onto any process whose credential
carries no audience, and a one-time migration backfills the same value onto the legacy rows that
were never stamped. With "unstamped" gone as a state, the read-side fallback knob
`MICROMEGAS_UNSTAMPED_AUDIENCE` and Prong B's `OwnerAudience::Unstamped` variant are removed — a
default audience assigned at write time is the concept that survives, a query-time reinterpretation
of missing data is not.

This is not a correctness fix — all six views are already access-controlled. It buys: (1) the
semi-join and the per-row JSONB extraction disappear from every query plan touching those views;
(2) the audience travels with the rows it governs, removing today's cross-view
materialization-freshness dependency (a `log_entries` row is currently invisible until the
*separate* `processes` view has also caught up); (3) it unlocks the partition pruning and
per-audience object-storage prefixing that step 15 of
`tasks/data_isolation/audience_based_access_control_plan.md` lists as enabled-by this change; (4)
one fewer knob and one fewer state (`Unstamped`) in both enforcement prongs.

**All six views bump their file-schema hash** and are regenerated over the retention window. That
is deliberate: a non-nullable column cannot be null-filled onto pre-existing partitions by schema
evolution, and (see [Trade-offs](#trade-offs)) nothing is lost by the bump — lakehouse partitions
already expire at the same retention horizon as their Postgres sources, so everything the bump hides
is regenerable. The price is one `regenerate_partitions` pass per view and a window during which
un-regenerated history is invisible (fail-closed, never fail-open).

## Current State

### Where the audience lives today

Postgres `processes.properties` (`micromegas_property[]`, column DDL at
`rust/ingestion/src/sql_telemetry_db.rs:39`; stamped server-side at registration since Stage 5 /
#1373 by `WebIngestionService::insert_process` / `register_otel_process`,
`rust/ingestion/src/web_ingestion_service.rs`) is the single origin. Two readers consume it:

- **Prong B** (`rust/analytics/src/lakehouse/audience_guard.rs:141-177`) resolves it straight from
  Postgres with a `LEFT JOIN LATERAL (SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1)`,
  behind a TTL-bounded `moka` cache. A row with no property becomes `OwnerAudience::Unstamped`
  (`:112`), which `is_readable` (`:274-282`) admits only when `unstamped_audience` is configured
  and in the caller's scope. `AUDIENCE_PROPERTY` (`audience_guard.rs:49`) is the analytics-side
  constant; the ingestion writer stamps with
  `micromegas_telemetry::property_names::PROPERTY_AUDIENCE` (`rust/telemetry/src/property_names.rs:13`)
  — two definitions of the same literal today, which §1 collapses to one.
- **Prong A** (`rust/analytics/src/lakehouse/ownership_rewrite.rs`) reads the *materialized*
  copy. `OwnershipRewrite::audience_col()` (`:170-178`) is
  `cast(property_get(col("properties"), AUDIENCE_PROPERTY), Utf8)`, aggregated per process by
  `per_process_audience()` (`:184-197`) as `Aggregate(GROUP BY process_id, MAX(audience_col))` over
  the raw `__processes__partitions` scan, then filtered by `resolved_predicate()` (`:216-238`) as
  `coalesce(resolved_audience, unstamped_audience) IN (caller audiences)`.

`predicate_for()` (`:312-372`; signature takes `table_name: &TableReference` and
`mat_view: &MaterializedView`, so the view's file schema is in reach) branches per view set:

| Branch | View sets | Shape |
|---|---|---|
| §7 | anything in `public_view_sets` | no predicate |
| §3 | `processes` | `process_id IN (subquery)` against its own resolved aggregate |
| §4 | any view whose file schema has a `process_id` field | `cast(process_id, Utf8) IN (subquery)` semi-join |
| §5 | `async_events` | literal-valued `EXISTS` keyed on `view_instance_id` |
| §6 | `thread_spans` | two-hop literal `EXISTS` through `streams` |
| — | anything else | `Err(DataFusionError::Plan)` |

### Where "unstamped" comes from, and who depends on it

Unstamped processes are produced at exactly one place: `resolve_write_audience`
(`rust/public/src/servers/write_audience.rs`) returns `WriteAudience::none()` when the request's
`AuthContext.bound_audience` is `None` — env-keyring keys (`MICROMEGAS_API_KEYS`), OIDC
credentials, and `--disable-auth` (no `AuthContext` at all). Its five callers are the HTTP-edge
handlers in `rust/public/src/servers/{ingestion,otlp,webhook,firehose,firehose_cloudwatch_logs}.rs`.
`WriteAudience` (`rust/ingestion/src/write_audience.rs`) is `Option<Arc<str>>` with a deliberate
no-`Default` policy. The OTLP identity derivation folds the audience into `process_id`/`block_id`
(`rust/otel-ingestion/src/identity.rs:52-58`, `IdentityContext.audience: Option<&str>`; `None`
reproduces pre-Stage-5 ids).

The read side consumes it through `IsolationConfig.unstamped_audience: Option<String>`
(`rust/analytics/src/lakehouse/read_scope.rs:127-141`, parsed from
`{prefix}_UNSTAMPED_AUDIENCE` / `MICROMEGAS_UNSTAMPED_AUDIENCE` in `from_env`, `:214-`; default
`public` via `DEFAULT_UNSTAMPED_AUDIENCE`, `:115`; empty string ⇒ `None` ⇒ fail-closed). It rides
on `CallerContext.isolation_config` and is handed to **both** prongs by `query.rs`
(`AudienceGuard::new` at `:126-129`, `OwnershipRewrite::new` at `:335-339`). Startup sites:
`rust/monolith/src/main.rs:284` (`IsolationConfig::from_env("MICROMEGAS_ANALYTICS")`) and
`rust/public/src/servers/flight_sql_server.rs:315`.

Ingestion's conflict guard (`WebIngestionService::check_process_audience_conflict`,
`rust/ingestion/src/web_ingestion_service.rs:566-628`) already treats a process's audience as
write-once: a same-`process_id` re-registration under a different audience is rejected
(`AudienceConflict`), the same audience is a no-op, and an existing `NULL` is left alone
("no retro-stamp", `:624-631` — the known gap recorded at `CHANGELOG.md:40`). There is no
`UPDATE processes` anywhere in the tree.

### How the six global views are materialized

The propagation chain is already in place for `properties`; `audience` can ride it.

```
Postgres processes/streams/blocks
   │  BlocksView::data_sql  (blocks_view.rs:60-71)   ← one SQL SELECT, per insert-hour
   ▼
blocks view partitions  (schema: blocks_view_schema(), blocks_view.rs:237-306)
   │                                    │
   │ processes_view.rs transform        │ partition_source_data.rs::fetch_partition_source_data
   │ streams_view.rs transform          │   (:245-283) → PartitionSourceBlock{process, stream}
   ▼                                    ▼
processes / streams (SqlBatchView)    log_entries / measures (BlockPartitionSpec)
                                         │
                                         │ log_stats_view.rs transform (FROM log_entries)
                                         ▼
                                       log_stats (SqlBatchView)
```

Mechanics that constrain the design:

- **`blocks`** is a `MetadataPartitionSpec`: `fetch_metadata_partition_spec` runs `data_sql`
  against Postgres and `rows_to_record_batch` (`sql_arrow_bridge.rs:371-396`) builds the Arrow
  batch from the *PG column types and ordinals*, while the parquet file is written with
  `blocks_view_schema()` as its declared schema (`write_partition.rs:886, 926`). Column **names**
  therefore need not match (they already don't — `processes.properties as process_properties` in
  SQL vs. the `processes.properties` field), but **order and type must**. A PG `TEXT` column maps
  to nullable `Utf8` (`sql_arrow_bridge.rs:322-325`); the declared field's nullability governs
  the file. `blocks_file_schema_hash()` is hand-written (`blocks_view.rs:308-310`, currently
  `vec![3]`). `data_sql` is an `Arc<String>` built in `BlocksView::new` (`:59-71`).
- **The parquet write is positional and does not check nullability.** `AsyncArrowWriter` zips
  the declared schema's fields against the batch's columns with no name check
  (`write_partition.rs:926`, parquet `arrow_writer/mod.rs:1027-1035`), and a null under a
  required leaf is written as the type's default value, not rejected
  (`arrow_writer/levels.rs:655-690`). "Append last" is therefore load-bearing at every site below,
  and a declared `false` nullability is documentation until something enforces it (§1 adds that).
- **`processes`/`streams`/`log_stats`** are `SqlBatchView`s. Their schema is *inferred* from the
  transform query at startup (`sql_batch_view.rs:118-122`) and their file schema hash is a hash of
  that schema (`:296-300`) — so adding a column to a transform query bumps the hash automatically,
  no constant to edit. Each registers two tables (`:327-350`): the raw `__<name>__partitions`
  scan (what `OwnershipRewrite` actually rewrites — its predicates are qualified with that name)
  and the merged query under the bare name.
- **`log_entries`/`measures`** are `BlockPartitionSpec`s. Their schemas are hand-written
  (`log_entries_table.rs:24-83`, `metrics_table.rs:18-88`) with `const SCHEMA_VERSION`
  (`log_view.rs:37` = 6, `metrics_view.rs:39` = 7). Rows are built by
  `LogEntriesRecordBuilder`/`MetricsRecordBuilder` from `ProcessMetadata`
  (`metadata.rs:37-51`) — `fill_constant_columns` fills the per-block constant columns once per
  block. **Two** independent builder sets exist per view: the shared record builder and a
  hand-rolled duplicate in the OTel processors (`lakehouse/otel/logs_block_processor.rs:222-240`,
  `otel/metrics_block_processor.rs:373-397`). Nothing else builds these batches (exhaustive grep
  of `log_table_schema` / `metrics_table_schema` users).
- `ProcessMetadata` is built at three production sites: `process_metadata_from_row`
  (`metadata.rs:225-248`, from a PG row — used only by `find_process`, `:251-279`, the JIT /
  per-process path), `find_process_with_latest_timing` (`metadata.rs:283-386`, from the
  `processes` view; feeds only the span views), and `partition_source_data.rs:208-220` (from a
  `blocks` partition batch — the global-instance path). Test literals: `tests/test_helpers.rs:23`
  (`make_process_metadata`), `time_tests.rs:12, 55`, `block_chain_grouping_tests.rs:52`,
  `jit_partition_grouping_tests.rs:49`, `jit_freshness_tests.rs:49`,
  `jit_partition_bounds_tests.rs:45`.

### What a schema-hash bump costs

`MaterializedView::scan` fetches partitions by **exact** `file_schema_hash`
(`materialized_view.rs:73-81`, `partition_cache.rs:238`), so a bump makes every pre-existing
partition of that view invisible rather than automatically rebuilt; they come back through the
admin UDTF `regenerate_partitions(view_set_name, begin, end, partition_delta_seconds)`
(`lakehouse/regenerate_partitions_table_function.rs`, global instances only), or — for the short
trailing windows only — through the maintenance daemon's normal cycle (`CHANGELOG.md:152`, the
#1359 `measures` precedent).

Retention bounds the bill and rules out data loss. `delete_old_data` (`delete.rs:152`) deletes
Postgres `blocks`/`streams`/`processes` rows and the payload blobs (`delete.rs:38-41`) past
`MICROMEGAS_RETENTION_DAYS` (default 90, `rust/monolith/src/main.rs:161`) — and, in the same
maintenance tick, retires lakehouse partitions past the same horizon (`retire_expired_partitions`,
`delete.rs:166` → `write_partition.rs:86-135`, files then removed by
`delete_expired_temporary_files`). Parquet partitions therefore never outlive their sources, and
everything a bump hides is regenerable. The cost is (a) the regeneration itself — for
`log_entries`/`measures`, re-processing up to a full retention window of raw blocks — and (b) the
window during which un-regenerated history is invisible.

JIT (per-process / per-stream) instances rebuild on first query after a bump: `spec_is_up_to_date`
(`jit_partitions.rs:1177-1182`) treats a hash mismatch as stale — the #1429 / #1478 precedent
(`CHANGELOG.md:54`, `:76`).

## Design

### 0. The invariant: every process has an audience

Everything below rests on one statement that becomes true at deploy time and stays true:

> **Every row of Postgres `processes` carries a `micromegas.audience` property.**

Three mechanisms establish and keep it, each closing one way a `NULL` could appear:

1. **Write path — a default audience.** `MICROMEGAS_DEFAULT_AUDIENCE` (default `public`;
   validated against the `[A-Za-z0-9_-]{1,255}` charset; malformed ⇒ startup error, the same
   fail-fast `IsolationConfig::from_env` uses) is resolved once at ingestion-server startup and
   stored on `WebIngestionService` as `default_audience: WriteAudience`.
   `resolve_write_audience(ctx, default: &WriteAudience) -> WriteAudience` returns the credential's
   `bound_audience` when it has one and the default otherwise. `WriteAudience` becomes
   `WriteAudience(Arc<str>)`: `none()` is deleted, `as_str()` returns `&str`, and the compiler
   enumerates the five HTTP-edge callers plus every test that built an unstamped write.
   `IdentityContext.audience` (OTLP) becomes `&str`.
2. **Backfill — migration v8.** `upgrade_data_lake_schema_v8` appends
   `ROW('micromegas.audience', $1)::micromegas_property` to `processes.properties` for every row
   that lacks the key, with `$1` = the configured default audience:

   ```sql
   UPDATE processes
      SET properties = array_append(properties, ROW('micromegas.audience', $1)::micromegas_property)
    WHERE NOT EXISTS (SELECT 1 FROM unnest(properties) WHERE key = 'micromegas.audience');
   ```

   `migrate_db(pool)` gains the default-audience parameter (callers:
   `web_ingestion_service.rs:246`, `rust/monolith/src/main.rs:183`). Migration v6 (#1372,
   `sql_migration.rs:152-175`) is the precedent — it backfilled `ingestion_api_keys.audience` to
   the literal `'public'`; v8 uses the knob instead so a deployment that wants its legacy data
   under a different label sets `MICROMEGAS_DEFAULT_AUDIENCE` *before* upgrading and gets exactly
   that. `LATEST_DATA_LAKE_SCHEMA_VERSION` 7 → 8.
3. **Conflict guard — no `NULL` arm.** In `check_process_audience_conflict`, the
   `let Some(incoming) = audience.as_str() else { return Ok(()) }` early-out goes (there is no
   unstamped write any more), and the `None =>` "no retro-stamp" arm becomes an error: a row
   without the property after v8 is an invariant violation (something wrote to `processes`
   bypassing ingestion), and an `IngestionServiceError::DatabaseError` naming the `process_id` is
   the right fail-closed response. This closes the known gap at `CHANGELOG.md:40`.

Consequences worth stating plainly:

- **Deployments that were unstamped become default-audience.** Under the default (`public` on
  both the old read knob and the new write knob) nothing observable changes: what was "unstamped,
  visible to `public`" is now "stamped `public`, visible to `public`". An operator who had set
  `MICROMEGAS_UNSTAMPED_AUDIENCE` to a non-default label, or to the empty string (fail-closed),
  must pick a `MICROMEGAS_DEFAULT_AUDIENCE` before upgrading — for the fail-closed case, a label
  that no principal is granted (e.g. `unassigned`). The startup check in §4 makes forgetting this
  loud rather than silent.
- **OTLP `process_id`s churn once** in previously-unstamped deployments: the audience is folded into
  the id (`identity.rs:52-58`), so a resource that produced id X unstamped produces id Y stamped
  `public`. The old row is backfilled to `public` by v8, the new row is stamped `public` by the
  writer; they are distinct processes and the conflict guard has nothing to say. The churn is the
  same shape #1373 already documented for deployments that switched from unstamped to keyed
  ingestion (`CHANGELOG.md:41`).
- `public` remains the sole built-in read grant every authenticated principal holds, so a
  default-audience process is exactly as visible as an unstamped one was under the default knob.

### 1. One extraction site: the `blocks` view's Postgres query

`BlocksView::data_sql` and `blocks_view_schema()` each gain one trailing entry:

```sql
   ...,
   processes.properties as process_properties,
   (SELECT value FROM unnest(processes.properties) WHERE key = 'micromegas.audience' LIMIT 1) AS audience
 FROM blocks, streams, processes
 ...
```

```rust
Field::new("audience", DataType::Utf8, false),   // appended last, NOT NULL
```

Both appends are last and move in lock-step (positional write, Current State). The batch column
arrives as nullable `Utf8` from the PG bridge; the declared field is non-nullable. **The parquet
writer does not enforce that.** For a required top-level leaf `ArrowLevels` has no definition
levels and `write_leaf` treats every index as non-null (`parquet-58.3.0/src/arrow/arrow_writer/levels.rs:655-690`),
so a `NULL` would be silently written as `""` — a mislabelled row, not an error. Therefore
`write_partition_from_rows` (`write_partition.rs:886-`) gains a **nullability guard** before
`AsyncArrowWriter::write`: for every declared non-nullable field, `column.null_count() == 0` or
the write fails with an error naming the view and column. It is one `null_count()` per column
per batch, it protects every view rather than just this one, and it turns a violated §0 invariant
(something wrote to `processes` bypassing ingestion) into a loud, fail-closed materialization
error instead of a silently `""`-labelled row.

The correlated scalar subselect runs once per block row over an insert-hour window (unlike
`audience_guard.rs`'s point lookups). `unnest` of a handful of properties per row is cheap, but
the `LEFT JOIN LATERAL (...) aud ON true` form `audience_guard.rs:141-177` already uses is an
equivalent alternative if the planner disagrees — either way the fragment comes from
`audience_subselect()` below, so switching is a one-site change.

**One constant.** A new top-level `rust/analytics/src/audience.rs` re-exports the telemetry
constant (`pub use micromegas_telemetry::property_names::PROPERTY_AUDIENCE as AUDIENCE_PROPERTY;`)
so the writer and both readers share one literal; `audience_guard.rs` drops its own definition.
Three consumers in `analytics`: `audience_guard.rs`, `blocks_view.rs`, `metadata.rs` (the last is
not under `lakehouse`, hence top-level). Alongside it, the fragment the two new SQL sites share:

```rust
/// `(SELECT value FROM unnest(<properties_expr>) WHERE key = '<AUDIENCE_PROPERTY>' LIMIT 1)`
pub fn audience_subselect(properties_expr: &str) -> String;
```

`data_sql` is built with `format!` around `audience_subselect("processes.properties")` so the
property name appears nowhere but the constant. `audience_guard.rs`'s `LEFT JOIN LATERAL` form is
equivalent; leave it (optional DRY follow-up).

### 2. Propagation into `processes` / `streams` / `log_stats`

Append one aggregate to each transform **and** merge query:

- `processes_view.rs`: `max("audience") as audience`, appended **last** in the SELECT list of both
  the transform (`:25-45`, after `last_block_end_time`) and the merge query (`:46-67`), so the
  inferred schema grows only at the end.
- `streams_view.rs`: same, after `last_update_time` (transform `:25-38`, merge `:39-53`).
- `log_stats_view.rs`: `arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience`,
  appended after `count` in both the transform (`:32-45`) and the merge query (`:50-59`). The cast
  is not optional: `max` **coerces a dictionary input to its value type** (`min_max.rs:58-77`,
  `coerce_types` at `:646`), so a bare `max(audience)` would infer `Utf8` — unlike `process_id`
  in the same view, which stays `Dictionary(Int32, Utf8)` only because it is a `GROUP BY` key. The
  cast keeps the view's two process-scoped columns the same type and the column dictionary-cheap.
  Left out of the `GROUP BY` on purpose: audience is functionally determined by `process_id`, so
  grouping on it cannot change row counts, and leaving the key list alone keeps the declared
  `(time_bin, process_id, level, target)` merge sort order (`log_stats_view.rs:84-89`) exactly as
  it is (`SqlBatchView` only requires declared sort columns to be merge `GROUP BY` keys,
  `sql_batch_view.rs:155-162`). Transform and merge must infer the same type — the file schema is
  fixed by the transform and the merge writes positionally.

`max()` rather than the neighbouring `first_value()` only because it is order-independent; with no
`NULL`s in the source there is nothing to outrank, and every row for a process carries the same
value (§5). **Inferred nullability**: DataFusion declares every aggregate output nullable, so on
these three views the column is *declared* nullable although it never holds a `NULL`. That is a
property of `SqlBatchView` schema inference, not of the data; document it as such.

### 3. Propagation into `log_entries` / `measures`

- `ProcessMetadata` (`metadata.rs:37-51`) gains `pub audience: Arc<str>`. A required field with
  no default, so the compiler enumerates every construction site (the project's stated preference
  in `CLAUDE.md`'s "Interface stability" section).
  - `process_metadata_from_row` — add `audience` to `find_process`'s SELECT (`:257-271`) using
    `audience_subselect("properties")`, read it as `String` (a `NULL` is a `try_get` error —
    correct, per §0). This is the JIT / per-process path.
  - `find_process_with_latest_timing` — add `audience` to its `SELECT ... FROM processes`
    (`:307-313`; the view now has the column) and read it with `string_column_by_name`.
  - `partition_source_data.rs` — add `"audience"` to `fetch_partition_source_data`'s projection
    list (`:253-262`) and read it with `string_column_by_name` at `:208-220`. The source column is
    non-nullable, so no null check is needed (note for the future: `StringColumnAccessor::value`
    does *not* null-check — `dfext/string_column_accessor.rs:30-40` — which is why the column being
    `NOT NULL` at the source matters here). This is the global-instance path.
- `log_table_schema()` / `metrics_table_schema()` each gain a trailing
  `Field::new("audience", Dictionary(Int32, Utf8), false)` — dictionary-encoded, matching the
  `process_id` treatment in the same schemas and giving the filter the cheap key comparison the
  issue asks for.
- `LogEntriesRecordBuilder` / `MetricsRecordBuilder` gain an
  `audiences: StringDictionaryBuilder<Int32Type>`: `append_value` in `append`, `append_n(a, n)` in
  `fill_constant_columns`. Same two lines in the two hand-rolled OTel builders. In all four the new
  array goes **last** in the `finish` column vector (positional write).
- `SCHEMA_VERSION`: `log_view.rs` 6 → 7, `metrics_view.rs` 7 → 8. Those two files change only that
  constant — they delegate everything else to the `*_table_schema()` functions.
- `blocks_file_schema_hash()`: `vec![3]` → `vec![4]`.

### 4. Removing `MICROMEGAS_UNSTAMPED_AUDIENCE` and `OwnerAudience::Unstamped`

- `IsolationConfig` (`read_scope.rs`) loses `unstamped_audience` and `DEFAULT_UNSTAMPED_AUDIENCE`;
  it keeps `public_view_sets`. `from_env` **errors** if `{prefix}_UNSTAMPED_AUDIENCE` or
  `MICROMEGAS_UNSTAMPED_AUDIENCE` is set — "removed in <version>; assign legacy data an audience
  with `MICROMEGAS_DEFAULT_AUDIENCE` on the ingestion side" — rather than silently ignoring a knob
  an operator may be relying on for fail-closed behaviour. The `resolved_var` helper already
  centralizes the prefix fallback.
- `OwnershipRewrite::new` and `AudienceGuard::new` lose their `unstamped_audience` parameter
  (`query.rs:126-129`, `:335-339` are the callers).
- `audience_guard.rs`: delete `OwnerAudience::Unstamped`; `merge_owner_rows` (`:107-118`) maps a
  `None` audience to `OwnerAudience::Unknown` — after v8 a `NULL` means "no such row" as far as
  access is concerned, and `Unknown` is already always-denied. `is_readable` (`:274-`) loses the
  `unstamped_audience` argument and its `Unstamped` arm; the module doc's prong-divergence
  discussion is rewritten (§7 below).
- `ownership_rewrite.rs`: `resolved_predicate()` drops the `coalesce` — it is
  `resolved_audience IN (caller audiences)`, `lit(false)` on an empty set as today.

### 5. `OwnershipRewrite`: a new first branch, keyed on the column's presence

`predicate_for` gains a branch ahead of §3/§4, keyed on
`view.get_file_schema().field_with_name("audience").is_ok()` — the same schema-introspection
style as the existing `process_id` test (`:344`), so a view set that gains the column later (the
JIT views, see [Future work](#future-work)) upgrades automatically with no edit here:

```rust
// §2 (new): views carrying a physical `audience` column -- processes, streams, blocks,
// log_entries, measures, log_stats. Filtered directly, no semi-join, no property_get.
if view.get_file_schema().field_with_name("audience").is_ok() {
    return Ok(Some(self.audience_column_predicate(table_name)));
}
```

with

```rust
/// `audience IN (caller audiences)`; `false` for an empty set (fail-closed, as
/// `resolved_predicate` already does). The column is NOT NULL, so there is no unstamped case.
fn audience_column_predicate(&self, table_name: &TableReference) -> Expr {
    let audiences = self.audiences();
    if audiences.is_empty() {
        return lit(false);
    }
    let raw = Expr::Column(Column::new(Some(table_name.clone()), "audience"));
    // Cast for the same reason every other expression in this file is cast: this rule runs
    // after DataFusion's own TypeCoercion pass, and `audience` is Dictionary(Int32, Utf8) in
    // log_entries/measures/log_stats but Utf8 in blocks/processes/streams.
    cast(raw, DataType::Utf8).in_list(
        audiences.iter().map(|a| lit(ScalarValue::Utf8(Some(a.clone())))).collect(),
        false,
    )
}
```

`table_name` here is the resolved scan — `__processes__partitions`, `__streams__partitions`, and
so on for the `SqlBatchView`s — the same qualifier the existing `process_id` predicate uses
(`:334`). The `IN` list is the shape Parquet's `PruningPredicate` can evaluate against row-group
statistics; whether pruning actually engages through the `cast` on dictionary views is for the
pruning follow-up to verify, not a claim this change makes.

Consequences inside the file:

- **§3 (`processes`) is deleted.** `processes` now carries the column and falls into the new
  branch, as the first member rather than a special case.
- **§4 shrinks** to the views that still have `process_id` but no `audience`: `net_spans`,
  `otel_spans`, `images` (all three carry `process_id`: `net_spans_table.rs:44`,
  `images_table.rs:17`, `otel/spans_table.rs:12`). It keeps today's semi-join, unchanged.
- **§5/§6 keep their `EXISTS` shapes**, but `per_process_audience()` now aggregates
  `max(col("audience"))` off `__processes__partitions` instead of
  `max(property_get(properties, ...))`. `audience_col()` and the `PropertyGet` /
  `AUDIENCE_PROPERTY` imports are removed from this file. `processes_source`/`streams_source` and
  their `make_session_context` plumbing stay — §4/§5/§6 still need them.
- The module doc comment's branch table and its "One audience per process, not per row" section
  are rewritten (see [Documentation](#documentation)). The aggregate in that section exists because
  `__processes__partitions` can hold several rows per `process_id` — one per materialized
  partition. That is still true; what changes is that the rows can no longer *disagree* (§6), so
  filtering them one at a time is sound.

### 6. Why per-row filtering is sound

The new branch filters rows individually. That is sound because **a process's audience is
write-once and always present**: it is written at registration (or by v8), there is no
`UPDATE processes` path in the tree, and the conflict guard rejects a same-`process_id`
re-registration under a different audience outright (§0). Every materialized row for a given
process therefore snapshots the same, non-null Postgres value, so per-row and `MAX`-per-process
filtering agree. Note that §2's `max()` is *not* what makes this safe — it collapses rows within one
partition only; the invariant does the work.

One edge case, documented rather than defended against: **retention-delete then re-register**
(possible for OTLP, whose `process_id` derivation is deterministic within an audience). Old
partitions carry the old audience, new ones the new one — and because the audience is part of the
OTLP id, they are different `process_id`s, so even the aggregate paths (§5/§6) see two processes,
not one with two labels. Per-row filtering hands each row to the audience that produced it.

### 7. Migration: bump all six, regenerate over the retention window

**Hashes.** `blocks_file_schema_hash()` `vec![3]` → `vec![4]`; `log_view.rs` `SCHEMA_VERSION`
6 → 7; `metrics_view.rs` 7 → 8; `processes`/`streams`/`log_stats` bump automatically with their
inferred schema. On deploy every pre-existing partition of the six views goes invisible.

**Order of operations on the ingestion side**: migration v8 runs at ingestion-service startup
(`migrate_db`) before any request is served, and the writer stamps the default from its first
request, so the §0 invariant holds before the first post-deploy partition is written. Set
`MICROMEGAS_DEFAULT_AUDIENCE` first if the default is not what legacy data should be labelled.

**Regeneration**, in dependency order, each over `[now - MICROMEGAS_RETENTION_DAYS, now]`:

| Step | View | Source | Cost |
|---|---|---|---|
| 1 | `blocks` | Postgres | cheap — metadata-sized |
| 2 | `processes`, `streams` | `blocks` partitions (new hash) | cheap — one row per process/stream per partition |
| 2 | `log_entries`, `measures` | `blocks` partitions (new hash) + payload blobs | **the expensive ones** — re-parses every retained block |
| 3 | `log_stats` | `log_entries` partitions (new hash) | re-aggregates all `log_entries` into 1-minute bins |

`blocks` must go first: `fetch_partition_source_data` selects source blocks by the current
`blocks` hash (`partition_source_data.rs:267`), so `log_entries`/`measures` regenerated before
`blocks` would see no sources. Step 2's four views are independent of each other. The maintenance
daemon re-materializes the trailing windows on its own; `regenerate_partitions` covers the rest.
Until a view's regeneration completes, queries against it return only post-deploy data — a visible
gap, never a leak.

**Nothing is lost.** Lakehouse partitions expire at the same horizon as their Postgres sources
(Current State), so every partition the bump hides is one whose sources still exist.

## Implementation Steps

### Phase 1 — every process has an audience (ingestion)

1. `rust/ingestion/src/write_audience.rs`: `WriteAudience(Arc<str>)`; delete `none()`;
   `as_str() -> &str`; add `pub fn default_from_env() -> anyhow::Result<WriteAudience>` reading
   `MICROMEGAS_DEFAULT_AUDIENCE` (default `public`, validated, fail-fast).
2. `rust/ingestion/src/web_ingestion_service.rs`: `default_audience: WriteAudience` field, set at
   construction; `check_process_audience_conflict` per §0.3; `migrate_db(pool, default_audience)`.
3. `rust/ingestion/src/sql_migration.rs`: `upgrade_data_lake_schema_v8` (backfill),
   `LATEST_DATA_LAKE_SCHEMA_VERSION` = 8; `remote_data_lake.rs` threads the parameter.
4. `rust/public/src/servers/write_audience.rs`: `resolve_write_audience(ctx, default)`; update
   the five callers to pass the service's default.
5. `rust/otel-ingestion/src/identity.rs`: `IdentityContext.audience: &str`.
6. `rust/monolith/src/main.rs`, `rust/telemetry-ingestion-srv`: resolve the default at startup
   and pass it through.
7. Compile fallout in `rust/ingestion/tests/{write_audience_tests,audience_stamping_db_test,process_audience_cache_test}.rs`,
   `rust/public/tests/resolve_write_audience_tests.rs`,
   `rust/otel-ingestion/tests/{identity_tests,split_tests}.rs`; add the v8 backfill test.

### Phase 2 — the column, materialized

8. New `rust/analytics/src/audience.rs`: re-export `PROPERTY_AUDIENCE` as `AUDIENCE_PROPERTY`,
   add `audience_subselect()`. Update `lib.rs`; `audience_guard.rs` and `ownership_rewrite.rs`
   import from here.
9. `lakehouse/blocks_view.rs`: `format!` the subselect into `data_sql`, append
   `Field::new("audience", Utf8, false)` to `blocks_view_schema()`, `blocks_file_schema_hash()`
   → `vec![4]`. `lakehouse/write_partition.rs`: the non-nullable-column guard in
   `write_partition_from_rows` (§1).
10. `lakehouse/processes_view.rs`, `lakehouse/streams_view.rs`: append `max("audience") as audience`
    to transform + merge queries.
11. `metadata.rs`: `ProcessMetadata::audience: Arc<str>`; populate it in
    `process_metadata_from_row` (extend `find_process`'s SELECT) and in
    `find_process_with_latest_timing` (extend its SELECT).
12. `lakehouse/partition_source_data.rs`: add `"audience"` to the projection and read it into
    `ProcessMetadata`.
13. `log_entries_table.rs`, `metrics_table.rs`: schema field (non-nullable, last) + builder field +
    `append` / `fill_constant_columns` / `finish`.
14. `lakehouse/otel/logs_block_processor.rs`, `lakehouse/otel/metrics_block_processor.rs`: the same
    two lines in each hand-rolled builder, array last in the column vector.
15. `lakehouse/log_view.rs` `SCHEMA_VERSION` 6 → 7; `lakehouse/metrics_view.rs` 7 → 8.
16. `lakehouse/log_stats_view.rs`: append `arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience`
    to transform + merge queries.
17. Compile fallout in the test fixtures that build `ProcessMetadata` literals
    (`tests/test_helpers.rs`, `time_tests.rs`, `block_chain_grouping_tests.rs`,
    `jit_partition_grouping_tests.rs`, `jit_freshness_tests.rs`, `jit_partition_bounds_tests.rs`).

At the end of this phase the column exists and is populated; `OwnershipRewrite` still uses the
semi-join, so the change is observable only as a new column in `SELECT *`.

### Phase 3 — enforcement: switch to the column, remove the unstamped state

18. `lakehouse/read_scope.rs`: remove `unstamped_audience` / `DEFAULT_UNSTAMPED_AUDIENCE`; `from_env`
    errors on a set `*_UNSTAMPED_AUDIENCE`.
19. `lakehouse/audience_guard.rs`: remove `OwnerAudience::Unstamped`, the `unstamped_audience`
    parameter, and the `is_readable` arm; `None` audience ⇒ `Unknown`.
20. `lakehouse/ownership_rewrite.rs`: add `audience_column_predicate` and the new branch; delete
    §3 and `audience_col()`; repoint `per_process_audience()` at `col("audience")`; drop the
    `coalesce` from `resolved_predicate`; drop the `PropertyGet` / `AUDIENCE_PROPERTY` imports and
    the `unstamped_audience` field; rewrite the module doc comment.
21. `lakehouse/query.rs`: drop the `unstamped_audience` arguments at both construction sites.
22. Tests — see [Testing Strategy](#testing-strategy): `ownership_rewrite_public_view_set_tests.rs`
    (restructure `real_view_factory_covers_every_registered_view_set`),
    `ownership_rewrite_config_tests.rs` (removal semantics), `ownership_rewrite_db_test.rs`,
    `prong_b_guard_db_test.rs`, `audience_guard_tests.rs`, `tests/common/db_fixtures.rs`
    (delete `caller_with_unstamped_audience`).

### Phase 4 — docs and changelog

23. Documentation updates listed below, plus the CHANGELOG entry with its **Operational note**,
    **Minor breaking change** clause, and the removed-env-var notice.
24. Mark step 15 of `tasks/data_isolation/audience_based_access_control_plan.md` as landed and
    point it at this plan.

## Files to Modify

**New**
- `rust/analytics/src/audience.rs`

**Ingestion — default audience and backfill**
- `rust/ingestion/src/write_audience.rs`
- `rust/ingestion/src/web_ingestion_service.rs`
- `rust/ingestion/src/sql_migration.rs`, `rust/ingestion/src/remote_data_lake.rs`
- `rust/public/src/servers/write_audience.rs` and its five callers
  (`ingestion.rs`, `otlp.rs`, `webhook.rs`, `firehose.rs`, `firehose_cloudwatch_logs.rs`)
- `rust/otel-ingestion/src/identity.rs`
- `rust/monolith/src/main.rs`, `rust/telemetry-ingestion-srv/src/main.rs`

**Analytics — materialization**
- `rust/analytics/src/lakehouse/blocks_view.rs`
- `rust/analytics/src/lakehouse/write_partition.rs` (nullability guard)
- `rust/analytics/src/lakehouse/processes_view.rs`
- `rust/analytics/src/lakehouse/streams_view.rs`
- `rust/analytics/src/lakehouse/log_stats_view.rs`
- `rust/analytics/src/lakehouse/log_view.rs`, `rust/analytics/src/lakehouse/metrics_view.rs` (`SCHEMA_VERSION` only)
- `rust/analytics/src/lakehouse/partition_source_data.rs`
- `rust/analytics/src/lakehouse/otel/logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel/metrics_block_processor.rs`
- `rust/analytics/src/log_entries_table.rs`
- `rust/analytics/src/metrics_table.rs`
- `rust/analytics/src/metadata.rs`
- `rust/analytics/src/lib.rs` (module declaration)

**Analytics — enforcement**
- `rust/analytics/src/lakehouse/ownership_rewrite.rs`
- `rust/analytics/src/lakehouse/audience_guard.rs`
- `rust/analytics/src/lakehouse/read_scope.rs`
- `rust/analytics/src/lakehouse/query.rs`
- `rust/public/src/servers/flight_sql_server.rs` (doc comment at `:157-160`)

**Tests**
- `rust/analytics/tests/ownership_rewrite_db_test.rs`
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`
- `rust/analytics/tests/ownership_rewrite_config_tests.rs`
- `rust/analytics/tests/prong_b_guard_db_test.rs`, `audience_guard_tests.rs`
- `rust/analytics/tests/common/db_fixtures.rs`
- `rust/analytics/tests/test_helpers.rs`, `time_tests.rs`, `block_chain_grouping_tests.rs`,
  `jit_partition_grouping_tests.rs`, `jit_freshness_tests.rs`, `jit_partition_bounds_tests.rs`
- `rust/ingestion/tests/write_audience_tests.rs`, `audience_stamping_db_test.rs`,
  `process_audience_cache_test.rs`; `rust/public/tests/resolve_write_audience_tests.rs`;
  `rust/otel-ingestion/tests/identity_tests.rs`, `split_tests.rs`

**Docs**
- `mkdocs/docs/query-guide/schema-reference.md`
- `doc/how_to_query/README.md`
- `mkdocs/docs/admin/authentication.md`, `ingestion.md`, `api-keys.md`, `flight-sql.md`,
  `monolith.md`, `functions-reference.md`
- `rust/analytics/src/lakehouse/view_factory.rs` (module doc schema tables)
- `CHANGELOG.md`
- `tasks/data_isolation/audience_based_access_control_plan.md`

Explicitly **not** modified: the Postgres `processes` table shape (`sql_telemetry_db.rs`) — the
audience stays a property, see Trade-offs; `net_spans`/`otel_spans`/`images`/`async_events`/
`thread_spans` views — out of scope per the issue; `tests/blocks_view_merge_ordering_tests.rs`,
which only passes `blocks_view_schema()` as a schema argument and needs no change.

## Trade-offs

- **Nullable column + DataFusion schema evolution, no bump on the big three** (the previous draft
  of this plan). DataFusion 54.1's `DefaultPhysicalExprAdapterFactory` null-fills a nullable
  column missing from a parquet file, so `blocks`/`log_entries`/`measures` could have kept their
  hashes and read `audience` as `NULL` on old partitions. **Rejected**, for three reasons that
  compound: (a) `NULL` would mean two things — "process never stamped" and "row predates the
  column" — and the enforcement predicate would need an `OR audience IS NULL` disjunct whenever the
  unstamped default is in scope, i.e. in practically every plan, permanently defeating audience
  pruning on old partitions; (b) soundness would rest on an *operational* precondition (ship the
  column before minting the first restricted key) rather than on the data; and (c) the argument
  for it — permanent history loss on a bump — turned out to be false: lakehouse partitions already
  expire with their sources (`retire_expired_partitions`), so a bump costs regeneration time, not
  data. A non-nullable column with a full regeneration is the simpler system by a wide margin.
- **Default audience vs. keeping `MICROMEGAS_UNSTAMPED_AUDIENCE`.** The knob could have stayed as
  the value coalesced in at materialization time. Rejected: it would bake a read-time policy value
  into data columns (changing the knob later would reinterpret nothing already written), it keeps
  `Unstamped` alive as a state in Prong B with a different treatment from Prong A, and "what
  audience does data with no explicit audience get" is a *write*-side question — answered once, at
  ingestion, it never has to be asked again. The v8 backfill is what lets the read side forget the
  concept entirely.
- **Backfill with the knob vs. the literal `'public'`.** v6 used the literal for keys. v8 takes the
  configured default so a fail-closed deployment can route legacy data to a label nobody is granted
  instead of silently publishing it; the cost is one parameter threaded into `migrate_db`.
- **Fail-fast on a set `*_UNSTAMPED_AUDIENCE` vs. ignore it.** Ignoring is the usual treatment of a
  retired var, but this one may be load-bearing for an operator's fail-closed posture; a startup
  error with a pointer to `MICROMEGAS_DEFAULT_AUDIENCE` is the safer default.
- **`max()` vs. `first_value()` in the `SqlBatchView` transforms.** Interchangeable now that the
  source has no `NULL`s; `max` is order-independent, which is one fewer thing to reason about.
- **Extract once in the `blocks` SQL vs. per-view `property_get` in each transform query.**
  Rejected: four extraction sites instead of one, for nothing — `blocks` is bumping regardless.
- **No Postgres `processes.audience` column.** A stored column written by ingestion would make the
  extraction free and would be a natural input to a Stage 5b write-path check. Deferred: it is a
  schema migration on the ingestion side that belongs with Stage 5b's own design, and the
  query-side extraction it would optimize now happens once per partition rather than once per
  query. (`tasks/completed/1373_ingestion_stamping_plan.md` §7 defers 5b and argues for resolving
  the owning audience through the existing `moka` caches rather than denormalizing — that plan
  should weigh the column against the cache, not assume it.)

## Documentation

- `mkdocs/docs/query-guide/schema-reference.md` — add the `audience` row to the `processes`
  (`:25`), `streams` (`:62`), `blocks` (`:91`), `log_entries` (`:151`), `log_stats` (`:201`), and
  `measures` (`:264`) field tables (last row, matching physical order). This is a **documented,
  stable column**, so the prose should say:
  - what it is: the audience of the owning process, written server-side from the authenticated
    ingestion credential or the deployment's `MICROMEGAS_DEFAULT_AUDIENCE`; never client-settable;
    never `NULL` in the data.
  - `Utf8` on `processes`/`streams`/`blocks`, `Dictionary(Int32, Utf8)` on
    `log_entries`/`measures`/`log_stats`. Both compare against string literals normally; the
    difference matters only to a client reading Arrow types directly (the
    `preserve_dictionary=True` python path). On the three `SqlBatchView`s the field is *declared*
    nullable by schema inference; it never holds a `NULL`.
  - it is **not a filter the user needs to apply**: enforcement is unconditional, and a caller can
    only ever see rows whose audience is in their read scope. The column is for observability —
    "whose data is this, how much of each" — not for a user-authored access check.
- `doc/how_to_query/README.md` — carries five of the six tables (`processes` `:248`, `streams`
  `:268`, `blocks` `:282`, `measures` `:351`, `log_entries` `:371`; **no `log_stats`**), and its
  `processes`/`streams` tails are already stale (missing `last_block_end_ticks`/
  `last_block_end_time` and `streams.format`). Bring those tails up to date *before* appending
  `audience`, so "last" is actually last, and add a `log_stats` table or a pointer to
  `schema-reference.md`.
- `mkdocs/docs/admin/authentication.md` — "Audience Filtering Activation" (`:152-190`) and
  "Write-Side Stamping" (`:207`): the audience is a physical column on the global views; the
  query-time property lookup is gone from those plans; `MICROMEGAS_UNSTAMPED_AUDIENCE` is removed
  and `MICROMEGAS_DEFAULT_AUDIENCE` replaces it on the write side; the "two prongs read different
  copies" paragraph (`:184-190`) is rewritten — both copies are now non-null and write-once, and
  the remaining skew is materialization latency only.
- `mkdocs/docs/admin/ingestion.md` — "What gets stamped" (`:70-92`): the env-keyring / OIDC /
  `--disable-auth` bullets now say "stamped with `MICROMEGAS_DEFAULT_AUDIENCE`"; add the var to
  the ingestion env table; the OTLP id-churn note.
- `mkdocs/docs/admin/flight-sql.md:33`, `monolith.md:51` — remove the `*_UNSTAMPED_AUDIENCE` rows;
  add `MICROMEGAS_DEFAULT_AUDIENCE` to the monolith table (ingestion role).
- `mkdocs/docs/admin/api-keys.md:271-296`, `functions-reference.md:75` — the "unstamped ... visible
  through `MICROMEGAS_UNSTAMPED_AUDIENCE`" phrasing → default audience.
- `rust/analytics/src/lakehouse/view_factory.rs` — the module doc's per-view schema tables
  (`log_entries` `:11`, `measures` `:29`, `processes` `:126`, `streams` `:145`, `blocks` `:159`;
  no `log_stats` table — add one or note the omission). The `blocks` and `processes` tables there
  are already stale (missing `insert_time`-suffixed columns, `parent_process_id`,
  `last_update_time`, `last_block_end_*`); fix the tails while appending.
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — module doc: new branch table; the "One
  audience per process, not per row" section rewritten around §6 (the aggregate is retained for
  §5/§6 because partitions still hold several rows per process, but the rows can no longer
  disagree; per-row filtering on the column is sound for the same reason); `audience_col` /
  `property_get` / `unstamped_audience` gone.
- `rust/analytics/src/lakehouse/audience_guard.rs` — module doc: `Unstamped` gone, `Unknown` covers
  a missing row *or* (post-v8, invariant violation) a missing property.
- `CHANGELOG.md` — Unreleased → Analytics and Ingestion, following the `:54`/`:76`/`:152`
  precedents:
  - **Operational note**: all six global views bump their file-schema hash; run
    `regenerate_partitions` over the retention window in the order given in §7 (`blocks` first,
    then `processes`/`streams`/`log_entries`/`measures`, then `log_stats`); until then those views
    show post-deploy data only. Migration v8 backfills `micromegas.audience` onto never-stamped
    processes with `MICROMEGAS_DEFAULT_AUDIENCE` — set it before upgrading if `public` is not the
    label legacy data should carry. OTLP `process_id`s churn once in previously-unstamped
    deployments.
  - **Removed**: `MICROMEGAS_UNSTAMPED_AUDIENCE` / `{prefix}_UNSTAMPED_AUDIENCE`; startup fails if
    set. Replaced by `MICROMEGAS_DEFAULT_AUDIENCE` on the ingestion side.
  - **Minor breaking change**: `ProcessMetadata` gains a required `audience: Arc<str>` field;
    `WriteAudience` is no longer optional (`none()` removed, `as_str() -> &str`);
    `resolve_write_audience` takes the default; `migrate_db` takes the default;
    `OwnershipRewrite::new` / `AudienceGuard::new` lose `unstamped_audience`;
    `OwnerAudience::Unstamped` removed; `IsolationConfig.unstamped_audience` removed.
  - Closes the `CHANGELOG.md:40` known gap (conflict guard's `NULL`→no-op branch).

## Testing Strategy

- **`tests/ownership_rewrite_db_test.rs`** is the acceptance vehicle: it seeds three processes
  through the real `WebIngestionService` with distinct `WriteAudience`s — one of them unstamped
  (`:82-93`, `:251-253`) — materializes `blocks` → `processes` → `streams`, and asserts visible
  rows per `ReadScope` (`:298-557`). The unstamped process becomes a *default-audience* process
  (`WriteAudience::none()` no longer exists); its assertions become: visible to a caller holding the
  default audience, invisible to one that does not. The stamped processes' assertions pass
  **unchanged** — the primary evidence the semantics did not move. Add: assertions that `audience`
  is present, non-null, and carries the expected value on each of the six views.
- **`tests/ownership_rewrite_public_view_set_tests.rs`** — `real_view_factory_covers_every_registered_view_set`
  (`:379-464`) enumerates every `default_view_factory()` view set and asserts each plan contains
  `LeftSemi Join`; six now produce a bare `Filter`. Restructure it into two expectations keyed on
  whether the view's file schema has `audience`: `Filter` on `audience IN (...)` and **no** join /
  no `property_get` for the six (the regression test for the optimization itself), the semi-join
  for the rest. Update the per-view shape assertions for `streams` (`:263-277`), `processes`
  (`:311-329`), and the empty-audience `EmptyRelation` case (`:290-307`).
- **`tests/ownership_rewrite_config_tests.rs`** — the `*_UNSTAMPED_AUDIENCE` parsing cases become
  one: a set var is a startup error naming `MICROMEGAS_DEFAULT_AUDIENCE`. Keeps its `#[serial]` +
  `EnvGuard` pattern (`:26-39`).
- **Prong B**: `prong_b_guard_db_test.rs` / `audience_guard_tests.rs` unstamped cases → deleted or
  converted to default-audience; add one asserting a `None` audience row resolves to `Unknown`
  (denied).
- **Unit-level**: a pure test over `audience_column_predicate` for the empty and non-empty
  audience sets, and one over `WriteAudience::default_from_env` (unset ⇒ `public`, malformed ⇒
  `Err`).
- **Migration v8**: a DB-backed test in `rust/ingestion/tests/` (the SQL is the thing under test)
  — insert a stamped and an unstamped process, run the migration, assert the unstamped one now
  carries the configured default and the stamped one is untouched. `sql_migration.rs:339`'s
  latest-version assertion updates to 8.
- **Non-nullability is enforced at write**: in `tests/write_partition_tests.rs` (the
  `AsyncArrowWriter` + `InMemory`-store seam), write a batch with a `NULL` in a declared
  non-nullable column and assert `write_partition_from_rows` fails naming the column, and that the
  same batch with the field declared nullable succeeds. This pins the guard §1 adds — without it
  parquet writes the null as `""`, which is exactly the silent mislabelling the guard exists to
  prevent.
- **Regeneration rehearsal**: against a stack with pre-change partitions, deploy, confirm the six
  views show post-deploy data only, run `regenerate_partitions` in §7's order, confirm full history
  returns with `audience` populated everywhere and `SELECT count(*) FROM log_entries WHERE audience IS NULL`
  is zero.
- **Python integration** (`python/micromegas/tests/`) — run the existing suite against a locally
  started stack (`local_test_env/ai_scripts/start_services.py`) after a fresh ingest, to confirm
  the new column appears and nothing regressed on `SELECT *` paths (clients access columns by name,
  never positionally — verified for `python/`, `grafana/`, `analytics-web-app/`).
- **Manual**: `micromegas-query "SELECT audience, count(*) FROM log_entries GROUP BY audience"`
  and an `EXPLAIN` of a filtered `log_entries` query, to eyeball the plan before/after.

## Future work

- Give the JIT views (`net_spans`, `otel_spans`, `images`, `async_events`, `thread_spans`) the same
  column. They cost nothing to bump — JIT partitions rebuild on first query — and the new
  `OwnershipRewrite` branch picks them up automatically with no rule edit, collapsing §4/§5/§6 to
  nothing and letting `processes_source`/`streams_source` and the whole subquery machinery be
  deleted. Mind the mixed-rollout caveat `CHANGELOG.md:54` records for JIT bumps. Worth its own
  issue.
- Partition pruning: the column alone buys row-group pruning only for partitions that happen to be
  single-audience. The real win needs audience-homogeneous partitions — per-audience object-storage
  prefixing / one partition set per audience, which is the rest of step 15. Two constraints that
  work inherits: `audience` is a documented stable column, so promoting it to a storage-path or
  partition-key component must *keep* it on the rows — `SELECT audience FROM log_entries` has to
  keep working; and it should verify that `PruningPredicate` engages through the `cast` on the
  dictionary views (§5).
- Stage 5b: a write-side authorization gate on `insert_stream`/`insert_block`
  (`tasks/completed/1373_ingestion_stamping_plan.md` §7). The deferred Postgres `processes.audience`
  column (Trade-offs) is a candidate input to it.
- Unify `audience_guard.rs`'s `LEFT JOIN LATERAL` with `audience_subselect()`.

## Open Questions

None outstanding — all resolved during review:

1. ~~Is the retention-bounded history loss of a bump acceptable?~~ **Moot.** Lakehouse partitions
   already expire with their Postgres sources (`retire_expired_partitions`), so a bump costs
   regeneration time, not data. All six views bump.
2. ~~Should the `audience` column be documented as a stable part of the SQL surface?~~ **Yes.** A
   documented, stable, non-nullable column on all six global views — queryable, groupable, joinable
   — appended last so `SELECT *` and positional readers keep working, per `CLAUDE.md`'s
   SQL-interface rule. It cannot be quietly dropped or renamed later, the per-audience partitioning
   follow-up must keep it on the rows, and `log_stats` ships with it (five of six would be a
   contract violation).
3. ~~Nullable + schema evolution, or non-nullable + bump everything?~~ **Non-nullable.** Decided
   once (1) fell: the nullable design's only advantage was avoiding a bump that turned out to cost
   nothing permanent, and its price was a two-meaning `NULL`, a permanent `OR audience IS NULL`
   disjunct, and an operational ordering precondition. See Trade-offs.
4. ~~Keep `MICROMEGAS_UNSTAMPED_AUDIENCE` as the value coalesced in at materialization?~~ **No —
   replace it with `MICROMEGAS_DEFAULT_AUDIENCE` on the write side and remove the unstamped state
   from both prongs.** "What audience does data with no explicit audience get" is a write-time
   question; answered at ingestion (and once by migration v8 for legacy rows) it never has to be
   asked at read time again. See §0 and §4.
