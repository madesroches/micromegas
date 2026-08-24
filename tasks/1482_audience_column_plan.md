# First-class `audience` column on global-instance views Plan (#1482 — AbAC "physical boundary")

## Overview

Promote `micromegas.audience` from a process **property** resolved at query time to a physical,
dictionary-cheap **column** materialized on every lakehouse view that has a `global` instance
(`blocks`, `processes`, `streams`, `log_entries`, `measures`, `log_stats`). The audience is
extracted exactly once per partition write — from Postgres, at the `blocks` view's materialization
— and then propagates structurally into every downstream view, the same way
`processes.properties` already does today. `OwnershipRewrite` (Prong A) then filters those six
views with a direct, prune-friendly predicate on their own `audience` column instead of injecting a
`process_id IN (SELECT ... FROM __processes__partitions ...)` semi-join whose subquery runs
`property_get(properties, 'micromegas.audience')` over dictionary-encoded JSONB for every process
row.

This is not a correctness fix — all six views are already access-controlled. It buys three things:
(1) the semi-join and the per-row JSONB extraction disappear from every query plan touching those
views; (2) the audience travels with the rows it governs, removing today's cross-view
materialization-freshness dependency (a `log_entries` row is currently invisible until the
*separate* `processes` view has also caught up); (3) it unlocks the partition pruning and
per-audience object-storage prefixing that step 15 of
`tasks/data_isolation/audience_based_access_control_plan.md` lists as enabled-by this change.

