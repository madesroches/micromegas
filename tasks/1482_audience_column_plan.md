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
`MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` (default `public`), is stamped onto any process whose
credential carries no audience, and an idempotent backfill, run at every ingestion-service startup,
stamps the same value onto the legacy rows that were never stamped. With "unstamped" gone as a state, the read-side fallback knob
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
  copy. `OwnershipRewrite::audience_col()` (`:155-168`) is
  `cast(property_get(col("properties"), AUDIENCE_PROPERTY), Utf8)`, aggregated per process by
  `per_process_audience()` (`:170-184`) as `Aggregate(GROUP BY process_id, MAX(audience_col))` over
  the raw `__processes__partitions` scan, then filtered by `resolved_predicate()` (`:196-226`) as
  `coalesce(resolved_audience, unstamped_audience) IN (caller audiences)`.

`predicate_for()` (`:312-374`; signature takes `table_name: &TableReference` and
`mat_view: &MaterializedView`, and `let view = mat_view.get_view()` at `:324` puts the view's file
schema in reach) checks `public_view_sets` first (`:316-323`, an early `return Ok(None)`) and then
branches per view set:

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
credentials, and `--disable-auth` (no `AuthContext` at all) — and, as a second, deliberate degrade
(`:18-24`, pinned by `rust/public/tests/resolve_write_audience_tests.rs:136-144`), when a
`bound_audience` fails `WriteAudience::new`'s charset check. Its five callers are the HTTP-edge
handlers in `rust/public/src/servers/{ingestion,otlp,webhook,firehose,firehose_cloudwatch_logs}.rs`.
`WriteAudience` (`rust/ingestion/src/write_audience.rs`) is `Option<Arc<str>>` with a deliberate
no-`Default` policy. The service that consumes it, `WebIngestionService`, is constructed by
`serve_ingestion` (`rust/public/src/servers/ingestion.rs:131`, `WebIngestionService::new(lake)` at
`:147`), by `WebIngestionService::from_env` (`web_ingestion_service.rs:243`, tests only), and by
~25 test sites across `rust/{ingestion,public,analytics}/tests`. The OTLP identity derivation folds
the audience into `process_id`/`block_id` (`rust/otel-ingestion/src/identity.rs:50-58`,
`IdentityContext.audience: Option<&str>`; the struct derives `Default`, and `None` reproduces
pre-Stage-5 ids).

The read side consumes it through `IsolationConfig.unstamped_audience: Option<String>`
(`rust/analytics/src/lakehouse/read_scope.rs:127-141`, parsed from
`{prefix}_UNSTAMPED_AUDIENCE` / `MICROMEGAS_UNSTAMPED_AUDIENCE` in `from_env`, `:223`; default
`public` via `DEFAULT_UNSTAMPED_AUDIENCE`, `:116`, and a hand-written `impl Default` at `:143-149`
whose semantics are documented at `:120-126`; empty string ⇒ `None` ⇒ fail-closed). It rides
on `CallerContext.isolation_config` and is handed to **both** prongs by `query.rs`
(`AudienceGuard::new` at `:126-131`, `OwnershipRewrite::new` at `:335-340`). Startup sites:
`rust/monolith/src/main.rs:284` (`IsolationConfig::from_env("MICROMEGAS_ANALYTICS")`) and
`rust/public/src/servers/flight_sql_server.rs:315`. Prong B has a third consumer besides
`is_readable`: `AudienceGuard::global_rows_visible` (`audience_guard.rs:415-430`), the
`list_partitions` `'global'`-row rule, admits a global partition row when the view set is on
`public_view_sets` **or** `unstamped_audience` is in the caller's scope — under the default knob,
that second disjunct is what makes global rows visible to every authenticated caller today.

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
  `blocks_view_schema()` as its declared schema (`write_partition.rs:884, :925`). Column **names**
  therefore need not match (they already don't — `processes.properties as process_properties` in
  SQL vs. the `processes.properties` field), but **order and type must**. A PG `TEXT` column maps
  to nullable `Utf8` (`sql_arrow_bridge.rs:320-323`), and a SQL `NULL` in any column becomes a
  real Arrow null (`append_null`, `sql_arrow_bridge.rs:58-61, 92-95`); the declared field's
  nullability governs the file. `blocks_file_schema_hash()` is hand-written (`blocks_view.rs:307`,
  currently `vec![3]`). `data_sql` is an `Arc<String>` built in `BlocksView::new` (`:59-71`).
- **The parquet write is positional and does not check nullability.** `AsyncArrowWriter` zips
  the declared schema's fields against the batch's columns with no name check
  (`write_partition.rs:925`, parquet `arrow_writer/mod.rs:1027-1035`), and a null under a
  required leaf is written as the type's default value, not rejected
  (`arrow_writer/levels.rs:655-690`). "Append last" is therefore load-bearing at every site below,
  and a declared `false` nullability is documentation until something enforces it (§1 adds that).
  **One declaration is already wrong today and relies on this leniency:** `blocks_view_schema()`
  declares `processes.parent_process_id` as `Utf8, false` (`blocks_view.rs:298`), but the PG
  column is nullable and is `NULL` for every OTLP process (`web_ingestion_service.rs:653, :689`)
  and every root native process (`process_info.rs:70`, `Option<Uuid>`). The null is written as
  `""` and read back as such — `partition_source_data.rs:202-206` even depends on the `""`
  (`if parent_value.is_empty() { None }`). An audit of every other partition builder found no
  other mislabelled field: the only `append_null` sites are `otel/spans_block_processor.rs:195,
  :220`, whose fields are correctly declared nullable (`otel/spans_table.rs:41, :68`), and
  `streams.tags` is already nullable (`blocks_view.rs:266`).
  The actual write loop is not in `write_partition_from_rows` (`:884-`) itself but in the
  separate `pub fn write_rows_and_track_times` (`write_partition.rs:693-730`, write at `:727`) —
  `pub` precisely so `tests/write_partition_tests.rs` can drive it against an `InMemory` store.
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

