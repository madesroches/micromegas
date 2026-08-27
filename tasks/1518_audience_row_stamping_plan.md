# Stamp the Write Audience on Every Metadata Row Plan

Issue: [#1518](https://github.com/madesroches/micromegas/issues/1518) (AbAC Stage 5b)

## Overview

Ingestion records the caller's resolved write audience on the **process** row only, as a
`micromegas.audience` entry in `processes.properties`; `streams` and `blocks` inherit it through
`process_id`. `insert_stream` and `insert_block` never look at the `AuthContext` at all, so a
credential bound to audience A that discovers a `process_id`/`stream_id` belonging to audience B
can append events to B's process, and those events are labelled B.

This plan replaces the inherited, property-carried audience with a **per-row `audience` column on
`processes`, `streams`, and `blocks`**, written from the authenticated credential at every insert.
Each row then carries the audience it was actually written under, and for every reader that
resolves audience off one of those three physical columns, no row's label is ever derived from
another row: an attacker's block carries A no matter which `process_id` it claims, so it surfaces
only to A's readers, and the victim never sees it there. The victim's process never has to be
resolvable at write time either way — which is the reason the originally-proposed write-side gate
could not work (streams and blocks routinely arrive before their process row exists; see
[Why not a write-side gate](#why-not-a-write-side-gate)). Five views that have no `audience`
column of their own, and the per-process JIT `view_instance` path, still resolve audience through
the owning process/stream row and are not closed by this change — see
["What remains process-anchored"](#what-remains-process-anchored) in §4.

Two decisions taken with the issue author, both narrowing the change:

- **Uniform treatment.** The column goes on all three tables, not just the two that lack a stamp.
  `processes` moves off the property too, so there is exactly one shape and one precedence rule
  rather than a column for two tables and a property for the third.
- **No property migration.** The whole audience-stamping stack (#1373, #1482, #1519) is still
  `## Unreleased`, and no deployed environment holds property-stamped rows. So the property is not
  backfilled into the column, is not read as a fallback, and stops being written. The only rows
  that carry a NULL column are genuinely pre-AbAC rows and admin `bulk_ingest` rows from a source
  that predates the column, and those resolve to `MICROMEGAS_DEFAULT_AUDIENCE` at read time
  exactly as they do today.

Scope is integrity, not confidentiality: no read escalation is created or removed here — reading B
still requires a read grant on B. That claim depends on `OwnershipRewrite` checking all three of
`blocks_view`'s audience columns, not just `audience`, on the `blocks` view itself — see
["The `max(audience)` regression"](#the-maxaudience-regression) in §4 for why the two appended
anchor columns would otherwise turn `blocks` into a cross-audience existence-and-label oracle.
Process squatting (`check_process_audience_conflict`) and cross-audience OTLP process collision
(audience-salted id derivation) are already closed and are untouched.

## Current State

### Write side — where the audience is and isn't recorded

| Site | File | Audience today |
|---|---|---|
| `insert_process` | `rust/ingestion/src/web_ingestion_service.rs:532-589` | `finalize_process_properties` appends `micromegas.audience` to `properties` |
| `register_otel_process` | `web_ingestion_service.rs:685-780` | same helper |
| `insert_stream` | `web_ingestion_service.rs:422-467` | **none** — no `WriteAudience` parameter |
| `register_otel_stream` | `web_ingestion_service.rs:482-514` | **none** |
| `insert_block` / `insert_block_typed` | `web_ingestion_service.rs:274-412` | **none** |
| `ingest_processes`/`ingest_streams`/`ingest_blocks` (admin replication) | `rust/analytics/src/replication.rs:21-82,86-185,187-236` | source properties copied verbatim; nothing stamped |

`insert_stream_request` and `insert_block_request` (`rust/public/src/servers/ingestion.rs:81-101`)
do not even extract the `AuthContext` extension, unlike `insert_process_request` (`:66-77`) which
takes `ctx: Option<Extension<AuthContext>>` and calls `resolve_write_audience`. The routes sit
under the global `auth_middleware`, so the extension is present on the request — the handlers just
don't read it.

`insert_block_typed` binds **both** ids verbatim from the client payload
(`web_ingestion_service.rs:338-351`) and its `INSERT INTO blocks VALUES($1..$11)` is positional
with no column list. Both `insert_block_typed` and `insert_stream` carry known-gap doc comments (`:287-296`,
`:417-421`) describing exactly this hole.

### Read side — three Postgres readers, all going through the property

`rust/analytics/src/audience.rs` owns the two SQL fragments every reader shares:

- `audience_subselect(properties_expr)` → `(SELECT value FROM unnest(<expr>) WHERE key =
  'micromegas.audience' LIMIT 1)`
- `coalesced_audience_subselect(properties_expr, param)` → that, wrapped in `COALESCE(..., $param)`

Consumers:

1. `rust/analytics/src/lakehouse/blocks_view.rs:66-83` — `data_sql` derives the block's audience
   from **`processes.properties`**, reached through `blocks.process_id = processes.process_id`.
   `streams.process_id` never enters the join, so a block claiming a foreign `process_id` is
   labelled with that foreign process's audience.
2. `rust/analytics/src/metadata.rs:275-308` — `find_process`, the JIT per-process path.
3. `rust/analytics/src/lakehouse/audience_guard.rs:166-205` — Prong B's `owner_query_sql`, one
   `LEFT JOIN LATERAL` unnest per `IdKind`; `IdKind::Block` resolves
   `block_id → blocks.process_id → processes.properties`, and the stream arm of
   `IdKind::ProcessOrStream` does the same two-hop through `streams.process_id`.

`check_process_audience_conflict` (`web_ingestion_service.rs:591-651`) is a fourth reader, in the
ingestion crate: it `SELECT properties FROM processes` and scans for the property.

### How the audience reaches the other views

`blocks_view` is the single extraction point. `processes_view` and `streams_view` are
`SqlBatchView`s built **over the `blocks` view**, and both take
`arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience` grouped by `process_id` /
`stream_id` (`processes_view.rs:43,66`, `streams_view.rs:36,52`). `log_entries`, `measures`, and
`log_stats` carry the block's audience straight through. `OwnershipRewrite` (Prong A) filters those
six views on their own physical `audience` column and resolves the other five through
`processes`/`streams`.

That `max(audience)` is safe today only because every block of a process derives the same value
from the same process row. **Per-row stamping breaks that assumption** — see
[The `max(audience)` regression](#the-maxaudience-regression) below, which is the one non-obvious
consequence of this change and drives two extra columns on `blocks_view`.

### Why not a write-side gate

The issue's original proposal was to resolve the target's owning audience (`process_id → audience`
for streams, `stream_id → process_id → audience` for blocks) and reject a mismatch. It cannot work,
because at the moment the decision must be made there is frequently nothing to resolve:

1. **Concurrent in-flight requests.** The sink drains up to `max_in_flight_requests` concurrently
   (`rust/telemetry-sink/src/http_event_sink.rs:775`) with per-item retry ladders.
   `UploadPriority::Metadata` orders the *enqueue*, not the completion — `insert_stream` can land
   while `insert_process` is still retrying.
2. **Retention.** Sweeps run bottom-up: `delete_expired_blocks` → `delete_empty_streams` →
   `delete_empty_processes` (`rust/analytics/src/delete.rs:152-170`). A long-lived process whose
   blocks have all aged out loses its row, and a subsequent block for it arrives with no anchor.

Fail-open leaves the hole exactly as it is; fail-closed rejects ordinary first-blocks and every
post-sweep block. Stamping needs no ordering guarantee, no cache, no TTL, and **no hot-path
database read at all**.

## Design

### 1. Precedence rule (stated once, in `audience.rs`)

> **A row's own `audience` column is the authoritative label for that row.** It is the
> authenticated fact recorded at the moment the row was written. A NULL column means the row
> predates this stage (or came from an admin `bulk_ingest` source that predates it) and resolves
> to the deployment's `MICROMEGAS_DEFAULT_AUDIENCE`. No row's audience is ever derived from
> another row's — not through `process_id`, not through `stream_id` — for any row that carries the
> column. A reader with no `audience` column to read still resolves through the owning
> process/stream row; see ["What remains process-anchored"](#what-remains-process-anchored) for
> which readers that is.

`check_process_audience_conflict`'s one-audience-per-process rule is unaffected: it still governs
the `processes` row and only the `processes` row.

### 2. Schema v8

`LATEST_DATA_LAKE_SCHEMA_VERSION` 7 → 8, new `upgrade_data_lake_schema_v8` in
`rust/ingestion/src/sql_migration.rs` following the v6 pattern:

```sql
ALTER TABLE processes ADD COLUMN audience VARCHAR(255);
ALTER TABLE streams   ADD COLUMN audience VARCHAR(255);
ALTER TABLE blocks    ADD COLUMN audience VARCHAR(255);
ALTER TABLE processes ADD CONSTRAINT processes_audience_name
  CHECK (audience ~ '^[A-Za-z0-9_-]+$') NOT VALID;
ALTER TABLE streams   ADD CONSTRAINT streams_audience_name
  CHECK (audience ~ '^[A-Za-z0-9_-]+$') NOT VALID;
ALTER TABLE blocks    ADD CONSTRAINT blocks_audience_name
  CHECK (audience ~ '^[A-Za-z0-9_-]+$') NOT VALID;
UPDATE migration SET version=8;
```

Deliberate properties:

- **Nullable, no `DEFAULT`, no backfill.** `ADD COLUMN` with no default is a catalog-only
  operation in Postgres 11+, so the column adds are instant even on a large `blocks` table. A
  `DEFAULT` would also let a not-yet-upgraded writer keep inserting rows that silently take a
  label, the same reason v6 refused one.
- **`CHECK ... NOT VALID`, not a plain `ADD COLUMN ... CHECK`.** A column-level `CHECK` folded into
  `ADD COLUMN` is not catalog-only: Postgres splits it into a separate `ADD CONSTRAINT`
  subcommand, and a *validated* check constraint puts the table on `ATRewriteTables`' validation
  path, which scans every existing row under `ACCESS EXCLUSIVE` before the `ALTER TABLE` commits —
  exactly the full-table lock-and-scan the v3 migration goes out of its way to avoid for `blocks`
  (`CREATE UNIQUE INDEX CONCURRENTLY`, run outside any transaction, `sql_migration.rs:288-302`).
  `NOT VALID` skips that scan: the constraint applies to rows written from this point on (which is
  all that matters here — `WriteAudience::new` already validates the charset in Rust before any
  row reaches SQL) while existing rows are simply not checked. `VALIDATE CONSTRAINT`, if ever
  wanted, is a separate statement that takes only a `SHARE UPDATE EXCLUSIVE` lock and can run
  later outside this migration. The `ingestion_api_keys_audience_name` precedent this mirrors is a
  validated check on a small, operator-populated table, where the scan cost doesn't matter; it
  does not apply unmodified to `blocks`.
- **No index.** Nothing queries Postgres *by* audience — Prong B looks rows up by primary key and
  projects the column. An index on the hot `blocks` table would be pure write cost.
- `rust/ingestion/src/sql_telemetry_db.rs`'s `create_tables` (the v1 shape) is **not** touched: a
  fresh database is created at v1 and then walks every upgrade, so adding the column in two places
  would double-apply.

### 3. Write path

Every insert binds the caller's resolved `WriteAudience`.

**HTTP handlers** (`rust/public/src/servers/ingestion.rs`) — both gain the extension and resolve it
exactly as `insert_process_request` already does:

```rust
pub async fn insert_stream_request(
    Extension(service): Extension<Arc<WebIngestionService>>,
    ctx: Option<Extension<AuthContext>>,
    body: bytes::Bytes,
) -> Result<(), IngestionError> {
    let audience = resolve_write_audience(ctx.as_ref(), service.default_audience());
    service.insert_stream(body, &audience).await.map_err(Into::into)
}
```

An unaudienced caller resolves to the deployment default — the single-state model #1519
established. No third "unstamped" state appears on the write side.

**Ingestion service** (`rust/ingestion/src/web_ingestion_service.rs`) — four methods gain an
`audience: &WriteAudience` parameter and one bind each:

- `insert_stream` — add `audience` to the existing explicit column list.
- `register_otel_stream` — same.
- `insert_block` — thread through to `insert_block_typed`.
- `insert_block_typed` — **give the `INSERT` an explicit column list** while adding the bind, in
  place of today's positional `VALUES($1..$11)`:

  ```sql
  INSERT INTO blocks (block_id, stream_id, process_id, begin_time, begin_ticks, end_time,
                      end_ticks, nb_objects, object_offset, payload_size, insert_time, audience)
  VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
  ON CONFLICT (block_id) DO NOTHING
  ```

  A positional insert into a table whose column list just grew is exactly the fragility worth
  removing while we are here.

`insert_process` and `register_otel_process` bind the column instead of appending the property:

- `insert_process`'s `INSERT INTO processes` is today a positional `VALUES($1..$13)` with no
  column list (`web_ingestion_service.rs:546`; `register_otel_process` already has an explicit
  list). A short `VALUES` list against a table whose column count just grew is silently accepted
  by Postgres, defaulting the missed bind to NULL — the row then just reads as the deployment
  default instead of the insert failing loudly. Give it an explicit column list ending in
  `audience`, the same fix as `insert_block_typed` below, for the same reason.
- `finalize_process_properties` is **removed** — it was *strip then append*, and it is the only
  call to `strip_reserved_properties` on the process write path today (`insert_stream` calls
  `strip_reserved_properties` directly; the process sites only reach it through this helper).
  Removing the helper without replacing that call would let a client re-assert arbitrary
  `micromegas.*` properties on `processes`. So `insert_process` and `register_otel_process` must
  each gain their own direct `strip_reserved_properties(...)` call at their `properties` bind site
  in place of the removed helper — a client-supplied `micromegas.*` property is still dropped, so a
  client can neither assert nor suppress a stamp; there is simply no property to append any more.
- `micromegas_telemetry::property_names::PROPERTY_AUDIENCE` becomes unused in production code and
  is removed along with its `property.rs` re-export. `RESERVED_PROPERTY_PREFIX` stays.
- `check_process_audience_conflict` reads `SELECT audience FROM processes WHERE process_id = $1`
  and compares `COALESCE(row, default)` against the incoming label — the same resolution it does
  today, one column read instead of an array scan. Its "row disappeared concurrently" arm, its
  cache, and its 403 shape are unchanged.

**OTLP** (`rust/otel-ingestion/src/handler.rs:95-145`) — `write_blocks` already carries
`audience: &WriteAudience` for `register_otel_process`. Pass the same reference to
`register_otel_stream` and `insert_block_typed`. Nothing in `identity.rs`/`block.rs` changes: id
derivation is already audience-salted and stays exactly as it is.

**Admin replication** (`rust/analytics/src/replication.rs`) — this path writes a source lake's rows
verbatim and has no credential of its own to stamp from. Today the process audience rides along
inside `properties`, so replication preserves it by accident; once it is a column, replication has
to carry it explicitly or every replicated row silently collapses to the target's default.

All three ingest functions read an `audience` column from the incoming record batch and bind it.
The `processes`, `streams`, and `blocks` views each already expose an `audience` column, so any
source built from `main` supplies it. A **missing column is a hard error**, matching the precedent
`ingest_streams` set for `format` in v4 ("Hard failure rather than a silent default so a v3 source
replicating into a v4 target surfaces the schema mismatch loudly"). `ingest_processes`'s and
`ingest_blocks`' positional `VALUES($1..$13)` / `VALUES($1..$11)` inserts (`replication.rs:122`)
both get an explicit column list for the same reason as above: a missed bind against a
just-widened table silently defaults to NULL instead of failing.

The `bulk_ingest` example in `mkdocs/docs/query-guide/python-api.md` needs the same fix for the
same reason: it hand-builds a `processes` table with no `audience` column, which the hard-error
above now rejects (see [Documentation](#documentation)).

`local_test_env/ai_scripts/import_net_blocks_from_prod.py` projects explicit column lists into
`bulk_ingest` and must add the audience to each: `process_audience → audience` for the processes
table, `stream_audience → audience` for streams, and `audience → audience` for blocks (see §4 for
those two new blocks-view columns). Note `_build_streams_table` is already missing `format`, which
`ingest_streams` requires — fix that in the same pass rather than leaving a second latent break.

### 4. Read path

**`rust/analytics/src/audience.rs`** loses both property fragments and the `AUDIENCE_PROPERTY`
re-export, and gains one column fragment:

```rust
/// `COALESCE(<table>.audience, $<param>)` -- a row's own stamp, or the deployment's
/// MICROMEGAS_DEFAULT_AUDIENCE for a row written before this stage. `qualifier` is inlined as
/// SQL text, so it must be a trusted table name or alias, never user input; the default stays a
/// bind parameter, since it is operator-supplied config.
pub fn coalesced_audience_column(qualifier: &str, param: usize) -> String {
    format!("COALESCE({qualifier}.audience, ${param})")
}
```

`DEFAULT_AUDIENCE`, `is_valid_audience`, and `default_audience_from_env` are unchanged.

**`blocks_view.rs`** — the audience now comes off the block's own row, and two columns are appended
so the derived views can anchor on their own rows too:

```sql
SELECT block_id, streams.stream_id, processes.process_id, ... ,
       COALESCE(blocks.audience, $3)    AS audience,
       COALESCE(streams.audience, $3)   AS stream_audience,
       COALESCE(processes.audience, $3) AS process_audience
FROM blocks, streams, processes
WHERE blocks.stream_id = streams.stream_id
AND blocks.process_id = processes.process_id
AND blocks.insert_time >= $1 AND blocks.insert_time < $2
ORDER BY blocks.insert_time, blocks.block_id;
```

The join itself is unchanged — relaxing the inner join to `processes` is explicitly out of scope
(see [Out of scope](#out-of-scope--follow-ups)). What changes is that the join no longer *sources*
any row's label.

`blocks_view_schema()` appends `stream_audience` and `process_audience` after the existing
`audience` field, both `Dictionary(Int32, Utf8)` and non-nullable (the `COALESCE` guarantees it),
matching the existing column exactly. Appending last keeps `SELECT *` and positional readers
working — additive, not a SQL break. `blocks_file_schema_hash()` bumps `vec![5]` → `vec![6]` to
force a rebuild.

`audience` keeps its name, type, and position, and its meaning is *the audience this block was
written under* — which for every legitimate block is the same value the old expression produced.

#### The `max(audience)` regression

This is the part the issue body does not cover, and the reason for the two extra columns.

`processes_view` and `streams_view` compute `max(audience)` over the blocks of a process/stream.
Today all those blocks derive one value from one process row, so `max` is a no-op. Under per-row
stamping they can genuinely disagree: an attacker in audience `zeta` writes one block claiming
victim `beta`'s `process_id`, and `max('beta','zeta') = 'zeta'` **relabels the victim's entire
process row** — hiding it from `beta`'s readers and exposing its metadata to `zeta`. Sourcing
`audience` from the block without fixing this would trade an integrity gap for a
confidentiality-and-availability one.

The fix is the same principle as everything else here — anchor each view's row on its own row's
stamp:

- `processes_view.rs` transform query: `max(audience)` → `max(process_audience)`.
- `streams_view.rs` transform query: `max(audience)` → `max(stream_audience)`.
- Both **merge** queries are unchanged: they read a column already named `audience` from
  `{source}`, and `max()` over rows that now agree by construction is a no-op that costs nothing to
  leave in place.
- `log_entries` and `measures` keep using `audience` unchanged — their rows come from a block's
  payload, so the block's own stamp is the correct anchor, and there is no cross-block aggregation
  to relabel a row.
- `log_stats_view.rs` has the same regression as `processes_view`/`streams_view`: its transform
  and merge queries both aggregate `log_entries` rows with `arrow_cast(max(audience), ...)`
  `GROUP BY process_id, level, target, time_bin` — a group that, after per-block stamping, can
  contain blocks from different audiences. An attacker's block landing in a victim's group
  relabels the victim's `log_stats` row, which is one of the six views Prong A filters on its own
  `audience` column. Fix: add `audience` to the `GROUP BY` in **both** the transform and merge
  queries (the selected column and schema are unchanged, so the file-schema hash does not need to
  bump).

`OwnershipRewrite` needs one change, confined to `blocks`. `blocks_view`'s join has no
`streams.process_id = processes.process_id` predicate, so an attacker in audience `alpha` can
insert a block naming its own `stream_id` (audience `alpha`, so `stream_audience='alpha'`) but a
victim's `process_id` (audience `beta`, so `process_audience='beta'`). The row materializes with
`audience='alpha'` — visible to the attacker — while carrying `process_audience='beta'`. If
`audience_column_predicate` keeps filtering `blocks` on `audience` alone, as it does for the other
five column-carrying views, `SELECT process_id, process_audience FROM blocks` becomes a
cross-audience existence-and-label oracle: the attacker learns that `beta` owns a given
`process_id` by probing it into blocks it can read. Fix: for the `blocks` table specifically,
`audience_column_predicate` requires `audience`, `stream_audience`, **and** `process_audience` all
be in the caller's read scope, not just `audience` — a legitimate block's three columns already
agree, so this is a no-op for every row that isn't itself an attempted cross-audience probe.
`processes`, `streams`, `log_entries`, `measures`, and `log_stats` carry only the single `audience`
column and keep the existing bare-column filter unchanged.

#### What remains process-anchored

This plan closes the row-derivation gap only where a row carries its own `audience` column.
Two classes of reader still resolve audience through the *owning* process/stream row, and this
plan leaves both as they are today:

- **The five views `OwnershipRewrite` resolves via `per_process_audience()`.** `net_spans`,
  `otel_spans`, and `images` are filtered through the `IN`-subquery built from
  `MAX(audience) GROUP BY process_id` over `__processes__partitions`; `async_events` and
  `thread_spans` are filtered through the equivalent `EXISTS` arms, the latter via `streams`. None
  of these five carries its own `audience` column, so a block an attacker writes onto a victim's
  `process_id`/`stream_id` still surfaces through them to the victim's readers, exactly as before
  this change. Giving them their own columns is a materialization change (they are not
  block-derived `SqlBatchView`s the way `log_entries`/`measures`/`log_stats` are) and is out of
  scope here.
- **The per-process JIT `view_instance` path.** `view_instance('log_entries'|'measures', pid)`
  (`log_view.rs`, `metrics_view.rs`, and similarly `images_view.rs`,
  `otel/spans_view.rs`) resolves one `ProcessMetadata` via `find_process` and stamps every block
  it fetches with that single `process.audience`
  (`jit_partitions.rs::fetch_process_blocks` sets `process: process.clone()` on each
  `PartitionSourceBlock`; `log_entries_table.rs`/`metrics_table.rs` emit `row.process.audience`).
  Only the global, blocks-view-backed instance is per-block. So a block an attacker writes onto a
  victim's `process_id` is labelled with the *victim's* audience — and is visible to the victim —
  through `view_instance`, while the same block correctly carries the attacker's own audience in
  the global view. Carrying the block's own stamp into the JIT path (splitting a per-block
  audience out of `ProcessMetadata`) is a real fix but a separate change; it is not attempted
  here.

Both are pre-existing gaps, not new ones — this plan does not widen either — and both are recorded
in [Out of scope](#out-of-scope--follow-ups).

**`metadata.rs::find_process`** — `coalesced_audience_subselect("properties", 2)` becomes
`coalesced_audience_column("processes", 2)`. Nothing else in `ProcessMetadata` changes.

**`audience_guard.rs::owner_query_sql`** — every arm reads a column, and the property-name bind
(`$2`) disappears, so the default audience moves from `$3` to `$2`:

```sql
-- IdKind::Process
SELECT process_id AS id, COALESCE(audience, $2) AS audience
FROM processes WHERE process_id = ANY($1::uuid[])

-- IdKind::Block          (one table, no join)
SELECT block_id AS id, COALESCE(audience, $2) AS audience
FROM blocks WHERE block_id = ANY($1::uuid[])

-- IdKind::ProcessOrStream
SELECT process_id AS id, COALESCE(audience, $2) AS audience
FROM processes WHERE process_id = ANY($1::uuid[])
UNION ALL
SELECT stream_id AS id, COALESCE(audience, $2) AS audience
FROM streams WHERE stream_id = ANY($1::uuid[])
```

Every `LEFT JOIN LATERAL` unnest and every join to `processes` is gone — three single-table point
queries on primary-key-indexed columns.

One behaviour change worth calling out: a **block or stream whose process row no longer exists**
(retention swept it, or it hasn't arrived yet) previously resolved to `OwnerAudience::Unknown` and
was denied by the inner join dropping it; it now resolves to its own stamp. That is the correct
answer under the precedence rule and it makes `get_payload`/`parse_block` work on
orphaned-but-present blocks. `merge_owner_rows`, `is_readable`, the `Ambiguous` handling, the
fail-closed treatment of `Unknown`, and the no-existence-oracle error shape are all unchanged.

### 5. Block/stream `process_id` mismatch (measure, don't reject yet)

Issue step 4 asks for a check that a block's `process_id` matches its stream's. With per-row
stamping this is **no longer security-critical** — the block's own stamp governs its label
regardless of what `process_id` it claims — so it is a plain data-integrity check, and the issue
asks to land it as warn + counter and measure the real mismatch rate before flipping it to a hard
reject.

Doing that at write time would cost a `SELECT process_id FROM streams WHERE stream_id = $1` on the
hot block path, plus a cache and a TTL to make it affordable, plus a fail-open arm for the
block-before-stream ordering — reintroducing exactly the machinery this design set out to avoid,
to measure something expected to be zero.

Measure it from the maintenance role instead, where it costs nothing on the ingest path. Add to
`EveryHourTask::run` (`rust/public/src/servers/maintenance.rs:129-156`), alongside the existing
`delete_old_data` call:

```sql
SELECT count(*) FROM blocks b
JOIN streams s ON s.stream_id = b.stream_id
WHERE b.process_id <> s.process_id
AND b.insert_time >= $1
```

bounded to the last hour, reported as an `imetric!("block_stream_process_id_mismatch", "count", n)`
and a `warn!` when non-zero. The healthy baseline is a flat zero; every non-zero reading is a bug
or an attack.

The hard reject is deferred to a follow-up, to be opened once there is a measurement to justify
paying for the write-path lookup.

### Data flow, after

```
credential (AuthContext.bound_audience)
        |
        v  resolve_write_audience   -- deployment default when the credential carries none
   WriteAudience
        |
        +--> processes.audience   (insert_process, register_otel_process)
        +--> streams.audience     (insert_stream, register_otel_stream)
        +--> blocks.audience      (insert_block_typed)

blocks_view  audience         = COALESCE(blocks.audience,    default)  -> log_entries, measures, log_stats
             stream_audience  = COALESCE(streams.audience,   default)  -> streams_view.audience
             process_audience = COALESCE(processes.audience, default)  -> processes_view.audience

audience_guard (Prong B)  block_id  -> blocks.audience
                          stream_id -> streams.audience
                          process_id-> processes.audience
```

## Implementation Steps

### Phase 1 — schema

1. `rust/ingestion/src/sql_migration.rs`: `LATEST_DATA_LAKE_SCHEMA_VERSION` 7 → 8, add
   `upgrade_data_lake_schema_v8` (§2) and its `if 7 == current_version` arm in
   `execute_migration`. Do **not** touch `sql_telemetry_db.rs`.

### Phase 2 — write path

2. `rust/ingestion/src/web_ingestion_service.rs`: add `audience: &WriteAudience` to
   `insert_stream`, `register_otel_stream`, `insert_block`, `insert_block_typed`; bind the column
   in each; give `insert_block_typed`'s `INSERT` an explicit column list. Drop the two known-gap
   doc comments (`:287-296`, `:417-421`) and replace them with a one-line note on what the stamp
   means.
3. Same file: bind `processes.audience` in `insert_process`/`register_otel_process`, give
   `insert_process`'s `INSERT INTO processes` an explicit column list (`register_otel_process`
   already has one), delete `finalize_process_properties`, add a direct
   `strip_reserved_properties(...)` call at each of `insert_process`'s and
   `register_otel_process`'s `properties` bind sites (the only call the removed helper made), and
   rewrite `check_process_audience_conflict` to `SELECT audience`.
4. `rust/telemetry/src/property_names.rs` + `property.rs`: remove `PROPERTY_AUDIENCE` and its
   re-export.
5. `rust/public/src/servers/ingestion.rs`: thread `ctx: Option<Extension<AuthContext>>` into
   `insert_stream_request`/`insert_block_request` and call `resolve_write_audience`.
6. `rust/otel-ingestion/src/handler.rs`: pass `audience` to `register_otel_stream` and
   `insert_block_typed` in `write_blocks`.
7. `rust/analytics/src/replication.rs`: read and bind `audience` in `ingest_processes`,
   `ingest_streams`, `ingest_blocks`; explicit column list on both the processes insert and the
   blocks insert.

### Phase 3 — read path

8. `rust/analytics/src/audience.rs`: replace `audience_subselect`/`coalesced_audience_subselect`
   with `coalesced_audience_column`, drop the `AUDIENCE_PROPERTY` re-export, and write the
   precedence rule (§1) into the module doc.
9. `rust/analytics/src/lakehouse/blocks_view.rs`: new `data_sql`, two appended schema fields,
   `blocks_file_schema_hash()` → `vec![6]`.
10. `rust/analytics/src/lakehouse/processes_view.rs` / `streams_view.rs`: transform queries switch
    to `max(process_audience)` / `max(stream_audience)`. `log_stats_view.rs`: add `audience` to the
    `GROUP BY` in both the transform and merge queries (§4, "The `max(audience)` regression").
    `ownership_rewrite.rs`: `audience_column_predicate` requires `audience`, `stream_audience`,
    and `process_audience` all be in the caller's read scope when filtering `blocks` specifically
    (same section) — otherwise the two appended columns turn `blocks` into a cross-audience
    existence-and-label oracle.
11. `rust/analytics/src/metadata.rs`: `find_process` uses `coalesced_audience_column`.
12. `rust/analytics/src/lakehouse/audience_guard.rs`: rewrite `owner_query_sql`'s three arms, drop
    the `AUDIENCE_PROPERTY` bind, renumber the default to `$2`, and update the module doc's
    "one cache, one question" and fail-closed paragraphs for the orphaned-row behaviour change.

### Phase 4 — integrity measurement

13. `rust/public/src/servers/maintenance.rs`: hourly mismatch count + `imetric!` + `warn!` (§5).

### Phase 5 — tests, docs, tooling

14. Tests (see [Testing Strategy](#testing-strategy)).
15. `local_test_env/ai_scripts/import_net_blocks_from_prod.py`: project the audience into all three
    tables, and add the missing `format` projection.
16. Docs and `CHANGELOG.md` (see [Documentation](#documentation)).
17. `tasks/data_isolation/audience_based_access_control_plan.md` §11b: replace the "residual,
    deferred to Stage 5b" text with what actually shipped.

## Files to Modify

- `rust/ingestion/src/sql_migration.rs`
- `rust/ingestion/src/web_ingestion_service.rs`
- `rust/telemetry/src/property_names.rs`, `rust/telemetry/src/property.rs`
- `rust/public/src/servers/ingestion.rs`, `rust/public/src/servers/maintenance.rs`
- `rust/otel-ingestion/src/handler.rs`
- `rust/analytics/src/replication.rs`, `rust/analytics/src/audience.rs`,
  `rust/analytics/src/metadata.rs`
- `rust/analytics/src/lakehouse/blocks_view.rs`, `processes_view.rs`, `streams_view.rs`,
  `log_stats_view.rs`, `audience_guard.rs`, `ownership_rewrite.rs` (`audience_column_predicate`
  gains a `blocks`-specific three-column check, §4's "`max(audience)` regression"; module doc also
  updated — the stale "what remains open" paragraph, narrowed per §4)
- Tests: `rust/ingestion/tests/audience_stamping_db_test.rs`,
  `rust/ingestion/tests/write_audience_tests.rs`, `rust/analytics/tests/common/db_fixtures.rs`,
  `rust/analytics/tests/audience_guard_tests.rs`, `rust/analytics/tests/prong_b_guard_db_test.rs`,
  `rust/analytics/tests/ownership_rewrite_db_test.rs`,
  `rust/ingestion/tests/insert_block_dedup_db_test.rs` (its raw positional `INSERT INTO blocks`),
  plus every `insert_stream`/`insert_block` call site in `rust/analytics/tests/`
- `local_test_env/ai_scripts/import_net_blocks_from_prod.py`
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/ingestion.md`,
  `mkdocs/docs/admin/api-keys.md`, `mkdocs/docs/admin/maintenance.md`,
  `mkdocs/docs/query-guide/schema-reference.md`, `mkdocs/docs/query-guide/python-api.md`,
  `python/micromegas/micromegas/flightsql/client.py`
- `CHANGELOG.md`, `tasks/data_isolation/audience_based_access_control_plan.md`

## Trade-offs

**Per-row column vs. write-side authorization gate.** The gate needs a cache, a point query, a TTL,
a fail-closed policy, and an ordering guarantee that does not exist. Neither of its possible
defaults is acceptable: fail-open leaves the hole, fail-closed drops ordinary first-blocks and
every post-sweep block. Stamping binds a string already in hand with no hot-path read at all, and
closes the hole on the *read* side rather than the write side, which is strictly stronger — the
victim never sees the attacker's row regardless of what it claims.

**Column vs. property, on `processes`.** Keeping the process stamp in `properties` while streams
and blocks used a column would leave two shapes, two extraction expressions, and a standing
question about which one wins. Given no deployed data carries the property, the migration cost of
unifying is zero and the design cost of not unifying is permanent. The trade is that
`processes.properties` no longer contains `micromegas.audience` — the audience remains queryable as
the top-level `audience` column every relevant view already exposes, which is where a dashboard
should have been reading it anyway.

**Two extra columns on `blocks_view` vs. leaving `max(audience)` alone.** Leaving it alone is not
an option — it converts an integrity gap into a confidentiality one (see
[The `max(audience)` regression](#the-maxaudience-regression)). The alternative to two columns is
re-sourcing `processes_view`/`streams_view` directly from Postgres instead of from the `blocks`
view, which means giving them their own partition specs and abandoning the batching they get for
free today: much larger, for the same result. Two dictionary-encoded columns with one distinct
value per partition are close to free on disk.

**Measuring the `process_id` mismatch from maintenance vs. at write time.** A write-time check
would reintroduce the cache-and-TTL machinery this design removes, in order to count something
expected to be zero, on the hottest path in the system. An hourly bounded query over an indexed
join gives the same number for nothing. The cost is latency to detection — up to an hour — which is
acceptable for a signal that no longer gates security.

**No backfill.** A NULL column resolving to the deployment default *is* today's semantics for an
unstamped row, so legacy rows and admin `bulk_ingest` rows read exactly as they do now. Backfilling
would mean a full table rewrite of `blocks` for no behavioural difference.

## Migration & Upgrade Notes

- The v8 migration runs in one transaction. `ADD COLUMN` is catalog-only, and the `CHECK`
  constraints are added `NOT VALID` (§2) specifically so the migration never scans existing rows —
  a validated `CHECK` on `ADD COLUMN` would otherwise force a full-table scan of `blocks` under
  `ACCESS EXCLUSIVE`. There is no table rewrite and no lock held over data.
- **Deploy order matters within a rolling upgrade, and it is not just about writes.**
  `migrate_db` only runs from `WebIngestionService::from_env`, `connect_to_remote_data_lake`
  (admin replication), and the monolith — `LakehouseContext::from_env` (flight-sql, maintenance)
  calls `connect_to_data_lake`, which never migrates. So the ingestion role (or the monolith) has
  to be upgraded and restarted first to apply v8; a v8 analytics or maintenance binary reading
  against a pre-v8 database doesn't just risk a bad insert, it fails every read that now
  references the new columns (`blocks_view`'s `data_sql`, `find_process`, `owner_query_sql`) with
  an "undefined column" error, before any writer has migrated it. A pre-v8 ingestion binary
  writing against an already-migrated v8 database is fine (it just leaves the column NULL, which
  reads as the default).
- `blocks_file_schema_hash()` bumping forces `blocks` partitions to rebuild. Per `CLAUDE.md` this
  is not a SQL break: the queryable Arrow schema gains two appended columns and every existing
  column keeps its name, type, and position.
- `processes`/`streams`/`log_entries`/`measures`/`log_stats` are all `SqlBatchView`s that hash
  their inferred Arrow schema (`SqlBatchView::get_file_schema_hash`), which does not change for any
  of them — `log_view.rs`/`metrics_view.rs` return a constant `vec![SCHEMA_VERSION]`, and
  `processes_view.rs`/`streams_view.rs`/`log_stats_view.rs` are equally schema-stable — so **none**
  of the five is auto-invalidated, the same as `processes`/`streams`. Their values only differ from
  the old ones for a row that was actually attacked, so no regeneration is required; an operator
  who wants strict consistency can `regenerate_partitions` over all six audience-carrying views
  (`blocks`, `processes`, `streams`, `log_entries`, `measures`, `log_stats`) for the retention
  window. `log_stats` is the one case where regeneration is more than a value fix: adding
  `audience` to its `GROUP BY` (§4) changes the grouping itself, so partitions materialized before
  this change keep rows grouped *without* audience — mixing what should now be separate
  per-audience rows — until they are regenerated; it's a shape disagreement with fresh partitions,
  not just a value one.
- No client change. No wire-format change. Native and OTLP producers are unaffected.

## Documentation

- `mkdocs/docs/query-guide/schema-reference.md` — the user-facing SQL-surface reference for the
  `audience` column. Retitle the per-view description (`:47,78,138,174,217,290`) from "The
  audience of the owning process" to reflect the per-row stamp (each view's own `audience` for
  `processes`/`streams`/`blocks`, the block's stamp for `log_entries`/`measures`/`log_stats`, and
  the process/stream stamp specifically for the process/stream-anchored views still listed under
  "What remains process-anchored"), and document the two new `blocks`-only columns
  (`stream_audience`, `process_audience`). Update the `:623-635` paragraph on where the default is
  applied to describe the column, not the property.
- `mkdocs/docs/query-guide/python-api.md` — the `bulk_ingest` example (`:488-507`) is a
  copy-pasteable `processes` `pyarrow.Table` with all thirteen current columns and no `audience`;
  after §3 makes a missing `audience` column a hard error, running that example against a v8
  target fails. Add `audience` to the example, and add the equivalent guidance for `streams` and
  `blocks` bulk-ingest tables nearby.
- `python/micromegas/micromegas/flightsql/client.py:630-654` — `bulk_ingest`'s docstring carries a
  second, independent copy of the same 13-column `processes` `pa.table({...})` example with no
  `audience` column. It fails against a v8 target for the same reason as the mkdocs example above
  and needs the same fix, applied separately since it's a different file with its own copy of the
  example.
- `mkdocs/docs/admin/authentication.md`
  - "Audience stamping and the default" (`:230-260`): the stamp is a column on `processes`,
    `streams`, and `blocks`, written at every insert, not a `micromegas.audience` property on the
    process.
  - **Narrow the "Residual gap: cross-audience write injection" warning admonition**
    (`:303-341`) — this plan closes the gap only for `blocks`/`streams`/`processes` themselves and
    the views derived straight from them (`log_entries`, `measures`, `log_stats`,
    `processes_view`, `streams_view`); it does not close it for `net_spans`, `otel_spans`,
    `images`, `async_events`, `thread_spans`, or the per-process `view_instance` path (§4, "What
    remains process-anchored"), which still resolve audience through the owning process/stream
    row. Rewrite the admonition to describe that narrower, remaining surface rather than deleting
    it. Keep the process-squatting paragraphs inside it (they describe a different, already-closed
    gap) by lifting them into the surrounding prose.
  - `:205` "the two prongs read different copies of `micromegas.audience`": still true (Prong A
    reads a materialized snapshot, Prong B reads Postgres live), but retitle to the column.
- `mkdocs/docs/admin/ingestion.md` "What gets stamped" (`:70-105`): the stamp now lands on all
  three metadata rows; the "a client that used to self-stamp" note stays accurate, since
  `strip_reserved_properties` still runs. Add a **deploy-order note** next to it, matching the
  style of `api-keys.md`'s "Migrating from the env keyring" ordering guidance and `monolith.md`'s
  schema-version table entry: in a split deployment, upgrade and restart ingestion (or the
  monolith) *before* flight-sql/maintenance — `migrate_db` only runs from
  `WebIngestionService::from_env`, `connect_to_remote_data_lake`, and the monolith, never from
  `LakehouseContext::from_env` (flight-sql, maintenance), so a v8 analytics or maintenance binary
  reading against a pre-v8 database fails every read that touches the new columns
  (`blocks_view`'s `data_sql`, `find_process`, `owner_query_sql`) with an "undefined column" error
  until ingestion has migrated it. A pre-v8 ingestion binary against an already-migrated v8
  database is fine (writes just leave the column NULL, which reads as the default).
- `mkdocs/docs/admin/api-keys.md:238`: the parenthetical naming the property.
- `CHANGELOG.md`, under `## Unreleased` → **Ingestion**: one entry describing the column, the
  closed gap, and the schema v8 bump. Because #1373/#1482/#1519 are all still `## Unreleased`,
  amend their entries in place (as #1482 and #1486 already do to earlier ones) rather than
  describing a break against a released API. Flag as **Minor breaking change**: `insert_stream`,
  `insert_block`, `insert_block_typed`, and `register_otel_stream` each gain a required
  `&WriteAudience`; `finalize_process_properties` and `PROPERTY_AUDIENCE` are removed;
  `audience_subselect`/`coalesced_audience_subselect` are replaced by `coalesced_audience_column`.
  **Upgrade note**: deploy and restart ingestion (or the monolith) before flight-sql/maintenance —
  only `migrate_db`'s callers (`WebIngestionService::from_env`, `connect_to_remote_data_lake`, the
  monolith) apply schema v8; `LakehouseContext::from_env` (flight-sql, maintenance) never migrates,
  so a v8 binary reading a pre-v8 database fails every query that touches the new columns
  (`blocks_view`, `find_process`, `owner_query_sql`) with an "undefined column" error. A pre-v8
  ingestion binary against an already-migrated database is fine.
- `mkdocs/docs/admin/maintenance.md` — the hourly task's table row (`:87`) gains the new
  `process_id`/`stream_id` mismatch count alongside retention cleanup, and the metric gets its own
  reference section matching `materialize_view_failure`'s (`:62-68`): name
  `block_stream_process_id_mismatch` (`count`), no tags, healthy baseline a flat zero — every
  non-zero reading is a bug or an attack (§5).
- `rust/analytics/src/lakehouse/ownership_rewrite.rs`'s module doc: the "What remains open,
  tracked separately" paragraph is only partly obsolete — narrow it to the five
  `per_process_audience()`-resolved views and the JIT `view_instance` path (§4, "What remains
  process-anchored"); its operational-mitigation advice (audience-bound DB-backed credentials
  only) still applies to that narrower surface and should stay.

## Testing Strategy

**Unit / offline** (plain `cargo test`)

- `write_audience_tests.rs`: drop the `finalize_process_properties` assertions, keep the
  `strip_reserved_properties` and `WriteAudience` charset ones.
- `audience.rs` unit tests: `coalesced_audience_column` emits `COALESCE(x.audience, $n)`, keeps the
  default a bind parameter, and honours a caller-chosen placeholder index.
- `audience_guard_tests.rs`: `is_readable`/`merge_owner_rows` are unchanged, but assert
  `owner_query_sql` no longer mentions `unnest` or `properties` for any `IdKind`.

**DB-backed** (`#[ignore]`, live Postgres + object store — the existing harness pattern)

- `audience_stamping_db_test.rs`: rewrite `read_audience_property` → `read_audience_column`, and
  `strip_audience_property` → `UPDATE ... SET audience = NULL` for fabricating a legacy row. The
  conflict-guard cases (same audience, different audience → 403, legacy NULL row vs. default) all
  carry over unchanged in intent.
- **New**: stamp round-trip for streams and blocks — `insert_stream`/`insert_block_typed` under
  audience `alpha` land rows whose `audience` column reads back `alpha`.
- **New, the actual regression this closes**: a block written under audience `alpha` carrying a
  `process_id` owned by `beta` materializes into `blocks_view` with `audience = 'alpha'`, and
  `beta`'s `processes_view`/`streams_view` rows keep `audience = 'beta'` — the `max(audience)`
  regression guard.
- **New, `ownership_rewrite_db_test.rs`**: an `alpha`-scoped query over `blocks` does not return
  the row above — confirms `audience_column_predicate`'s three-column check on `blocks` keeps
  `process_audience='beta'` from surfacing to `alpha`, i.e. `blocks` is not an existence-and-label
  oracle for `beta`.
- **New**: Prong B resolves `IdKind::Block` for a block whose `processes` row has been deleted
  (orphan), returning the block's own stamp rather than `Unknown`.
- `prong_b_guard_db_test.rs` / `ownership_rewrite_db_test.rs` / `jit_process_batch_db_test.rs` /
  `thread_spans_ordering_db_test.rs`: mechanical updates for the new `insert_stream`/`insert_block`
  signatures.
- `insert_block_dedup_db_test.rs:194`: its raw positional `INSERT INTO blocks` needs the explicit
  column list; the four (object, row) dedup outcomes are otherwise unaffected.
- `common/db_fixtures.rs:123`: legacy-row fabrication switches from stripping the property to
  nulling the column.

**End-to-end**

- Start services (`local_test_env/ai_scripts/start_services.py`), ingest under two DB-backed
  ingestion keys bound to different audiences, and confirm via `micromegas-query` that each
  audience's reader sees only its own blocks — including after a hand-crafted cross-audience block
  insert.
- Confirm the hourly maintenance mismatch counter reads zero on a clean run.

## Out of Scope / Follow-ups

- **The five process/stream-anchored views** (`net_spans`, `otel_spans`, `images`,
  `async_events`, `thread_spans`) and **the per-process JIT `view_instance` path**
  (`log_view.rs`/`metrics_view.rs`/`images_view.rs`/`otel/spans_view.rs`) still resolve audience
  through the owning process/stream row rather than a per-row column — see
  ["What remains process-anchored"](#what-remains-process-anchored). Pre-existing gaps, not
  widened by this plan; giving each its own per-row stamp is a follow-up.
- **Relaxing `blocks_view`'s inner join to `processes`.** Now that blocks are self-describing, that
  join is the only thing still hiding early-arriving and post-sweep blocks from every view.
  Relaxing it becomes possible; it is a separate change with its own materialization consequences.
- **Hard-rejecting a block whose `process_id` disagrees with its stream's** (§5) — open once the
  hourly counter has data.
- **Dropping the process-squatting conflict guard's cache.** `check_process_audience_conflict` now
  reads one indexed column instead of an array; whether its `moka` cache still earns its keep is
  worth re-measuring, but not here.
- **Tightening `audience` to `NOT NULL` on `processes`/`streams`/`blocks`.** Nullability today
  exists only for genuinely pre-AbAC legacy rows — §3 has replication read and bind `audience` from
  the incoming batch and hard-fail when it's missing, so the admin replication path already always
  stamps a real label and is not itself a reason to stay nullable. Once every deployment has cycled
  past its retention window and no legacy-NULL row remains, the column could become `NOT NULL`,
  dropping the `COALESCE` from the three read sites that use it (`blocks_view`, `find_process`,
  `owner_query_sql`). A retention-window question, not a design one; worth its own follow-up issue
  once this ships.

## Open Questions

1. **Replication and a pre-column source.** The plan hard-fails `bulk_ingest` when the incoming
   batch has no `audience` column, matching the `format` precedent from schema v4. The alternative
   is to accept the column as optional and bind NULL (which reads as the target's default). Hard
   failure is louder and the feature is unreleased, so nothing in the wild breaks — but confirm
   that no internal replication tooling pins an older source lake.