**No history is lost.** The three big views (`blocks`, `log_entries`, `measures`) keep their
existing file-schema hashes and read the new column as `NULL` on pre-existing partitions, via
DataFusion's schema evolution. Only the three `SqlBatchView`s (`processes`, `streams`,
`log_stats`) bump — unavoidably, since their hash is derived from their inferred schema — and all
three are fully regenerable over the whole history precisely *because* their sources stayed
readable. See [Migration](#migration) for the one operational precondition this rests on.

## Current State

### Where the audience lives today

Postgres `processes.properties` (`micromegas_property[]`, written server-side at registration
since Stage 5 / #1373 — `rust/ingestion/src/sql_telemetry_db.rs:39`) is the single origin. Two
readers consume it:

- **Prong B** (`rust/analytics/src/lakehouse/audience_guard.rs:141-176`) resolves it straight from
  Postgres with a `LEFT JOIN LATERAL (SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1)`,
  behind a TTL-bounded `moka` cache. `AUDIENCE_PROPERTY` (`audience_guard.rs:47`) is the shared
  constant.
- **Prong A** (`rust/analytics/src/lakehouse/ownership_rewrite.rs`) reads the *materialized*
  copy. `OwnershipRewrite::audience_col()` (`:170-178`) is
  `cast(property_get(col("properties"), AUDIENCE_PROPERTY), Utf8)`, aggregated per process by
  `per_process_audience()` (`:184-197`) as `Aggregate(GROUP BY process_id, MAX(audience_col))` over
  the raw `__processes__partitions` scan, then filtered by `resolved_predicate()` (`:216-238`) as
  `coalesce(resolved_audience, unstamped_audience) IN (caller audiences)`.

`predicate_for()` (`:301-372`) branches per view set:

| Branch | View sets | Shape |
|---|---|---|
| §7 | anything in `public_view_sets` | no predicate |
| §3 | `processes` | `process_id IN (subquery)` against its own resolved aggregate |
| §4 | any view whose file schema has a `process_id` field | `cast(process_id, Utf8) IN (subquery)` semi-join |
| §5 | `async_events` | literal-valued `EXISTS` keyed on `view_instance_id` |
| §6 | `thread_spans` | two-hop literal `EXISTS` through `streams` |
| — | anything else | `Err(DataFusionError::Plan)` |

### How the six global views are materialized

The propagation chain is already in place for `properties`; `audience` can ride it.

```
Postgres processes/streams/blocks
   │  BlocksView::data_sql  (blocks_view.rs:59-70)   ← one SQL SELECT, per insert-hour
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
  `blocks_view_schema()` as its declared schema (`write_partition.rs:924-927`). Column **names**
  therefore need not match (they already don't — `processes.properties as process_properties` in
  SQL vs. the `processes.properties` field), but **order and type must**. A PG `TEXT` column maps
  to nullable `Utf8` (`sql_arrow_bridge.rs:322-325`). `blocks_file_schema_hash()` is hand-written
  (`blocks_view.rs:308-310`, currently `vec![3]`).
- **`processes`/`streams`/`log_stats`** are `SqlBatchView`s. Their schema is *inferred* from the
  transform query at startup (`sql_batch_view.rs:118-121`) and their file schema hash is a hash of
  that schema (`:296-300`) — so adding a column to a transform query bumps the hash automatically,
  no constant to edit. Each registers two tables: the raw `__<name>__partitions` scan (what
  `OwnershipRewrite` actually rewrites) and the merged query under the bare name.
- **`log_entries`/`measures`** are `BlockPartitionSpec`s. Their schemas are hand-written
  (`log_entries_table.rs:24-83`, `metrics_table.rs:18-88`) with `const SCHEMA_VERSION`
  (`log_view.rs:37` = 6, `metrics_view.rs:39` = 7). Rows are built by
  `LogEntriesRecordBuilder`/`MetricsRecordBuilder` from `ProcessMetadata`
  (`metadata.rs:38-52`) — `fill_constant_columns` fills the per-block constant columns once per
  block. **Two** independent builder sets exist per view: the shared record builder and a
  hand-rolled duplicate in the OTel processors
  (`lakehouse/otel/logs_block_processor.rs:222-240`, `otel/metrics_block_processor.rs:377`).
- `ProcessMetadata` is built at three sites: `process_metadata_from_row`
  (`metadata.rs:227-247`, from a PG row — used by `find_process`, the JIT/per-process path),
  `find_process_with_latest_timing` (`metadata.rs:366-380`, from the `processes` view), and
  `partition_source_data.rs:208-220` (from a `blocks` partition batch — the global-instance path).

### What a schema-hash bump costs, and why we avoid it

`MaterializedView::scan` fetches partitions by **exact** `file_schema_hash`
(`materialized_view.rs:73-81`, `partition_cache.rs:238`), so a bump makes every pre-existing
partition of that view invisible rather than automatically rebuilt. Regeneration is bounded by
retention: `delete_old_data` (`delete.rs:152`) deletes Postgres `blocks`/`streams`/`processes`
rows **and the payload blobs** (`delete.rs:38-41`) past `MICROMEGAS_RETENTION_DAYS` (default 90,
`rust/monolith/src/main.rs:161`), while lakehouse parquet partitions are never retention-deleted.
Beyond that horizon the parquet partitions are the only surviving copy, so a bumped view's older
history can never be rebuilt — it is simply gone from query results.

JIT (per-process / per-stream) instances are the exception: they rebuild on first query after a
bump — the #1429 / #1478 precedent. It is the *global* instances that lose history.

### Schema evolution is available, and is what lets us skip the bump (verified)

DataFusion 54.1 (`rust/Cargo.toml:50`) fills a table-schema column that is missing from a Parquet
file with nulls. The mechanism is `DefaultPhysicalExprAdapterFactory`
(`datafusion-physical-expr-adapter-54.1.0/src/schema_rewriter.rs:405-416`); the old
`SchemaAdapter`/`DefaultSchemaAdapterFactory` path is a deprecated skeleton in this version that
returns `not_impl_err`, so this is the live mechanism, not the legacy one.
`datafusion-datasource-parquet-54.1.0/src/source.rs:548-551` falls back to it when the
`FileScanConfig` names no factory — and nothing under `rust/*/src` sets one, so both scan paths
(`MaterializedView` and `PartitionedTableProvider`, which share
`make_partitioned_execution_plan` → `ParquetSource`) get it. `source.rs:265-266` states the
contract directly: "By default missing columns are filled with nulls."

One hard constraint: `schema_rewriter.rs:405-411` **errors** on a missing *non-nullable* column
("Non-nullable column '{}' is missing from the physical schema"). The new field must therefore be
declared nullable — which it is, for independent reasons. Getting this wrong fails every scan of
every pre-existing partition, so it is worth an explicit test.

## Design

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
Field::new("audience", DataType::Utf8, true),   // appended last
```

`NULL` means "no audience is recorded for this row", which covers two cases that this design
deliberately treats identically:

1. the owning process carries no `micromegas.audience` property (`OwnerAudience::Unstamped`);
2. the row lives in a partition written before this column existed (null-filled by schema
   evolution — §6).

Both resolve through the existing `MICROMEGAS_UNSTAMPED_AUDIENCE` knob, which is exactly the
right answer for both: case 1 is what the knob is *for*, and case 2 is data that predates any
restricted audience and so is genuinely unstamped. A sentinel value distinguishing the two would
buy nothing, because there is no treatment we would want to differ. What makes this safe is not a
property of the encoding but the ordering precondition in §6.

The property name comes from a shared constant, not an inlined literal. Move `AUDIENCE_PROPERTY`
out of `lakehouse/audience_guard.rs` into a new top-level `rust/analytics/src/audience.rs` (three
consumers now: `audience_guard.rs`, `blocks_view.rs`, `metadata.rs`, the last of which is not under
`lakehouse`), alongside the scalar-subselect fragment the two new SQL sites share:

```rust
/// `(SELECT value FROM unnest(<properties_expr>) WHERE key = '<AUDIENCE_PROPERTY>' LIMIT 1)`
pub fn audience_subselect(properties_expr: &str) -> String;
```

`audience_guard.rs`'s existing `LEFT JOIN LATERAL` form is equivalent; leave it (an optional DRY
follow-up, not part of this change).

### 2. Propagation into `processes` / `streams` / `log_stats`

Append one aggregate to each transform **and** merge query:

- `processes_view.rs`: `max("audience") as audience`, appended **last** in the SELECT list of both
  the transform and the merge query (after `last_block_end_time`), so the inferred schema grows
  only at the end.
- `streams_view.rs`: same, appended after `last_update_time`.
- `log_stats_view.rs`: `max(audience) as audience`, appended after `count`. Left out of the
  `GROUP BY` on purpose: audience is functionally determined by `process_id`, so grouping on it
  cannot change row counts today, and keeping the `GROUP BY` key list untouched keeps the declared
  `(time_bin, process_id, level, target)` merge sort order (`log_stats_view.rs:84-89`) exactly as
  it is.

`max()` rather than the neighbouring `first_value()`: `max` over a nullable column ignores
`NULL`s, so a stamped row always outranks an unstamped one — the same reasoning
`OwnershipRewrite::per_process_audience` documents today, now performed once at materialization
instead of on every query. Note it in a comment at each site.

### 3. Propagation into `log_entries` / `measures`

- `ProcessMetadata` (`metadata.rs:38-52`) gains `pub audience: Option<Arc<str>>`. A required field
  with no default, so the compiler enumerates every construction site (the project's stated
  preference in `CLAUDE.md`'s "Interface stability" section).
  - `process_metadata_from_row` — add `audience` to `find_process`'s SELECT using
    `audience_subselect("properties")`, read it as `Option<String>`. This is the JIT /
    per-process path.
  - `find_process_with_latest_timing` — add `audience` to its `SELECT ... FROM processes` (the
    view now has the column) and read it with `string_column_by_name` + `is_null`.
  - `partition_source_data.rs` — read the new `audience` column off the `blocks` batch, add
    `"audience"` to `fetch_partition_source_data`'s projection list (`:255-260`). This is the
    global-instance path.
- `log_table_schema()` / `metrics_table_schema()` each gain a trailing
  `Field::new("audience", Dictionary(Int32, Utf8), true)` — dictionary-encoded, matching the
  `process_id` treatment in the same schemas and giving the filter the cheap key comparison the
  issue asks for.
- `LogEntriesRecordBuilder` / `MetricsRecordBuilder` gain an `audiences: StringDictionaryBuilder<Int32Type>`:
  `append_option` in `append`, and in `fill_constant_columns` `append_n(a, n)` for `Some` /
  `append_nulls(n)` for `None`. Same two lines in the two hand-rolled OTel builders.
- `SCHEMA_VERSION`: `log_view.rs` 6 → 7, `metrics_view.rs` 7 → 8.
- `blocks_file_schema_hash()`: `vec![3]` → `vec![4]`.

### 4. `OwnershipRewrite`: a new first branch, keyed on the column's presence

`predicate_for` gains a branch ahead of §3/§4, keyed on
`view.get_file_schema().field_with_name("audience").is_ok()` — the same schema-introspection
style as the existing `process_id` test, so a view set that gains the column later (the JIT views,
see [Future work](#future-work)) upgrades automatically with no edit here:

```rust
// §2 (new): views carrying a physical `audience` column -- processes, streams, blocks,
// log_entries, measures, log_stats. Filtered directly, no semi-join, no property_get.
if view.get_file_schema().field_with_name("audience").is_ok() {
    return Ok(Some(self.audience_column_predicate(table_name)));
}
```

with

```rust
/// `audience IN (caller audiences)`, plus `OR audience IS NULL` when the deployment's
/// `unstamped_audience` is itself one of the caller's audiences. Deliberately *not*
/// `coalesce(audience, unstamped) IN (...)`: whether the unstamped default is in scope is known
/// here, at analyze time, so the predicate collapses to two forms that Parquet's
/// `PruningPredicate` can both evaluate against row-group statistics (`IN` on a
/// dictionary column, and null counts), which `coalesce` cannot.
fn audience_column_predicate(&self, table_name: &TableReference) -> Expr {
    let audiences = self.audiences();
    if audiences.is_empty() {
        return lit(false);          // fail-closed, same as resolved_predicate today
    }
    let raw = Expr::Column(Column::new(Some(table_name.clone()), "audience"));
    // Cast for the same reason every other expression in this file is cast: this rule runs
    // after DataFusion's own TypeCoercion pass, and `audience` is Dictionary(Int32, Utf8) in
    // log_entries/measures/log_stats but Utf8 in blocks/processes/streams.
    let in_list = cast(raw.clone(), DataType::Utf8).in_list(
        audiences.iter().map(|a| lit(ScalarValue::Utf8(Some(a.clone())))).collect(),
        false,
    );
    let unstamped_in_scope = self
        .unstamped_audience
        .as_ref()
        .is_some_and(|u| audiences.iter().any(|a| a == u));
    if unstamped_in_scope { in_list.or(raw.is_null()) } else { in_list }
}
```

This is semantically identical to today's `coalesce(resolved_audience, unstamped) IN (audiences)`
for every case the invariant in §5 admits, including the empty-audience-set and
unstamped-not-configured cases.

Consequences inside the file:

- **§3 (`processes`) is deleted.** `processes` now carries the column and falls into the new
  branch, as the first member rather than a special case.
- **§4 shrinks** to the views that still have `process_id` but no `audience`: `net_spans`,
  `otel_spans`, `images`. It keeps today's semi-join, unchanged.
- **§5/§6 keep their `EXISTS` shapes**, but `per_process_audience()` now aggregates
  `max(col("audience"))` off `__processes__partitions` instead of
  `max(property_get(properties, ...))`. `audience_col()` and the `PropertyGet` /
  `AUDIENCE_PROPERTY` imports are removed from this file. `processes_source`/`streams_source` and
  their `make_session_context` plumbing stay — §4/§5/§6 still need them.
- The module doc comment's branch table and its "One audience per process, not per row" section
  need rewriting (see [Documentation](#documentation)); the latter's hazard is now handled at
  materialization time by §2's `max()`, and the argument for why per-row filtering is now sound
  is §5 below.

### 5. Why per-row filtering is sound (the invariant this rests on)

Today's aggregate exists because `__processes__partitions` can hold several rows per
`process_id` — one per materialized partition — and filtering them individually would let a
process admitted once by a stale, unstamped row stay visible to the unstamped audience forever.
The new branch filters rows individually. That is sound because **a process's audience in
Postgres is write-once**: it is written at registration and there is no `UPDATE processes` path
anywhere in the tree; Stage 5's conflict guard rejects a same-`process_id` re-registration under a
different audience outright, and its `NULL` → no-op branch means an unstamped row is never
retro-stamped. Every materialized row for a given process therefore snapshots the same PG value,
so per-row and `MAX`-per-process filtering agree.

Two edge cases, both documented rather than defended against:

- **Retention-delete then re-register** (possible for OTLP, whose `process_id` derivation is
  deterministic): old partitions carry the old audience, new ones the new audience. Per-row
  filtering hands each row to the audience that actually produced it — strictly better than
  `MAX`, which hands *all* rows to whichever label sorts higher.
- **Partitions written before the column existed** carry `NULL` and are therefore treated as
  unstamped. This is correct exactly while no such partition contains rows of a process stamped
  with a restricted audience — see §6's precondition, which is what this rests on.
- **A stamped process with rows in a pre-column partition** is where the two prongs diverge:
  Prong A reads `NULL` and applies the unstamped rule, while Prong B resolves from Postgres and
  sees the real stamp. The window is every process registered after Stage 5 (#1373) shipped but
  materialized before this change. It is benign today — every such stamp is `public`, and
  `MICROMEGAS_UNSTAMPED_AUDIENCE` defaults to `public`, so both prongs reach the same verdict —
  and §6's precondition is exactly what keeps it benign. Worth an explicit test (see
  [Testing Strategy](#testing-strategy)) rather than left as a reasoned assumption, and worth a
  line in `ownership_rewrite.rs`'s doc comment: it is a narrower, time-bounded instance of the
  cross-prong skew that file already discusses.

### 6. Migration: no bump on the big three, forced bump on the three `SqlBatchView`s

**`blocks`, `log_entries`, `measures` keep their hashes.** `blocks_file_schema_hash()` stays
`vec![3]`; `log_view.rs`'s `SCHEMA_VERSION` stays 6; `metrics_view.rs`'s stays 7. Their
pre-existing partitions remain queryable and mergeable, and read `audience` as `NULL` (schema
evolution, verified above). Nothing to regenerate, no history lost, no operator action. These are
the three with hand-written hashes, and — not coincidentally — the three whose data volume and
retention-bounded irreproducibility make a bump expensive.

**`processes`, `streams`, `log_stats` bump automatically.** Their hash is
`DefaultHasher` over the schema inferred from the transform query (`sql_batch_view.rs:296-300`),
so adding a column changes it. There is no way to avoid this short of overriding
`get_file_schema_hash` to ignore part of the schema, which would break the mechanism's whole
purpose. Their pre-existing partitions go invisible on deploy and need
`regenerate_partitions(...)` — but all three are regenerable over the *entire* history, because
their sources were not bumped:

| View | Source | Regeneration cost |
|---|---|---|
| `processes` | `blocks` partitions (intact) | cheap — metadata-sized scan per 1-day partition, one row per process |
| `streams` | `blocks` partitions (intact) | cheap — same shape |
| `log_stats` | `log_entries` partitions (intact) | **the expensive one** — re-aggregates all `log_entries` history into 1-minute bins |

`processes` is worth its bump: it is what lets `per_process_audience()` drop `property_get`, which
is the last JSONB extraction on the §5/§6 `EXISTS` paths. `log_stats` carries the only real bill
and still ships (Open Question 2) — but its regeneration can be run lazily and out of band: its
pre-bump partitions are simply invisible until regenerated, and it is a derived rollup, always
recomputable from `log_entries`. Nothing else depends on it.

#### The precondition

Treating `NULL` as unstamped is correct only while **no partition written before this change
contains rows belonging to a process stamped with a restricted (non-`public`) audience.** So:

> **Ship the column before minting the first ingestion key with a non-`public` audience.**

That holds today — every existing `ingestion_api_keys` row was backfilled to `public` by migration
v6 (#1372), so every process on record is either stamped `public` or unstamped, and
`MICROMEGAS_UNSTAMPED_AUDIENCE` defaults to `public`. Both readings of `NULL` therefore resolve to
`public`, which is the truth.

The invariant is self-maintaining once satisfied, for three independent reasons: partitions are
immutable, so an old partition can never acquire restricted rows; every partition written after
the deploy carries a real audience; and Stage 5's `NULL`→no-op branch means an already-registered
unstamped process is never retro-stamped, so an old process stays unstamped forever.

If it were violated — a restricted key minted before the column ships — that key's rows sitting in
pre-column partitions would read `NULL` and be treated as `public`, i.e. world-readable. The
mitigation if that ever happens is the bump-and-regenerate path this section replaces, applied to
the affected view only. This is an operational precondition, not a structural guarantee, and it
belongs in `ownership_rewrite.rs`'s module doc comment stated in exactly those terms — the file
already documents its residual gaps this way.

Two smaller consequences worth recording:

- Because `public` is the sole built-in read grant every authenticated principal holds, the
  `OR audience IS NULL` disjunct (§4) is present in practically every plan for as long as
  pre-column partitions exist. Correct, but it means **audience-based partition pruning will not
  eliminate those partitions** — pruning was already a follow-up, not this change's win.
- Merges spanning the deploy boundary mix null-filled and populated source rows into one output
  partition written under the current schema. Harmless: the null rows are public either way.

## Implementation Steps

### Phase 1 — the column, materialized (no enforcement change yet)

1. New `rust/analytics/src/audience.rs`: move `AUDIENCE_PROPERTY` here from
   `lakehouse/audience_guard.rs`, add `audience_subselect()`. Update `lib.rs`,
   `audience_guard.rs`, and `ownership_rewrite.rs`'s import.
2. `lakehouse/blocks_view.rs`: append the subselect to `data_sql`, append
   `Field::new("audience", Utf8, true)` to `blocks_view_schema()`. **Leave
   `blocks_file_schema_hash()` at `vec![3]`** — the nullable field is null-filled on existing
   partitions (§6). Nullability is load-bearing: a non-nullable missing column is a scan error.
3. `lakehouse/processes_view.rs`, `lakehouse/streams_view.rs`: append `max("audience") as audience`
   to transform + merge queries.
4. `metadata.rs`: add `ProcessMetadata::audience`; populate it in `process_metadata_from_row`
   (extend `find_process`'s SELECT) and in `find_process_with_latest_timing` (extend its SELECT).
5. `lakehouse/partition_source_data.rs`: add `"audience"` to `fetch_partition_source_data`'s
   projection and read it into `ProcessMetadata`.
6. `log_entries_table.rs`, `metrics_table.rs`: schema field + builder field + `append` /
   `fill_constant_columns` / `finish`.
7. `lakehouse/otel/logs_block_processor.rs`, `lakehouse/otel/metrics_block_processor.rs`: the same
   two lines in each hand-rolled builder.
8. **Do not touch** `SCHEMA_VERSION` in `lakehouse/log_view.rs` (stays 6) or
   `lakehouse/metrics_view.rs` (stays 7). Both new fields must be nullable.
9. `lakehouse/log_stats_view.rs`: append `max(audience) as audience` to transform + merge queries.
   This is the one step carrying a real regeneration bill (§6), but it ships — the documented
   "every global view carries `audience`" contract (Open Question 2) rules out leaving it out.
10. Fix the compile fallout in test fixtures that build `ProcessMetadata` literals or
    `blocks_view_schema()`-shaped batches (`tests/test_helpers.rs`, `tests/time_tests.rs`,
    `tests/block_chain_grouping_tests.rs`, `tests/jit_*_tests.rs`,
    `tests/blocks_view_merge_ordering_tests.rs`).

At the end of this phase the column exists and is populated; `OwnershipRewrite` still uses the
semi-join, so the change is observable only as a new column in `SELECT *`.

### Phase 2 — switch enforcement to the column

11. `lakehouse/ownership_rewrite.rs`: add `audience_column_predicate` and the new branch; delete
    §3 and `audience_col()`; repoint `per_process_audience()` at `col("audience")`; drop the
    `PropertyGet` / `AUDIENCE_PROPERTY` imports; rewrite the module doc comment's branch table and
    the "One audience per process" section.
12. Update `tests/ownership_rewrite_public_view_set_tests.rs` — it asserts optimized plan shapes
    for `SELECT * FROM blocks/streams/processes`, which no longer contain a join.
13. Extend `tests/ownership_rewrite_db_test.rs` (see [Testing Strategy](#testing-strategy)).

### Phase 3 — docs and changelog

14. Documentation updates listed below, plus the CHANGELOG entry with its **Operational note** and
    **Minor breaking change** clause (`ProcessMetadata` gains a required public field).
15. Mark step 15 of `tasks/data_isolation/audience_based_access_control_plan.md` as landed and
    point it at this plan.

## Files to Modify

**New**
- `rust/analytics/src/audience.rs`

**Analytics — materialization**
- `rust/analytics/src/lakehouse/blocks_view.rs`
- `rust/analytics/src/lakehouse/processes_view.rs`
- `rust/analytics/src/lakehouse/streams_view.rs`
- `rust/analytics/src/lakehouse/log_stats_view.rs`
- `rust/analytics/src/lakehouse/log_view.rs`
- `rust/analytics/src/lakehouse/metrics_view.rs`
- `rust/analytics/src/lakehouse/partition_source_data.rs`
- `rust/analytics/src/lakehouse/otel/logs_block_processor.rs`
- `rust/analytics/src/lakehouse/otel/metrics_block_processor.rs`
- `rust/analytics/src/log_entries_table.rs`
- `rust/analytics/src/metrics_table.rs`
- `rust/analytics/src/metadata.rs`
- `rust/analytics/src/lib.rs` (module declaration)

**Analytics — enforcement**
- `rust/analytics/src/lakehouse/ownership_rewrite.rs`
- `rust/analytics/src/lakehouse/audience_guard.rs` (constant moved out)

**Tests**
- `rust/analytics/tests/ownership_rewrite_db_test.rs`
- `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`
- `rust/analytics/tests/blocks_view_merge_ordering_tests.rs`
- `rust/analytics/tests/test_helpers.rs`, `time_tests.rs`, `block_chain_grouping_tests.rs`,
  `jit_partition_grouping_tests.rs`, `jit_freshness_tests.rs`, `jit_partition_bounds_tests.rs`

**Docs**
- `mkdocs/docs/query-guide/schema-reference.md`
- `doc/how_to_query/README.md`
- `mkdocs/docs/admin/authentication.md`
- `rust/analytics/src/lakehouse/view_factory.rs` (module doc schema tables)
- `CHANGELOG.md`
- `tasks/data_isolation/audience_based_access_control_plan.md`

Explicitly **not** modified: Postgres schema (`rust/ingestion/src/sql_telemetry_db.rs`) — see
Trade-offs; `net_spans`/`otel_spans`/`images`/`async_events`/`thread_spans` views — out of scope
per the issue.

## Trade-offs

- **Fill legacy rows with the default audience instead of `NULL`.** Possible — DataFusion lists
  "filling in non-null default values" as a first-class use case for a custom
  `PhysicalExprAdapter` (`schema_rewriter.rs:52-53`, with a worked example at `:101-130`), passed
  to `FileScanConfigBuilder` in place of the default factory. **Rejected: the complexity is
  permanent, the value is transitional.** A custom adapter is code on every scan path forever,
  while the only thing it serves — partitions written before this change — is a fixed set that
  never grows and matters less every day. Three further reasons, in case the trade ever looks
  tempting again:
  - *It doesn't collapse the two meanings of `NULL`, which is the only thing it would be for.*
    A genuinely unstamped process still materializes `NULL` on new partitions, so the
    `OR audience IS NULL` disjunct stays either way. Removing it needs `NULL` never to be written
    at all — i.e. writing the default at *materialization* time — which is the next two problems
    in their worst form.
  - *It writes a policy value into a data column.* `unstamped_audience` is deployment policy;
    "this process has no stamp" is a fact about the data. Today `NULL` + the query-time coalesce
    keeps the knob governing **all** unrecorded data uniformly, so changing it (e.g. to the
    fail-closed empty string) reinterprets everything at once. Baking the current value in — at
    read time, and permanently if done at write time — makes the knob's reach depend on when a
    partition happened to be written.
  - *It desynchronizes the two prongs, and asserts something false while doing it.* Prong B keeps
    `OwnerAudience::Unstamped` as a distinct variant resolved from Postgres and passes it only
    when the knob is configured and in scope (`audience_guard.rs` module doc). A pre-column row
    whose process *is* stamped in Postgres (§5's third edge case) would be filled with the
    *default* — not the process's real audience — so Prong A would confidently report `public` for
    a row Prong B correctly resolves to its actual audience. `NULL` reports "not recorded here",
    which is true; a filled literal reports a specific audience, which may not be. Absence is the
    honest encoding, and the two prongs reading different copies is a hazard the #1370 plan
    already singles out (its §11).
- **Rely on schema evolution vs. bump the hashes.** Chosen: schema evolution, for the three views
  where we have the choice. The bump's cost is permanent, retention-bounded history loss on the
  three largest views; the null-fill's cost is a `NULL` that means two things at once. That
  ambiguity would be a real confidentiality hazard — a restricted process's rows in a pre-column
  partition read as `public` — except that it is unreachable given §6's ordering precondition,
  which holds today and is self-maintaining once satisfied. Trading a permanent data loss for a
  precondition that is already true is the right side of that trade. Rejected alternatives for
  disambiguating without the precondition: a written sentinel for "unstamped" plus a permanent
  `OR (audience IS NULL AND process_id IN (subquery))` fallback — which keeps the semi-join in
  every plan and so defeats the point of the issue; or a new "compatible legacy schema hashes"
  mechanism in `QueryPartitionProvider`/`PartitionCache`, a much larger change that is worth
  building only if this deployment's assumptions stop holding.
- **`log_stats`: take the column, or leave it on the semi-join.** It is the only view whose forced
  bump carries a real bill (re-aggregating all `log_entries` history), and leaving it out would
  cost only that one view's cheap path — it keeps `process_id`, so §4 still covers it correctly.
  **Decided: include it.** Documenting `audience` as a stable column on the global views (Open
  Question 2) makes a five-of-six surface an inconsistency users will hit, which outweighs the
  one-off regeneration. The regeneration can still be run lazily — `log_stats` partitions are
  invisible until regenerated, and it is a derived rollup, always recomputable from `log_entries`.
- **Extract once in the `blocks` SQL vs. per-view `property_get` in each transform query.**
  `processes`/`streams` could compute `property_get(first_value("processes.properties"), ...)`
  themselves and `log_entries`/`measures` could extract from `ProcessMetadata.properties` in Rust.
  Rejected: it duplicates the extraction in four places, violating the single-definition discipline
  `AUDIENCE_PROPERTY` already establishes, and it buys nothing now that `blocks` isn't bumping
  either way. (This alternative existed only to spare `blocks`' hash.)
- **`max()` vs. `first_value()` in the `SqlBatchView` transforms.** `first_value` is what the
  neighbouring columns use, but it is order-dependent and would let an unstamped snapshot win.
  `max` ignoring `NULL`s reproduces exactly what `OwnershipRewrite` does today.
- **`audience IN (...) OR audience IS NULL` vs. `coalesce(audience, unstamped) IN (...)`.** The
  coalesce form is a one-line port of the existing predicate but is opaque to Parquet's
  `PruningPredicate`. Since "is the unstamped default in scope" is known at analyze time, the
  disjunction costs nothing and keeps both halves prunable.
- **No Postgres `processes.audience` column.** A stored column written by ingestion would make the
  extraction free and would also serve Stage 5b's write-path check (denormalizing audience onto
  `streams`/`blocks` so `insert_stream`/`insert_block` can verify it). Deliberately deferred: it
  is a schema migration plus a backfill on the ingestion side, it belongs with Stage 5b's own
  design, and the query-side extraction it would optimize now happens once per partition rather
  than once per query.

## Documentation

- `mkdocs/docs/query-guide/schema-reference.md` — add the `audience` row to the `processes`,
  `streams`, `blocks`, `log_entries`, `log_stats`, and `measures` field tables (last row, matching
  physical order). This is a **documented, stable column**, so the prose around it carries real
  weight and should say four things:
  - what it is: the audience of the owning process, written server-side from the authenticated
    ingestion credential; never client-settable.
  - `Utf8` on `processes`/`streams`/`blocks`, `Dictionary(Int32, Utf8)` on
    `log_entries`/`measures`/`log_stats`. Both compare against string literals normally; the
    difference matters only to a client reading Arrow types directly (the
    `preserve_dictionary=True` python path).
  - **nullable, and `NULL` has two causes**: the owning process was never stamped, or the row
    predates this column. Both are treated as the deployment's `MICROMEGAS_UNSTAMPED_AUDIENCE`.
    Spell this out with the consequence: `WHERE audience = 'x'` silently excludes `NULL` rows, so
    a user auditing coverage wants `WHERE audience = 'x' OR audience IS NULL` (or
    `GROUP BY audience` to see the split).
  - it is **not a filter the user needs to apply**: enforcement is already unconditional, and a
    caller can only ever see rows whose audience is in their read scope. The column is for
    observability — "whose data is this, how much of each" — not for a user-authored access check.
- `doc/how_to_query/README.md` — the same tables are duplicated here; keep them in sync.
- `mkdocs/docs/admin/authentication.md` — in the audience/data-isolation section: the audience is
  now a physical column on the global views, the query-time property lookup is gone from those
  plans, and the re-materialization procedure from [Migration](#migration).
- `rust/analytics/src/lakehouse/view_factory.rs` — the module doc's per-view schema tables.
- `rust/analytics/src/lakehouse/ownership_rewrite.rs` — module doc: new branch table, rewritten
  "One audience per process" section (the resolution moved to materialization; the per-row
  soundness argument of Design §5 replaces it), a note that `audience_col`/`property_get` are
  gone, and — in the same register as the file's existing residual-gap admonitions — the §6
  precondition: pre-column partitions read `audience` as `NULL` and are treated as unstamped, which
  is sound only because no restricted audience existed when they were written.
- `CHANGELOG.md` — Unreleased → Analytics, with an **Operational note** covering: the three
  `SqlBatchView`s needing `regenerate_partitions` (and that `blocks`/`log_entries`/`measures` do
  **not**, reading `NULL` on old partitions by schema evolution), and the ordering precondition —
  ship this before minting the first non-`public` ingestion key. Plus a **Minor breaking change**
  clause (`ProcessMetadata`, a published all-public-field struct, gains a required `audience`
  field).

## Testing Strategy

- **`tests/ownership_rewrite_db_test.rs`** is the acceptance vehicle and already seeds processes
  stamped with different audiences (plus one unstamped) through the real ingestion pipeline, then
  asserts visible rows per `ReadScope`. Its existing assertions should pass **unchanged** — that
  is the primary evidence the semantics did not move. Add:
  - assertions that the new `audience` column is present and carries the expected value on each of
    the six views (including `NULL` for the unstamped process);
  - an explicit check that a stamped process's rows are invisible to a caller holding only
    `MICROMEGAS_UNSTAMPED_AUDIENCE`, and that the unstamped process's rows *are* visible to it —
    the pair that would break if `NULL` ever regained a second meaning.
- **`tests/ownership_rewrite_public_view_set_tests.rs`** — update the plan-shape assertions for
  `blocks`/`streams`/`processes` and add one asserting the six views' optimized plans contain
  **no** join / no `property_get`, which is the regression test for the optimization itself.
- **Unit-level**: a pure test over `audience_column_predicate` covering the four combinations of
  (empty audience set, unstamped configured / in scope / not in scope).
- **Schema evolution — the load-bearing new test.** Write a partition under the *old*
  `blocks`/`log_entries` schema (no `audience` field), then query it through the new schema and
  assert the rows come back with `audience = NULL` rather than an error. This is what proves the
  no-bump decision, and it is also the guard against someone later declaring the field
  non-nullable, which would turn every pre-existing partition into a scan failure.
  `tests/blocks_view_merge_ordering_tests.rs` already has the partition-writing scaffolding to
  build this on.
- **Prong A / Prong B agreement on `NULL`.** Build the §5 third-edge-case shape — a process
  stamped in Postgres whose rows sit in a pre-column partition — and assert Prong A (a
  `log_entries` scan) and Prong B (`process_spans` / `get_payload` on the same process) reach the
  same visible/not-visible verdict for the same caller, both with `MICROMEGAS_UNSTAMPED_AUDIENCE`
  set to `public` and with it set to the empty string. This is the test that would catch a future
  well-meaning change to fill `NULL` with a default value (see Trade-offs).
- **Migration rehearsal**: against a stack with pre-change partitions, deploy, confirm
  `blocks`/`log_entries`/`measures` still return their full history with `audience` null, then run
  `regenerate_partitions` for `processes`/`streams` and confirm they come back populated.
- **`tests/blocks_view_merge_ordering_tests.rs`** — fixtures build `blocks_view_schema()` batches;
  extend them and confirm the merge path carries the column through.
- **Python integration** (`python/micromegas/tests/`) — run the existing suite against a locally
  started stack (`local_test_env/ai_scripts/start_services.py`) after a fresh ingest, to confirm
  the new column appears and nothing regressed on `SELECT *` paths.
- **Manual**: `micromegas-query "SELECT audience, count(*) FROM log_entries GROUP BY audience"`
  and an `EXPLAIN` of a filtered `log_entries` query, to eyeball the plan before/after.

## Future work

- Give the JIT views (`net_spans`, `otel_spans`, `images`, `async_events`, `thread_spans`) the same
  column. They cost nothing to bump — JIT partitions rebuild on first query — and the new
  `OwnershipRewrite` branch picks them up automatically with no rule edit, collapsing §4/§5/§6 to
  nothing and letting `processes_source`/`streams_source` and the whole subquery machinery be
  deleted. Worth its own issue.
- Partition pruning: the column alone buys row-group pruning only for partitions that happen to be
  single-audience. The real win needs audience-homogeneous partitions — per-audience object-storage
  prefixing / one partition set per audience, which is the rest of step 15. **Constraint that work
  inherits**: `audience` is a documented stable column (Open Question 2), so promoting it to a
  storage-path or partition-key component must *keep* it on the rows rather than replace the column
  with path metadata — a `SELECT audience FROM log_entries` has to keep working.
- Stage 5b: denormalize audience onto Postgres `streams`/`blocks` so `insert_stream`/`insert_block`
  can reject a cross-audience append (`tasks/completed/1373_ingestion_stamping_plan.md` §7).
- Unify `audience_guard.rs`'s `LEFT JOIN LATERAL` with `audience_subselect()`.

## Open Questions

1. ~~**Is the retention-bounded history loss acceptable?**~~ **Resolved: avoided entirely.** All
   existing data is public and no restrictive ingestion keys have been minted, so `NULL` →
   unstamped → `public` is accurate for pre-column partitions, and the three big views need no
   bump. See §6. The only residual decision is whether to include `log_stats` now or defer it
   (Trade-offs) — a scheduling call, not a design one.
2. ~~Should the `audience` column be documented as a stable part of the SQL surface?~~
   **Resolved: yes.** It is a documented, stable column on all six global views — queryable,
   groupable, joinable — appended last so `SELECT *` and positional readers keep working, per
   `CLAUDE.md`'s SQL-interface rule. Three consequences now binding rather than optional:
   - **It cannot be quietly dropped or renamed later.** In particular the per-audience
     partitioning follow-up (step 15's remainder) must keep the column on the rows, even if the
     audience also becomes a storage-path component — see [Future work](#future-work).
   - **`log_stats` should ship with the column, not be deferred.** A documented "every global view
     carries `audience`" contract is false if five of six do, and the gap is exactly the kind of
     inconsistency a user hits while writing a rollup query. This overrides the scheduling
     argument in Trade-offs; the `log_stats` regeneration bill gets paid.
   - **The `NULL` semantics must be documented honestly**, including that it covers both an
     unstamped process and a pre-column row (§1), because it changes how a user writes a filter.