Retention bounds the bill and (almost) rules out data loss. `delete_old_data` (`delete.rs:152`)
deletes Postgres `blocks` rows and payload blobs (`delete.rs:23, :38-41`) past
`MICROMEGAS_RETENTION_DAYS` (default 90 in both `rust/monolith/src/main.rs:161` and
`rust/telemetry-maintenance-srv/src/main.rs:24`), deletes `streams`/`processes` rows once they are
both past the horizon **and** empty (`delete_empty_streams_batch` / `delete_empty_processes_batch`,
`delete.rs:66-72, :108-114` — a long-lived process with recent blocks keeps its row), and, in the
same function, retires lakehouse partitions past the same horizon (`retire_expired_partitions`,
`delete.rs:146-166` → `write_partition.rs:86-135`, files then removed by
`delete_expired_temporary_files`; all of it runs from the hourly task, `maintenance.rs:141-142`).
Parquet partitions therefore outlive their sources by at most one partition width: partitions
retire on `end_insert_time < expiration` (`write_partition.rs:97`) while blocks are deleted on
`insert_time <= expiration` (`delete.rs:23`), so the oldest day-sized partition of each view can
survive after some of its source blocks are gone. Everything a bump hides is regenerable except
that boundary bucket, which shrinks to nothing within a day. The cost is (a) the regeneration
itself — for `log_entries`/`measures`, re-processing up to a full retention window of raw blocks —
and (b) the window during which un-regenerated history is invisible.

JIT (per-process / per-stream) instances rebuild on first query after a bump: `spec_is_up_to_date`
(`jit_partitions.rs:1177-1182`) treats a hash mismatch as stale — the #1429 / #1478 precedent
(`CHANGELOG.md:54`, `:76`).

## Design

### 0. The invariant: every process has an audience

Everything below rests on one statement that becomes true at deploy time and stays true:

> **Every row of Postgres `processes` carries a `micromegas.audience` property.**

Three mechanisms establish and keep it, each closing one way a `NULL` could appear:

1. **Write path — a default audience.** `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` (default `public`;
   validated against the `[A-Za-z0-9_-]{1,255}` charset; malformed ⇒ startup error, the same
   fail-fast `IsolationConfig::from_env` uses) is resolved once at ingestion-server startup by
   `WriteAudience::default_from_env()`. The name is deliberately parallel to the existing
   `MICROMEGAS_DEFAULT_KEY_AUDIENCE` (`rust/auth/src/policy.rs:54-63`, the web role's fallback
   audience for a *newly minted key*) and must not be confused with it: one says what audience a
   new key gets, the other what audience *data written without one* gets. It is stored on
   `WebIngestionService` as `default_audience: WriteAudience`, which means
   `WebIngestionService::new(lake, default_audience)` gains the parameter, `from_env` resolves it
   itself, and `serve_ingestion`'s public signature (`rust/public/src/servers/ingestion.rs:131`)
   gains it so the two binaries can pass it through. The ~25 test sites that build the service
   get a `WebIngestionService::new_for_test(lake)` helper (default `public`) so the ones that don't
   care about audiences are a one-word edit.
   `resolve_write_audience(ctx, default: &WriteAudience) -> Result<WriteAudience, _>` returns the
   credential's `bound_audience` when it has one and the default otherwise. A `bound_audience`
   that fails the charset check is **rejected**, not degraded: today it degrades to unstamped
   (`write_audience.rs:18-24`), but with `none()` gone the only degrade left would be *the default
   audience* — moving a restricted key's writes into `public` — which is fail-open on the one
   boundary this whole design exists to hold. Every handler already has an error path to return
   through: the native route's `IngestionError::Forbidden` (`ingestion.rs:26-29, :65-74`) and the
   inline error `Response`s of the OTLP / firehose handlers (`otlp.rs:150-152`, `firehose.rs:52-58`).
   The case is near-unreachable in practice (`ingestion_api_keys.audience` is `CHECK`-constrained,
   `sql_migration.rs:161-169`, and the other three `bound_audience` producers hard-code `None`),
   so a 403 costs nothing. `WriteAudience` becomes `WriteAudience(Arc<str>)`: `none()` is deleted,
   `as_str()` returns `&str`, and the compiler enumerates the five HTTP-edge callers plus every
   test that built an unstamped write.
   `IdentityContext.audience` (OTLP) becomes `&str` **and the struct drops its `Default` derive**
   (`identity.rs:50`, doc paragraph `:46-49`): `<&str>::default()` is `""`, so with the derive
   kept every `IdentityContext::default()` — ~25 sites in `identity_tests.rs`, `block_tests.rs`,
   `cloudwatch_logs_tests.rs` — would keep compiling and silently fold an empty audience into
   every id, which is exactly the silent default the "compiler enumerates" rule exists to prevent.
