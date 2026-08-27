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
Each row then carries the audience it was actually written under, and no row's label is ever
derived from another row. An attacker's block carries A no matter which `process_id` it claims, so
it surfaces only to A's readers; the victim never sees it, and the victim's process never has to
be resolvable at write time — which is the reason the originally-proposed write-side gate could not
work (streams and blocks routinely arrive before their process row exists; see
[Why not a write-side gate](#why-not-a-write-side-gate)).

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
still requires a read grant on B. Process squatting (`check_process_audience_conflict`) and
cross-audience OTLP process collision (audience-salted id derivation) are already closed and are
untouched.

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
> another row's — not through `process_id`, not through `stream_id`.

`check_process_audience_conflict`'s one-audience-per-process rule is unaffected: it still governs
the `processes` row and only the `processes` row.

### 2. Schema v8

`LATEST_DATA_LAKE_SCHEMA_VERSION` 7 → 8, new `upgrade_data_lake_schema_v8` in
`rust/ingestion/src/sql_migration.rs` following the v6 pattern:

```sql
ALTER TABLE processes ADD COLUMN audience VARCHAR(255)
  CONSTRAINT processes_audience_name CHECK (audience ~ '^[A-Za-z0-9_-]+$');
ALTER TABLE streams   ADD COLUMN audience VARCHAR(255)
  CONSTRAINT streams_audience_name   CHECK (audience ~ '^[A-Za-z0-9_-]+$');
ALTER TABLE blocks    ADD COLUMN audience VARCHAR(255)
  CONSTRAINT blocks_audience_name    CHECK (audience ~ '^[A-Za-z0-9_-]+$');
UPDATE migration SET version=8;
```

Deliberate properties:

- **Nullable, no `DEFAULT`, no backfill.** `ADD COLUMN` with no default is a catalog-only
  operation in Postgres 11+, so this is instant even on a large `blocks` table and the whole
  migration stays inside one transaction like every prior version. A `DEFAULT` would also let a
  not-yet-upgraded writer keep inserting rows that silently take a label, the same reason v6
  refused one.
- **`CHECK` permits NULL** (a `NULL ~ '...'` predicate evaluates to NULL, which passes), so it
  constrains stamped rows without demanding a backfill. It mirrors
  `ingestion_api_keys_audience_name` and re-states in SQL what `WriteAudience::new` already
  validates in Rust.
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

- `finalize_process_properties` is **removed**. `strip_reserved_properties` stays and keeps doing
  its job — a client-supplied `micromegas.*` property is still dropped, so a client can neither
  assert nor suppress a stamp; there is simply no property to append any more.
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
replicating into a v4 target surfaces the schema mismatch loudly"). `ingest_blocks`' positional
`VALUES($1..$11)` gets an explicit column list for the same reason as above.

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
- `log_entries`, `measures`, and `log_stats` keep using `audience` — their rows come from a block's
  payload, so the block's own stamp is the correct anchor. No change.

`OwnershipRewrite` needs no change at all: it dispatches on whether a view's file schema has an
`audience` field, all six column-carrying views still have one, and the two new columns on
`blocks_view` are ordinary data columns it ignores.

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
3. Same file: bind `processes.audience` in `insert_process`/`register_otel_process`, delete
   `finalize_process_properties`, and rewrite `check_process_audience_conflict` to
   `SELECT audience`.
4. `rust/telemetry/src/property_names.rs` + `property.rs`: remove `PROPERTY_AUDIENCE` and its
   re-export.
5. `rust/public/src/servers/ingestion.rs`: thread `ctx: Option<Extension<AuthContext>>` into
   `insert_stream_request`/`insert_block_request` and call `resolve_write_audience`.
6. `rust/otel-ingestion/src/handler.rs`: pass `audience` to `register_otel_stream` and
   `insert_block_typed` in `write_blocks`.
7. `rust/analytics/src/replication.rs`: read and bind `audience` in `ingest_processes`,
   `ingest_streams`, `ingest_blocks`; explicit column list on the blocks insert.

### Phase 3 — read path

8. `rust/analytics/src/audience.rs`: replace `audience_subselect`/`coalesced_audience_subselect`
   with `coalesced_audience_column`, drop the `AUDIENCE_PROPERTY` re-export, and write the
   precedence rule (§1) into the module doc.
9. `rust/analytics/src/lakehouse/blocks_view.rs`: new `data_sql`, two appended schema fields,
   `blocks_file_schema_hash()` → `vec![6]`.
10. `rust/analytics/src/lakehouse/processes_view.rs` / `streams_view.rs`: transform queries switch
    to `max(process_audience)` / `max(stream_audience)`.
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
  `audience_guard.rs`, `ownership_rewrite.rs` (module doc only — the stale "what remains open"
  paragraph)
- Tests: `rust/ingestion/tests/audience_stamping_db_test.rs`,
  `rust/ingestion/tests/write_audience_tests.rs`, `rust/analytics/tests/common/db_fixtures.rs`,
  `rust/analytics/tests/audience_guard_tests.rs`, `rust/analytics/tests/prong_b_guard_db_test.rs`,
  `rust/analytics/tests/ownership_rewrite_db_test.rs`,
  `rust/ingestion/tests/insert_block_dedup_db_test.rs` (its raw positional `INSERT INTO blocks`),
  plus every `insert_stream`/`insert_block` call site in `rust/analytics/tests/`
- `local_test_env/ai_scripts/import_net_blocks_from_prod.py`
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/ingestion.md`,
  `mkdocs/docs/admin/api-keys.md`
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

- The v8 migration is catalog-only and runs in one transaction; there is no table rewrite and no
  lock held over data.
- **Deploy order matters within a rolling upgrade.** A pre-v8 ingestion binary writing against a
  v8 database is fine (it just leaves the column NULL, which reads as the default). A v8 binary
  against a pre-v8 database fails its inserts — so migrate first, which `migrate_db` on startup
  already does.
- `blocks_file_schema_hash()` bumping forces `blocks` partitions to rebuild. Per `CLAUDE.md` this
  is not a SQL break: the queryable Arrow schema gains two appended columns and every existing
  column keeps its name, type, and position.
- `processes`/`streams` are `SqlBatchView`s that hash their inferred Arrow schema, which does not
  change, so they are **not** auto-invalidated. Their values only differ from the old ones for a
  row that was actually attacked, so no regeneration is required; an operator who wants strict
  consistency can `regenerate_partitions` over `blocks`, `processes`, and `streams` for the
  retention window.
- No client change. No wire-format change. Native and OTLP producers are unaffected.

## Documentation

- `mkdocs/docs/admin/authentication.md`
  - "Audience stamping and the default" (`:230-260`): the stamp is a column on `processes`,
    `streams`, and `blocks`, written at every insert, not a `micromegas.audience` property on the
    process.
  - **Delete the "Residual gap: cross-audience write injection" warning admonition**
    (`:303-341`) — this plan is what closes it. Keep the process-squatting paragraphs inside it
    (they describe a different, already-closed gap) by lifting them into the surrounding prose.
  - `:205` "the two prongs read different copies of `micromegas.audience`": still true (Prong A
    reads a materialized snapshot, Prong B reads Postgres live), but retitle to the column.
- `mkdocs/docs/admin/ingestion.md` "What gets stamped" (`:70-105`): the stamp now lands on all
  three metadata rows; the "a client that used to self-stamp" note stays accurate, since
  `strip_reserved_properties` still runs.
- `mkdocs/docs/admin/api-keys.md:238`: the parenthetical naming the property.
- `CHANGELOG.md`, under `## Unreleased` → **Ingestion**: one entry describing the column, the
  closed gap, and the schema v8 bump. Because #1373/#1482/#1519 are all still `## Unreleased`,
  amend their entries in place (as #1482 and #1486 already do to earlier ones) rather than
  describing a break against a released API. Flag as **Minor breaking change**: `insert_stream`,
  `insert_block`, `insert_block_typed`, and `register_otel_stream` each gain a required
  `&WriteAudience`; `finalize_process_properties` and `PROPERTY_AUDIENCE` are removed;
  `audience_subselect`/`coalesced_audience_subselect` are replaced by `coalesced_audience_column`.
- `rust/analytics/src/lakehouse/ownership_rewrite.rs`'s module doc: the "What remains open,
  tracked separately" paragraph and its operational-mitigation advice are obsolete.

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

- **Relaxing `blocks_view`'s inner join to `processes`.** Now that blocks are self-describing, that
  join is the only thing still hiding early-arriving and post-sweep blocks from every view.
  Relaxing it becomes possible; it is a separate change with its own materialization consequences.
- **Hard-rejecting a block whose `process_id` disagrees with its stream's** (§5) — open once the
  hourly counter has data.
- **Dropping the process-squatting conflict guard's cache.** `check_process_audience_conflict` now
  reads one indexed column instead of an array; whether its `moka` cache still earns its keep is
  worth re-measuring, but not here.

## Open Questions

1. **Replication and a pre-column source.** The plan hard-fails `bulk_ingest` when the incoming
   batch has no `audience` column, matching the `format` precedent from schema v4. The alternative
   is to accept the column as optional and bind NULL (which reads as the target's default). Hard
   failure is louder and the feature is unreleased, so nothing in the wild breaks — but confirm
   that no internal replication tooling pins an older source lake.
2. **Whether `processes.audience` should become `NOT NULL` in a later migration.** It cannot now
   (legacy rows), but once every deployment has cycled past its retention window the column could
   be tightened, which would let the `COALESCE` disappear from three read sites. Worth an issue, or
   worth leaving permanently nullable to keep the admin replication path free to stamp nothing?