2. **Backfill — idempotent, at every ingestion startup.** A new
   `backfill_default_audience(pool, &WriteAudience)` (`rust/ingestion/src/audience_backfill.rs`)
   appends `ROW('micromegas.audience', $1::text)::micromegas_property` to `processes.properties`
   for every row that lacks the key, with `$1` = the configured default audience:

   ```sql
   UPDATE processes
      SET properties = array_append(properties, ROW('micromegas.audience', $1::text)::micromegas_property)
    WHERE NOT EXISTS (SELECT 1 FROM unnest(properties) WHERE key = 'micromegas.audience');
   ```

   (`micromegas_property` is `(key TEXT, value TEXT)`, `sql_telemetry_db.rs:17`; `properties` is
   nullable, `:37`, and both `array_append(NULL, x)` and `unnest(NULL)` do the right thing.) It
   runs on the **ingestion-role startup path only**, right after `connect_to_remote_data_lake`
   (`rust/telemetry-ingestion-srv/src/main.rs:52`; `rust/monolith/src/main.rs:184`, gated on
   `roles.ingestion`), before the listener binds. It is **not** a versioned migration
   (`LATEST_DATA_LAKE_SCHEMA_VERSION` stays 7, `migrate_db`/`execute_migration` are untouched),
   for two reasons:
   - *Rolling upgrades.* Ingestion is stateless and documented as horizontally scaled
     (`mkdocs/docs/admin/ingestion.md:100-108`). A version-gated backfill runs exactly once, at
     the first new replica's startup; every `processes` row an old replica writes after that is
     unstamped and never repaired — and then trips the conflict guard, the `NOT NULL` extraction,
     and §1's poison-pill partition write. An idempotent statement re-run at every start repairs
     stragglers at the next replica start. A zero-row run is one sequential scan of a
     retention-bounded table with no row locks, cheap enough to not think about.
   - *Who runs `migrate_db`.* Its real callers are `connect_to_remote_data_lake`
     (`remote_data_lake.rs:60`) — reached from the monolith for **any** lake-backed role
     (`needs_lakehouse()`, `main.rs:96-98`, i.e. `--roles maintenance` or `flightsql` too) — and
     `WebIngestionService::from_env` (tests only). A backfill inside `migrate_db` would let a
     maintenance-only monolith, with no reason to carry an ingestion knob, win the race and label
     legacy data with whatever default it happened to have. Keeping the backfill on the ingestion
     path keeps the knob where the plan says it belongs.

   Migration v6 (#1372, `sql_migration.rs:152-175`) is the precedent for the *shape* — it
   backfilled `ingestion_api_keys.audience` to the literal `'public'`; this backfill uses the knob
   instead, so a deployment that wants its legacy data under a different label sets
   `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` *before* upgrading and gets exactly that.
3. **Conflict guard — no `NULL` arm.** In `check_process_audience_conflict`, the
   `let Some(incoming) = audience.as_str() else { return Ok(()) }` early-out goes (there is no
   unstamped write any more), and the `None =>` "no retro-stamp" arm becomes an error: a row
   without the property after the backfill is an invariant violation (an old replica during a
   rollout, or something writing to `processes` bypassing ingestion), and an
   `IngestionServiceError::DatabaseError` naming the `process_id` is the right fail-closed
   response. This closes the known gap at `CHANGELOG.md:40`. The doc comments that describe the
   `None` behaviour go with it: `finalize_process_properties` (`web_ingestion_service.rs:113-118`),
   `remember_process_audience` (`:637-641`), the arm's own message (`:620-628`), and the
   `write_audience.rs` module doc (`:1-4, :9-16`).

Consequences worth stating plainly:

- **Deployments that were unstamped become default-audience.** Under the default (`public` on
  both the old read knob and the new write knob) nothing observable changes: what was "unstamped,
  visible to `public`" is now "stamped `public`, visible to `public`". An operator who had set
  `MICROMEGAS_UNSTAMPED_AUDIENCE` to a non-default label, or to the empty string (fail-closed),
  must pick a `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` before upgrading — for the fail-closed case,
  a label that no principal is granted (e.g. `unassigned`). The startup check in §4 makes
  forgetting this loud rather than silent.
- **A rolling upgrade has a residual window.** Rows written by an old replica after the *last*
  new replica started are repaired only at the next ingestion (re)start. Until then, an
  insert-hour containing such a row fails its `blocks` materialization (§1's guard, fail-closed,
  retried by the daemon every tick) — visible in the maintenance logs, never a leak. The
  operational note tells operators to restart one ingestion replica once the rollout completes if
  they see it.
- **OTLP `process_id`s churn once** in previously-unstamped deployments: the audience is folded into
  the id (`identity.rs:52-58`), so a resource that produced id X unstamped produces id Y stamped
  `public`. The old row is backfilled to `public`, the new row is stamped `public` by the
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
`write_rows_and_track_times` (`write_partition.rs:693-730`, the loop that actually calls
`AsyncArrowWriter::write` at `:727`) gains a **nullability guard**: for every declared
non-nullable field, `column.null_count() == 0` or the write fails with an error naming the view,
the column, and the partition's insert range. It is one `null_count()` per column per batch, it
protects every view rather than just this one, and it turns a violated §0 invariant (a straggler
old replica, or something writing to `processes` bypassing ingestion) into a loud, fail-closed
materialization error instead of a silently `""`-labelled row.

Two things the guard forces into the open:

- **It is a poison pill by design.** One bad row fails the whole insert-hour `blocks` write, and
  `blocks` is the root of all six views — so that hour is invisible on every global view until the
  row is repaired and the daemon's retry succeeds. That is the intended fail-closed shape, but it
  is why the error message must carry view + column + range, and why §0.2's backfill re-runs at
  every ingestion start.
- **`processes.parent_process_id` must be re-declared nullable first.** As Current State records,
  `blocks_view_schema()` labels it `Utf8, false` while it is `NULL` for every OTLP and root
  process; the guard as specified would reject essentially every fresh `blocks` partition. Flip
  the declaration to `true` in the same edit that appends `audience` (`blocks_view.rs:298`) — free,
  since the hash is bumping to `vec![4]` anyway, and both readers keep working:
  `partition_source_data.rs:202` (`is_empty()` on the accessor's value; a real null reads as `""`
  through `StringColumnAccessor`) and `metadata.rs:349` (`is_null(0)`). The audit of the other
  partition builders (Current State) found nothing else to fix, so the guard can be universal.

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

- `processes_view.rs`: `max(audience) as audience`, appended **last** in the SELECT list of both
  the transform (`:25-45`, after `last_block_end_time`) and the merge query (`:46-67`), so the
  inferred schema grows only at the end. Note the column is referenced **unquoted**: the
  neighbouring `"processes.exe"`-style names are quoted because the dots are literal characters in
  the `blocks` field names; `audience` has no prefix and `"processes.audience"` would not resolve.
- `streams_view.rs`: same, after `last_update_time` (transform `:25-38`, merge `:39-53`).
- `log_stats_view.rs`: `arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience`,
  appended after `count` in both the transform (`:32-45`) and the merge query (`:50-59`). The cast
  is not optional: `max` **coerces a dictionary input to its value type** (`get_min_max_result_type`,
  `min_max.rs:58-77`, called from `Max::coerce_types` at `:360`), so a bare `max(audience)` would
  infer `Utf8` — unlike `process_id`
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
  it keeps `public_view_sets`. The hand-written `impl Default` (`:143-149`) — whose only reason to
  exist is the `public` default for the removed field — becomes `#[derive(Default)]`, and the
  default-semantics paragraph at `:120-126` goes; the ~12 `IsolationConfig::default()` callers
  (`CallerContext::internal`/`maintenance`, `flight_sql_server.rs:283, :327`, tests) compile
  unchanged. `from_env` **errors** if `{prefix}_UNSTAMPED_AUDIENCE` or
  `MICROMEGAS_UNSTAMPED_AUDIENCE` is set — "removed in <version>; assign legacy data an audience
  with `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` on the ingestion side" — rather than silently
  ignoring a knob an operator may be relying on for fail-closed behaviour. The `resolved_var`
  helper (`:201-212`) already centralizes the prefix fallback, so one
  `std::env::var(&resolved_var(prefix, "UNSTAMPED_AUDIENCE")).is_ok()` catches both spellings,
  including an explicit empty string.
- `OwnershipRewrite::new` and `AudienceGuard::new` lose their `unstamped_audience` parameter
  (`query.rs:126-131`, `:335-340` are the callers).
- `audience_guard.rs`: delete `OwnerAudience::Unstamped`; `merge_owner_rows` (`:107-118`) maps a
  `None` audience to `OwnerAudience::Unknown` — after the backfill a `NULL` means "no such row" as
  far as access is concerned, and `Unknown` is already always-denied. `is_readable` (`:272-292`)
  loses the `unstamped_audience` argument and its `Unstamped` arm (`:281-283`); the module doc's
  prong-divergence discussion (`:21-22`) and `owner_query_sql`'s comment about keeping unstamped
  rows (`:137-140`) are rewritten (§7 below).
- **`AudienceGuard::global_rows_visible` (`:415-430`) becomes public-allowlist-only.** Its
  second disjunct (`unstamped_audience` in the caller's scope) is what admits `list_partitions`'
  `'global'` rows to every authenticated caller today; deleting the field without a decision would
  silently change that. The decision: `ReadScope::Audiences` callers see a global partition row
  only when its view set is on `public_view_sets`; `ReadScope::All` is unchanged. This is an
  intentional **tightening**, recorded in the CHANGELOG breaking-change clause. Rationale: a
  global partition is a multi-audience file whose metadata (row counts, sizes, time ranges) says
  something about every tenant at once, so a scoped caller has no claim to it; and
  `list_partitions` is admin tooling — its only in-tree client is
  `python/micromegas/micromegas/admin.py`, which runs under `ReadScope::All`. The alternative
  ("admit when the deployment's default audience is in scope") would put the write-side knob back
  on the read side, which §0 exists to avoid. Three tests encode the old rule
  (`audience_guard_tests.rs:209-234`, `prong_b_guard_db_test.rs:610-640`) and are rewritten to the
  new one.
- `ownership_rewrite.rs`: `resolved_predicate()` drops the `coalesce` — it is
  `resolved_audience IN (caller audiences)`, `lit(false)` on an empty set as today.

### 5. `OwnershipRewrite`: a new branch after the `public_view_sets` check, keyed on the column's presence

`predicate_for` gains a branch **immediately after the §7 `public_view_sets` early-return
(`:316-323`) and ahead of §3/§4** — not literally first: a public view set that carries the column
must still get no predicate, and `public_view_set_plans_with_no_injected_predicate`
(`ownership_rewrite_public_view_set_tests.rs:249-263`) pins that. It is keyed on
`view.get_file_schema().field_with_name("audience")` — the same schema-introspection style as the
existing `process_id` test (`:344`), so a view set that gains the column later (the JIT span/image
views, see [Future work](#future-work)) upgrades automatically with no edit here:

```rust
// §2 (new): views carrying a physical `audience` column -- processes, streams, blocks,
// log_entries, measures, log_stats (global and per-process instances alike). Filtered
// directly, no semi-join, no property_get.
if let Ok(field) = view.get_file_schema().field_with_name("audience") {
    return Ok(Some(self.audience_column_predicate(table_name, field)));
}
```

with

```rust
/// `audience IN (caller audiences)`; `false` for an empty set (fail-closed, as
/// `resolved_predicate` already does). The column is NOT NULL, so there is no unstamped case.
fn audience_column_predicate(&self, table_name: &TableReference, field: &Field) -> Expr {
    let audiences = self.audiences();
    if audiences.is_empty() {
        return lit(false);
    }
    let raw = Expr::Column(Column::new(Some(table_name.clone()), "audience"));
    // This rule runs after DataFusion's own TypeCoercion pass, so a Dictionary(Int32, Utf8)
    // column (log_entries/measures/log_stats) must be cast to compare against Utf8 literals.
    // blocks/processes/streams carry plain Utf8 -- skip the no-op cast there, as §3 already
    // does for `processes.process_id`, so PruningPredicate sees a bare column reference.
    let lhs = if field.data_type() == &DataType::Utf8 { raw } else { cast(raw, DataType::Utf8) };
    lhs.in_list(
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

The per-process JIT instances of `log_entries` and `measures` share `log_table_schema()` /
`metrics_table_schema()` with their global instances (`log_view.rs:83-92`, `metrics_view.rs:83-91`)
and are populated through `find_process` → `process_metadata_from_row` (`log_view.rs:159`,
`metrics_view.rs:159`), which §3 extends — so they carry `audience` too and take this branch as
well. "The JIT views" in [Future work](#future-work) means only the five view sets that still lack
the column: `net_spans`, `otel_spans`, `images`, `async_events`, `thread_spans`.

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
write-once and always present**: it is written at registration (or by the backfill), there is no
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

**Order of operations on the ingestion side**: the backfill runs at ingestion-service startup,
after `migrate_db` and before the listener binds (both binaries `await` the lake connection
sequentially in `main` before serving — `telemetry-ingestion-srv/src/main.rs:51-75`,
`monolith/src/main.rs:184-315`), and the writer stamps the default from its first request, so the
§0 invariant holds before the first post-deploy partition is written, modulo the rolling-upgrade
window §0 describes. Set `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` first if the default is not what
legacy data should be labelled.

**Regeneration**, in dependency order, over the retention window:

| Step | View | Source | Cost |
|---|---|---|---|
| 1 | `blocks` | Postgres | cheap — metadata-sized |
| 2 | `processes`, `streams` | `blocks` partitions (new hash) | cheap — one row per process/stream per partition; but see below, `processes` gates §5/§6 |
| 2 | `log_entries`, `measures` | `blocks` partitions (new hash) + payload blobs | **the expensive ones** — re-parses every retained block |
| 3 | `log_stats` | `log_entries` partitions (new hash) | re-aggregates all `log_entries` into 1-minute bins |

`blocks` must go first: `fetch_partition_source_data` selects source blocks by the current
`blocks` hash (`partition_source_data.rs:266-267`), so `log_entries`/`measures` regenerated before
`blocks` would see no sources. Step 2's four views are independent of each other, but `processes`
deserves priority within the step: §5/§6 (`async_events`, `thread_spans`) still resolve audiences
through `__processes__partitions`, so until `processes` is regenerated those two JIT views deny
every pre-deploy process outright. Until any view's regeneration completes, queries against it
return only post-deploy data — a visible gap, never a leak.

**The calls must tile day-aligned buckets.** `regenerate_partition_range` rejects a range that is
not a whole multiple of `partition_delta_seconds` (`batch_update.rs:293-301`), and
`verify_force_regeneration_alignment` (`:111-139`) bails if any existing partition of the view —
old-hash ones included — is not fully contained in one bucket (documented at
`mkdocs/docs/admin/functions-reference.md:143-145`). The daemon writes day-sized partitions at
midnight boundaries for anything older than its short trailing windows (`maintenance.rs:113-118`),
so a naive `[now - 90d, now]` fails loudly. The working shape, per view, is
`regenerate_partitions('<view>', date_trunc('day', now()) - interval '90 days', date_trunc('day', now()) - interval '2 days', 86400)`;
the daemon's own 1s/1min/1h/1day tasks re-materialize the trailing ≤2 days on the new hash by
themselves. `retire_partitions` matches on view/instance/time only (`write_partition.rs:195-225`),
so regeneration also reclaims the old-hash files in the ranges it covers; for a range an operator
chooses *not* to regenerate, `micromegas.admin.list_incompatible_partitions()` /
`retire_incompatible_partitions()` (`python/micromegas/micromegas/admin.py:14, :87`) and the
procedure at `mkdocs/docs/admin/maintenance.md:184-190` already exist.

**Nothing is lost, to within one bucket.** Lakehouse partitions expire at the same horizon as
their Postgres sources (Current State), so every partition the bump hides is one whose sources
still exist — except the oldest day at the retention edge, where blocks (`insert_time <=`) can be
deleted a little ahead of the partition that covers them (`end_insert_time <`). Regenerating that
bucket yields whatever sources remain; it is gone within a day regardless.

## Implementation Steps

### Phase 1 — every process has an audience (ingestion)

1. `rust/ingestion/src/write_audience.rs`: `WriteAudience(Arc<str>)`; delete `none()`;
   `as_str() -> &str`; add `pub fn default_from_env() -> anyhow::Result<WriteAudience>` reading
   `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` (default `public`, validated, fail-fast); rewrite the
   module doc (it is entirely about `None`).
2. `rust/ingestion/src/web_ingestion_service.rs`: `default_audience: WriteAudience` field;
   `new(lake, default_audience)` and a `new_for_test(lake)` helper (default `public`); `from_env`
   resolves the default itself; `check_process_audience_conflict` per §0.3; rewrite the
   `None`/unstamped doc comments at `:113-118`, `:620-628`, `:637-641`.
3. New `rust/ingestion/src/audience_backfill.rs`: `pub async fn backfill_default_audience(pool, &WriteAudience)`
   running §0.2's idempotent `UPDATE`; `lib.rs` module declaration. No schema-version change.
4. `rust/public/src/servers/write_audience.rs`: `resolve_write_audience(ctx, default) -> Result<..>`;
   update the five callers to pass the service's default and map `Err` to their existing 403 /
   error-response paths. `rust/public/src/servers/ingestion.rs`: `serve_ingestion` gains the
   `default_audience` parameter and passes it to `WebIngestionService::new`.
5. `rust/otel-ingestion/src/identity.rs`: `IdentityContext.audience: &str`; drop the `Default`
   derive (`:50`) and its doc paragraph (`:46-49`); `block.rs:306`'s doc reference.
6. `rust/telemetry-ingestion-srv/src/main.rs` (after `connect_to_remote_data_lake`, `:52`) and
   `rust/monolith/src/main.rs` (after `:184`, gated on `roles.ingestion`): resolve the default via
   `WriteAudience::default_from_env()`, call `backfill_default_audience`, pass the default to
   `serve_ingestion`.
7. Compile fallout: `rust/ingestion/tests/{write_audience_tests,audience_stamping_db_test,process_audience_cache_test,insert_block_dedup_db_test,readiness}.rs`,
   `rust/public/tests/{resolve_write_audience_tests,firehose_tests,firehose_cloudwatch_logs_tests}.rs`,
   `rust/otel-ingestion/tests/{identity_tests,split_tests,block_tests,cloudwatch_logs_tests}.rs`,
   and the `analytics` DB tests that build a `WebIngestionService` (listed under Tests). Add the
   backfill test; convert `malformed_bound_audience_warns_and_degrades_to_none`
   (`resolve_write_audience_tests.rs:136`) to "rejects".

### Phase 2 — the column, materialized

8. New `rust/analytics/src/audience.rs`: re-export `PROPERTY_AUDIENCE` as `AUDIENCE_PROPERTY`,
   add `audience_subselect()`. Update `lib.rs`; `audience_guard.rs` and `ownership_rewrite.rs`
   import from here.
9. `lakehouse/blocks_view.rs`: `format!` the subselect into `data_sql`, append
   `Field::new("audience", Utf8, false)` to `blocks_view_schema()`, re-declare
   `processes.parent_process_id` nullable (`:298`), `blocks_file_schema_hash()` → `vec![4]`.
   `lakehouse/write_partition.rs`: the non-nullable-column guard in `write_rows_and_track_times`
   (§1), error naming view + column + insert range.
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

18. `lakehouse/read_scope.rs`: remove `unstamped_audience` / `DEFAULT_UNSTAMPED_AUDIENCE`; replace
    the hand-written `impl Default` (`:143-149`) with `#[derive(Default)]` and drop the `:120-126`
    paragraph; `from_env` errors on a set `*_UNSTAMPED_AUDIENCE`.
19. `lakehouse/audience_guard.rs`: remove `OwnerAudience::Unstamped`, the `unstamped_audience`
    parameter, and the `is_readable` arm; `None` audience ⇒ `Unknown`; `global_rows_visible`
    becomes public-allowlist-only (§4); rewrite the `:21-22` and `:137-140` doc comments.
20. `lakehouse/ownership_rewrite.rs`: add `audience_column_predicate` and the new branch right
    after the `public_view_sets` early-return; delete §3 and `audience_col()`; repoint
    `per_process_audience()` at `col("audience")`; drop the `coalesce` from `resolved_predicate`;
    drop the `PropertyGet` / `AUDIENCE_PROPERTY` imports and the `unstamped_audience` field;
    rewrite the module doc comment.
21. `lakehouse/query.rs`: drop the `unstamped_audience` arguments at both construction sites.
22. Tests — see [Testing Strategy](#testing-strategy): `ownership_rewrite_public_view_set_tests.rs`
    (restructure `real_view_factory_covers_every_registered_view_set`),
    `ownership_rewrite_config_tests.rs` (removal semantics), `ownership_rewrite_db_test.rs`,
    `prong_b_guard_db_test.rs` and `audience_guard_tests.rs` (including the three
    `global_rows_visible` tests), `tests/common/db_fixtures.rs` (delete
    `caller_with_unstamped_audience`).

### Phase 4 — docs and changelog

23. Documentation updates listed below, plus the CHANGELOG entry with its **Operational note**,
    **Minor breaking change** clause, and the removed-env-var notice.
24. Mark step 15 of `tasks/data_isolation/audience_based_access_control_plan.md` as landed and
    point it at this plan.

## Files to Modify

**New**
- `rust/analytics/src/audience.rs`
- `rust/ingestion/src/audience_backfill.rs`

**Ingestion — default audience and backfill**
- `rust/ingestion/src/write_audience.rs`
- `rust/ingestion/src/web_ingestion_service.rs`, `rust/ingestion/src/lib.rs`
- `rust/public/src/servers/write_audience.rs` and its five callers
  (`ingestion.rs` — also `serve_ingestion`'s signature —, `otlp.rs`, `webhook.rs`, `firehose.rs`,
  `firehose_cloudwatch_logs.rs`)
- `rust/otel-ingestion/src/identity.rs` (and the doc reference in `block.rs:306`)
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
- `rust/analytics/tests/write_partition_tests.rs` (nullability guard)
- `rust/analytics/tests/thread_spans_ordering_db_test.rs`, `jit_process_batch_db_test.rs`
  (`WebIngestionService` constructor fallout only)
- `rust/ingestion/tests/write_audience_tests.rs`, `audience_stamping_db_test.rs`,
  `process_audience_cache_test.rs`, `insert_block_dedup_db_test.rs`, `readiness.rs`, plus the new
  backfill test; `rust/public/tests/resolve_write_audience_tests.rs`, `firehose_tests.rs`,
  `firehose_cloudwatch_logs_tests.rs`; `rust/otel-ingestion/tests/identity_tests.rs`,
  `split_tests.rs`, `block_tests.rs`, `cloudwatch_logs_tests.rs`

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
  hashes and read `audience` as `NULL` on old partitions (and, tellingly, *errors* for a missing
  non-nullable one — `schema_rewriter.rs:405-411`). **Rejected**, for three reasons that
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
  ingestion, it never has to be asked again. The backfill is what lets the read side forget the
  concept entirely.
- **Backfill with the knob vs. the literal `'public'`.** v6 used the literal for keys. The backfill
  takes the configured default so a fail-closed deployment can route legacy data to a label nobody
  is granted instead of silently publishing it.
- **Idempotent startup backfill vs. a versioned migration v8.** The versioned form is the house
  precedent (v6), records that the step ran, and would have been the first draft's choice.
  Rejected because it runs exactly once, at the first upgraded replica's start, in a service that is
  documented as horizontally scaled — every row an old replica writes during the rollout is then
  permanently unstamped with no repair path — and because `migrate_db` is also reached from the
  monolith's non-ingestion roles, which have no business carrying the ingestion knob. A statement
  that is safe to re-run costs one cheap scan per ingestion start and needs no version, no
  `migrate_db` parameter, and no `sql_migration_test.rs` fixture.
- **`MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` vs. `MICROMEGAS_DEFAULT_AUDIENCE`.** The shorter name
  would sit two rows from `MICROMEGAS_DEFAULT_KEY_AUDIENCE` in the monolith's env table with a
  different meaning; the longer one names what it defaults.
- **`global_rows_visible`: public-allowlist-only vs. "default audience in scope".** See §4. The
  second option re-imports a write-side knob into the read side; the first is a tightening whose
  only affected surface is admin tooling that already runs under `ReadScope::All`.
- **Fail-fast on a set `*_UNSTAMPED_AUDIENCE` vs. ignore it.** Ignoring is the usual treatment of a
  retired var, but this one may be load-bearing for an operator's fail-closed posture; a startup
  error with a pointer to `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` is the safer default.
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
    ingestion credential or the deployment's `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE`; never client-settable;
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
  and `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` replaces it on the write side; the "two prongs read
  different copies" paragraph (`:184-190`) is rewritten — both copies are now non-null and
  write-once, so the prongs can never disagree about the *value*; the remaining skew is
  lakehouse-vs-Postgres lag in both directions — materialization lag on the way in (a row is
  visible to Prong B before Prong A), retention lag on the way out (after `delete_old_data` drops
  the Postgres row, Prong B resolves `Unknown` → deny while Prong A still admits from partitions
  that survive until `retire_expired_partitions`, `audience_guard.rs:52-55` already notes this).
- `mkdocs/docs/admin/ingestion.md` — "What gets stamped" (`:71-91`): the env-keyring / OIDC /
  `--disable-auth` bullets now say "stamped with `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE`"; add the
  var to the ingestion env table with a sentence contrasting it with
  `MICROMEGAS_DEFAULT_KEY_AUDIENCE`; the startup backfill and the rolling-upgrade note; the OTLP
  id-churn note.
- `mkdocs/docs/admin/flight-sql.md:33`, `monolith.md:51` — remove the `*_UNSTAMPED_AUDIENCE` rows;
  add `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` to the monolith table (ingestion role), next to
  `MICROMEGAS_DEFAULT_KEY_AUDIENCE` (`:54`) with the same contrasting sentence.
- `mkdocs/docs/admin/api-keys.md:271-299` (two mentions, `:272` and `:298`),
  `mkdocs/docs/admin/functions-reference.md:75` (the `list_partitions` note — which also gains the
  new `global_rows_visible` rule) — the "unstamped ... visible through
  `MICROMEGAS_UNSTAMPED_AUDIENCE`" phrasing → default audience.
- `rust/analytics/src/lakehouse/view_factory.rs` — the module doc's per-view schema tables
  (`log_entries` `:11`, `measures` `:30`, `processes` `:126`, `streams` `:146`, `blocks` `:160`;
  no `log_stats` table — add one or note the omission). The `blocks`, `processes`, and `streams`
  tables there are already stale (`blocks` misses `streams.insert_time`, `streams.format`,
  `processes.insert_time`, `processes.parent_process_id`, `processes.properties`; `processes`
  misses `last_update_time`, `last_block_end_*`; `streams` misses `format`, `last_update_time`);
  fix the tails while appending.
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — module doc: new branch table; the "One
  audience per process, not per row" section rewritten around §6 (the aggregate is retained for
  §5/§6 because partitions still hold several rows per process, but the rows can no longer
  disagree; per-row filtering on the column is sound for the same reason); `audience_col` /
  `property_get` / `unstamped_audience` gone.
- `rust/analytics/src/lakehouse/audience_guard.rs` — module doc (`:21-22`) and `owner_query_sql`'s
  comment (`:137-140`): `Unstamped` gone, `Unknown` covers a missing row *or* (post-backfill,
  invariant violation) a missing property; `global_rows_visible`'s doc (`:411-414`) states the
  allowlist-only rule.
- `CHANGELOG.md` — Unreleased → Analytics and Ingestion, following the `:54`/`:76`/`:152`
  precedents (`**Operational note**` is the label those two entries use):
  - **Operational note**: all six global views bump their file-schema hash; run
    `regenerate_partitions` with day-aligned, `86400`-second buckets from
    `date_trunc('day', now()) - 90 days` to `date_trunc('day', now()) - 2 days` in the order given
    in §7 (`blocks` first, then `processes`/`streams`/`log_entries`/`measures`, then `log_stats`;
    the daemon covers the trailing two days); until then those views show post-deploy data only.
    Ingestion backfills `micromegas.audience` onto never-stamped processes with
    `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` at every startup — set it before upgrading if `public`
    is not the label legacy data should carry; after a rolling upgrade, restart one ingestion
    replica if the maintenance log reports a `blocks` write rejected for a null `audience`. OTLP
    `process_id`s churn once in previously-unstamped deployments.
  - **Breaking change** (the `:106` convention for a removed env var):
    `MICROMEGAS_UNSTAMPED_AUDIENCE` / `{prefix}_UNSTAMPED_AUDIENCE` removed; startup fails if set;
    replaced by `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` on the ingestion side. `list_partitions`
    shows `'global'` partition rows to audience-scoped callers only for view sets on
    `MICROMEGAS_PUBLIC_VIEW_SETS` (previously also whenever the unstamped audience was in scope).
    A malformed `bound_audience` is now rejected (403) instead of ingesting unstamped.
  - **Minor breaking change**: `ProcessMetadata` gains a required `audience: Arc<str>` field;
    `WriteAudience` is no longer optional (`none()` removed, `as_str() -> &str`);
    `resolve_write_audience` takes the default and returns `Result`; `WebIngestionService::new`
    and `serve_ingestion` take the default; `IdentityContext` no longer implements `Default` and
    `audience` is `&str`; `OwnershipRewrite::new` / `AudienceGuard::new` lose `unstamped_audience`;
    `OwnerAudience::Unstamped` removed; `IsolationConfig.unstamped_audience` removed;
    `blocks.processes.parent_process_id` is declared nullable (it always could be null).
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
  (`:389-464`) enumerates every `default_view_factory()` view set and asserts each plan contains
  `LeftSemi Join` (the §5/§6 `EXISTS` shapes decorrelate to `LeftSemi` too, `:334-388`); six now
  produce a bare `Filter`. Restructure it into two expectations keyed on whether the view's file
  schema has `audience`: `Filter` on `audience IN (...)` and **no** join / no `property_get` for
  the six (the regression test for the optimization itself), the semi-join for the rest. Update
  the per-view shape assertions for `streams` (`:264-279`), `processes` (`:313-332`), and the
  empty-audience `EmptyRelation` case (`:293-312`); `public_view_set_plans_with_no_injected_predicate`
  (`:249-263`) must keep passing — it is what pins the branch placement.
- **`tests/ownership_rewrite_config_tests.rs`** — the `*_UNSTAMPED_AUDIENCE` parsing cases become
  one: a set var is a startup error naming `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE`. Keeps its
  `#[serial]` + `EnvGuard` pattern (`:27-39`).
- **Prong B**: `prong_b_guard_db_test.rs` / `audience_guard_tests.rs` unstamped cases → deleted or
  converted to default-audience; add one asserting a `None` audience row resolves to `Unknown`
  (denied). The three `global_rows_visible` tests (`audience_guard_tests.rs:209-234`,
  `prong_b_guard_db_test.rs:610-640`) are rewritten to the allowlist-only rule: public view set ⇒
  visible, anything else ⇒ hidden for `Audiences`, everything visible for `All`.
- **Unit-level**: a pure test over `audience_column_predicate` for the empty and non-empty
  audience sets and for a `Utf8` vs. `Dictionary` field (cast present only for the latter), one
  over `WriteAudience::default_from_env` (unset ⇒ `public`, malformed ⇒ `Err`), and
  `resolve_write_audience_tests.rs:136`'s malformed case flipped to "rejects".
- **Backfill**: a DB-backed test in `rust/ingestion/tests/` (the SQL is the thing under test) —
  insert a stamped and an unstamped process (one with `properties = NULL`), run
  `backfill_default_audience`, assert the unstamped ones now carry the configured default and the
  stamped one is untouched; run it a second time and assert nothing changes (idempotency is the
  property the startup re-run relies on).
- **Non-nullability is enforced at write**: in `tests/write_partition_tests.rs` (which already
  drives `write_rows_and_track_times` against an `AsyncArrowWriter` over an `InMemory` store,
  `:20-27, :54, :97, :134` — `write_partition_from_rows` itself needs a live lake and is out of
  reach there), write a batch with a `NULL` in a declared non-nullable column and assert the call
  fails naming the column, and that the same batch with the field declared nullable succeeds. This
  pins the guard §1 adds — without it parquet writes the null as `""`, which is exactly the silent
  mislabelling the guard exists to prevent.
- **Regeneration rehearsal**: against a stack with pre-change partitions (ideally including OTLP
  processes, whose `parent_process_id` is null), deploy, confirm the six views show post-deploy
  data only and that the daemon's fresh `blocks` writes succeed, run `regenerate_partitions` with
  §7's aligned calls in §7's order, confirm full history returns with `audience` populated
  everywhere and `SELECT count(*) FROM log_entries WHERE audience IS NULL` is zero.
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
   replace it with `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE` on the write side and remove the
   unstamped state from both prongs.** "What audience does data with no explicit audience get" is
   a write-time question; answered at ingestion (and by the startup backfill for legacy rows) it
   never has to be asked at read time again. See §0 and §4.
5. ~~Versioned migration v8 or an idempotent startup backfill?~~ **Startup backfill.** A one-shot
   migration cannot repair rows written by old replicas during a rolling upgrade, and `migrate_db`
   is reached from monolith roles that have no ingestion knob. See §0.2 and Trade-offs.
6. ~~What does `list_partitions` show audience-scoped callers once `unstamped_audience` is gone?~~
   **Public-allowlist view sets only.** Global partitions are multi-audience files; the only client
   is admin tooling under `ReadScope::All`. Recorded as a breaking change. See §4.
