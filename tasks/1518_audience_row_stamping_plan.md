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

Three decisions taken with the issue author:

- **Uniform treatment.** The column goes on all three tables, not just the two that lack a stamp.
  `processes` moves off the property too, so there is exactly one shape and one precedence rule
  rather than a column for two tables and a property for the third.
- **No property migration.** The whole audience-stamping stack (#1373, #1482, #1519) is still
  `## Unreleased`, and no deployed environment holds property-stamped rows. So the property is not
  backfilled into the column, is not read as a fallback, and stops being written. The only rows
  that carry a NULL column are genuinely pre-AbAC rows — admin `bulk_ingest` from a source that
  predates the column is rejected outright rather than written with NULL (§3) — and those resolve
  to `MICROMEGAS_DEFAULT_AUDIENCE` at read time exactly as they do today.
- **`blocks` and `streams` carry their own audience, not the process's.** This is settled, not a
  trade-off to be revisited: a block's label is the credential that wrote *that block*, and a
  stream's is the credential that wrote *that stream*. The point of the change is precisely that
  an attacker's block cannot borrow its victim's label by naming the victim's `process_id`. Where
  a downstream view aggregates or joins across rows, the fix is to re-anchor the view on the
  row's own stamp (§4) — never to relabel the row from the process it points at.

Scope is integrity, not confidentiality: for any row whose own anchor already carries a real,
non-NULL audience, no read escalation is created or removed here — reading B still requires a read
grant on B. That claim depends on a mismatched row never materializing into any view in the first
place: a block whose `audience` disagrees with the `streams.audience` or `processes.audience` it
joins to is excluded from materialization by a single predicate on `blocks_view`'s `data_sql`
(mirrored in its `source_count_query`) — it never becomes a row of `blocks` at all, so it is equally
absent from `log_entries`, `measures`, and every other view that reads blocks through the `blocks`
view's own materialized partitions — see
["The `max(audience)` regression"](#the-maxaudience-regression) in §4 for the residual cases the
predicate does not close: a NULL-anchored `processes`/`streams` row relabeled by an attacker's block,
and its mirror image, a NULL-audience block resolving through a real `processes`/`streams` anchor —
which *is* a bounded read escalation, not covered by "already public" — and, for
`log_entries`/`measures` specifically, why letting a mismatched row materialize would leak the
victim process's own `exe`/`username`/`computer`/`process_properties`.
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
consequence of this change and the reason a NULL-anchored `processes`/`streams` row is a residual,
accepted exposure rather than a closed one.

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
> predates this stage and resolves to the deployment's `MICROMEGAS_DEFAULT_AUDIENCE`. No row's
> audience is ever derived from
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

- `insert_stream` — add `audience` to the existing explicit column list, and on
  `rows_affected() == 0` call the new `check_stream_audience_conflict` (§5) before returning.
- `register_otel_stream` — same, plus the same conflict check.
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
replicating into a v4 target surfaces the schema mismatch loudly"). This is safe to do outright:
the issue author has confirmed that no replication tooling outside this repository pins a source
lake older than the column, and the only in-repo producers — `import_net_blocks_from_prod.py` and
the two `bulk_ingest` examples — are updated by this plan. `ingest_processes`'s and `ingest_blocks`'
positional `VALUES($1..$13)` / `VALUES($1..$11)` inserts (`replication.rs:122` and `:213`) both get
an explicit column list for the same reason as above: a missed bind against a just-widened table
silently defaults to NULL instead of failing.

The `bulk_ingest` example in `mkdocs/docs/query-guide/python-api.md` needs the same fix for the
same reason: it hand-builds a `processes` table with no `audience` column, which the hard-error
above now rejects (see [Documentation](#documentation)).

`local_test_env/ai_scripts/import_net_blocks_from_prod.py` projects explicit column lists into
`bulk_ingest`, but all three (`_build_processes_table`, `_build_streams_table`,
`_build_blocks_table`) derive from a single `SELECT * FROM blocks` result (`_select_net_blocks`),
and `blocks_view` exposes exactly one `audience` column — the block's own stamp — with no
`processes.audience`/`streams.audience` alongside it (§4: those anchor columns were dropped, not
projected into the Arrow output). So each of the three `_project(...)` calls must add
`("audience", "audience")`, sourcing all three tables' `audience` from that one block-level column;
§4's mismatch predicate guarantees it agrees with `processes.audience`/`streams.audience` for any
block that isn't NULL-anchored legacy data, which is the only case this script's source query can
produce. Note `_build_streams_table` is already missing `format`, which `ingest_streams` requires —
fix that in the same pass rather than leaving a second latent break.

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

**`blocks_view.rs`** — the audience now comes off the block's own row, and a predicate excludes a
block whose own stamp disagrees with either row it joins to. `blocks` keeps exactly one audience
column — the block's own stamp — no new column is appended for either row it joins to; the
predicate references `streams.audience`/`processes.audience` directly in the plain Postgres SQL of
`data_sql`, so it needs neither one projected into the Arrow output. See
["The `max(audience)` regression"](#the-maxaudience-regression) below for why that predicate is
needed and why it is the single place this plan puts it:

```sql
SELECT block_id, streams.stream_id, processes.process_id, ... ,
       COALESCE(blocks.audience, $3)    AS audience
FROM blocks, streams, processes
WHERE blocks.stream_id = streams.stream_id
AND blocks.process_id = processes.process_id
AND blocks.insert_time >= $1 AND blocks.insert_time < $2
AND (blocks.audience IS NULL OR streams.audience IS NULL OR blocks.audience = streams.audience)
AND (blocks.audience IS NULL OR processes.audience IS NULL OR blocks.audience = processes.audience)
ORDER BY blocks.insert_time, blocks.block_id;
```

The mismatch predicate is NULL-tolerant rather than a `COALESCE`-then-compare: it excludes a row
only when **both** sides carry a real, non-NULL stamp and those stamps disagree. A `COALESCE`d
comparison would get this wrong in the other direction — `COALESCE(NULL, 'public') = 'public'`
against `COALESCE('alpha', 'public') = 'alpha'` disagree, so a legacy row with a `NULL` audience on
one side and any non-default real stamp on the other would be excluded even though nothing was
ever attacked. That failure mode is not theoretical: `processes`/`streams` rows are immutable
(`ON CONFLICT (process_id|stream_id) DO NOTHING`, `web_ingestion_service.rs:437-440,490-493,546`)
and there is no backfill (§2), so every process/stream registered before the v8 ingestion binary
rolls out keeps `audience = NULL` for its entire remaining life while its post-upgrade blocks carry
the credential's real (non-default) audience — a `COALESCE` comparison would silently drop all of
that process's/stream's post-upgrade telemetry from `blocks`, `log_entries`, `measures`, and
`log_stats` forever, not just during the rollout window. The same mixed-version exposure applies
mid-rolling-upgrade with a pre-v8 and v8 ingestion binary running side by side (a pre-v8 process
writes NULL-audience blocks onto a stream a v8 binary already stamped). The NULL-tolerant form
avoids this: an attacker-injected block always carries a real, non-NULL stamp (§3 — every insert
binds a resolved `WriteAudience`, defaulted but never NULL), so nothing an attacker controls is
ever `IS NULL`, and the only way to reach the "either side NULL" pass-through is a genuinely
pre-v8 row. That residual pass-through is accepted, not an open question: a NULL anchor means the
row predates per-row stamping and resolves to `MICROMEGAS_DEFAULT_AUDIENCE` — the deployment's
public/legacy audience — so the metadata an attacker reaches through it was already public. See
Migration & Upgrade Notes for the resulting window and the rationale in full.
`make_batch_partition_spec`'s `source_count_query` (below) carries the identical WHERE-clause
predicate, so the row count it uses to size partition work agrees with what `data_sql` actually
returns. Because the predicate is the NULL-tolerant form above rather than a `COALESCE` comparison,
it references only `$1`/`$2` (`blocks.insert_time`) — no `$3` appears anywhere in the mirrored
WHERE clause, only in `data_sql`'s own `SELECT` list, which `source_count_query` (a bare
`COUNT(*)`) doesn't have. So `fetch_metadata_partition_spec` (`metadata_partition_spec.rs:60-67`)
needs no new bind for the predicate to work, and `MetadataPartitionSpec::default_audience`'s
existing doc comment — "the separate `source_count_query` deliberately does **not** get this bind
-- it has no `$3`" — stays accurate as written.

The join itself is unchanged — relaxing the inner join to `processes` is explicitly out of scope
(see [Out of scope](#out-of-scope--follow-ups)). What changes is that the join no longer *sources*
any row's label, and a mismatched row no longer survives the join at all.

`blocks_view_schema()` appends nothing — the Arrow schema is unchanged from today. `audience` keeps
its name, type, and position, and its meaning is *the audience this block was written under* —
which for every legitimate block is the same value the old expression produced.

`blocks_file_schema_hash()` **stays at `vec![5]` — no bump, and no forced rebuild.** `blocks` is the
largest view in the lake, and its Arrow schema does not change: `audience` keeps its name, type, and
position, so today's partitions remain byte-identical and perfectly valid to read under the
unchanged schema. Nothing about that schema *requires* a bump, and the author has decided not to pay
for a full rebuild just to force one. This is possible only because the two anchor columns an
earlier draft would have appended to `blocks_view` were dropped (see "One audience column on
`blocks` vs. two extra anchor columns" in Trade-offs) — with those gone, the schema this plan
produces is exactly the schema already on disk.

Consequence: existing `blocks` partitions keep whatever `audience` values they were materialized
with under the old query — sourced from the owning process's `micromegas.audience` property rather
than the block's own stamp — and predate the mismatch predicate (§4), so a partition may contain a
row the predicate would now exclude. Old- and new-semantics partitions are queried together going
forward, with nothing in the row data itself distinguishing which query produced it. In practice this
is a consistency note rather than an open confidentiality question: every partition in the lake today
holds public data, so a pre-change `audience` label describes public data either way, and the per-row
guarantee this plan establishes simply takes effect for everything ingested from here on — old
partitions carry no confidential exposure to age out of, they just age out of the retention window on
the usual schedule. An operator who wants uniform, per-row semantics on the existing partitions sooner
than that can run `regenerate_partitions` over `blocks`. See Migration & Upgrade Notes for the full
accounting.

#### The `max(audience)` regression

This is the part the issue body does not cover, and the reason it needs its own section even though
it turns out to resolve mostly on its own once `blocks` keeps exactly one `audience` column.

**The normal case.** The mismatch predicate excludes every block whose own stamp disagrees with a
non-NULL `streams.audience`/`processes.audience` it joins to. So for any block that survives the
predicate against **non-NULL** anchors, `audience == streams.audience == processes.audience` by
construction — `processes_view`/`streams_view`'s existing `max(audience)` over a process's/stream's
blocks is therefore already correct and needs no change: every block contributing to the `max()`
agrees with every other, so the aggregate can only ever equal that one shared value. This is the
normal case: a process/stream whose own row already carries a real, non-NULL stamp.

**The NULL-anchor case — kept honest, not closed.** The predicate is NULL-tolerant (§4 above): a
block matched against a legacy `processes`/`streams` row whose `audience` is still NULL — every row
registered before its ingestion binary reached v8, since those rows are immutable and there is no
backfill (§2) — survives with its own real stamp regardless of what that stamp is. With
`processes_view`/`streams_view` computing plain `max(audience)` over a process's/stream's blocks,
such a block **can** relabel that legacy row: an attacker in audience `zeta` writes one block
claiming victim `beta`'s NULL-audience `process_id`, and the aggregate resolves to `zeta` (or, if
`beta` also has legitimate post-upgrade blocks on the same process, `max('beta', 'zeta') = 'zeta'`)
— relabeling the victim's process row, hiding it from `beta`'s readers and exposing its metadata to
`zeta`'s. This is exactly the relabeling a per-row-anchored `max()` would have prevented.

This is now an accepted consequence, on the same grounds as every other NULL-anchor exposure in
this plan: a NULL-anchored `processes`/`streams` row is pre-AbAC data that resolves to
`MICROMEGAS_DEFAULT_AUDIENCE` at read time, so the row `max(audience)` can relabel here was already
public within the deployment before this label was ever attached to it. It is not closed by this
design — see the NULL-tolerant-window discussion in Migration & Upgrade Notes for the window's
lifetime and the rationale in full.

**The reverse direction — a real anchor with a NULL block.** The predicate's NULL-tolerance is
symmetric: it also passes a block whose own `audience` is NULL against a `processes`/`streams`
anchor that already carries a real, non-NULL stamp. This is reachable mid-rolling-upgrade of the
ingestion tier, not just before it starts: a v8 replica registers process `P` with
`processes.audience = 'beta'`, while a pre-v8 replica — still issuing the old positional
`VALUES($1..$11)` `INSERT INTO blocks` — writes one of `P`'s blocks with a short `VALUES` list,
leaving `blocks.audience` NULL. That block survives the predicate on its own NULL side, and resolves
to `COALESCE(NULL, default) = MICROMEGAS_DEFAULT_AUDIENCE` everywhere it is read: `blocks_view`'s own
`data_sql`, `log_entries`/`measures`, and Prong B's `IdKind::Block` arm (`owner_query_sql`, which
resolves this block alone, off `blocks.audience`, never through `P`). Unlike the NULL-anchor case
above, the anchor here is real, so the "already public" argument does not apply: before this plan,
the same block resolves through `P`'s process row to `beta` and is gated accordingly; after this
plan, until the block's own `audience` is populated, it reads as the deployment default instead —
a genuine read escalation to any default-audience reader — and `blocks` rows are immutable (§2), so
the mislabel is permanent until the row ages out under retention. The actual bound is on the
deployment, not the row: it requires a deployment already running production traffic under a
non-default audience while its ingestion tier is only partway upgraded to v8, and #1373/#1482/#1519
— the rest of the audience-stamping stack this plan builds on — are all still `## Unreleased`, so no
deployment holds a non-default audience today. Migration & Upgrade Notes carries the operational
guidance this bound depends on: every ingestion replica, not just one, has to reach v8 before
audience separation can be relied on.

- `processes_view.rs` / `streams_view.rs`: no change. Both the transform and merge queries keep
  plain `max(audience)` — see "The normal case" above.
- `log_entries` and `measures` have a different regression, not a relabeling one: `audience` itself
  is already the right anchor for the row (their rows come from a single block's payload, so there
  is no cross-block aggregation to average away). But every other column on the row — `exe`,
  `username`, `computer`, `process_properties` (`log_table_schema()` in
  `rust/analytics/src/log_entries_table.rs`; `measures` adds `realname`/`distro`/`cpu_brand` in
  `rust/analytics/src/metrics_table.rs`) — is filled from the `processes` row the block's
  `process_id` joins to (`partition_source_data.rs`'s `ProcessMetadata` construction), not from the
  block's own stamp. A block written under `alpha` naming victim `beta`'s `process_id` would
  otherwise materialize with `audience = 'alpha'` (correctly labelled) but
  `exe`/`username`/`computer`/`process_properties` copied from `beta`'s real process row —
  `alpha`'s readers, already scoped into a view they're allowed to read, would get `beta`'s real
  process metadata handed to them, which is worse than a mislabeled row since it costs the attacker
  nothing it doesn't already have (a guessed `process_id`) and gives up real victim data in return.
  Fix: the mismatch predicate below on `blocks_view`'s own `data_sql` keeps the row out of the
  `blocks` view entirely, and `partition_source_data.rs` (`fetch_partition_source_data`) sources
  `log_entries`/`measures`' blocks from `blocks`' own materialized partitions
  (`existing_partitions.filter("blocks", "global", ...)`), not from Postgres directly — so a row the
  predicate excludes was never a candidate block for `log_entries`/`measures` in the first place,
  **for every case where the predicate actually excludes it.** It does not exclude this one when
  victim `beta`'s `processes` row is a legacy, pre-v8 row whose `audience` is still NULL: the
  NULL-tolerant predicate passes the block through (§4 above), so it does materialize into
  `log_entries`/`measures` with `audience = 'alpha'` and `beta`'s real
  `exe`/`username`/`computer`/`process_properties`. That residual exposure is accepted, not
  closed by this design: `beta`'s process row here is NULL-anchored, i.e. pre-AbAC, so its
  `exe`/`username`/`computer`/`process_properties` already resolve to `MICROMEGAS_DEFAULT_AUDIENCE`
  and are already public within the deployment — `alpha`'s readers gain nothing that wasn't
  already exposed under the default audience. See Migration & Upgrade Notes for the window's
  lifetime. `log_table_schema()`, the measures schema, and `log_view.rs`'s and
  `metrics_view.rs`'s `SCHEMA_VERSION` are untouched — the mismatch predicate lives only in
  `blocks_view.rs`'s `data_sql`; no downstream view needs to carry a copy of it or repeat the check.
- `log_stats_view.rs` aggregates `log_entries` rows with `arrow_cast(max(audience), ...)`
  `GROUP BY process_id, level, target, time_bin`. Once the `blocks_view` mismatch predicate (§4) is
  in place, a `process_id`'s group can still span two audiences whenever that process's row is a
  legacy, pre-v8 row with `audience` still NULL: the predicate's NULL-tolerant pass-through lets a
  block stamped with a real, non-default audience (e.g. an attacker's) through alongside the
  process's own legacy blocks (default audience), so `max(audience)` over that group would relabel
  the victim's aggregated stats row with the attacker's stamp — the same relabeling
  `processes_view`/`streams_view` are fixed against above, for the same reason. Adding `audience`
  to the `GROUP BY` in **both** the transform and merge queries does act during that window: it
  keeps that legacy-row group from merging two audiences' rows into one labelled row. But the
  victim's own row there is a NULL-anchored, pre-v8 row, whose data is already public (it resolves
  to `MICROMEGAS_DEFAULT_AUDIENCE` — Migration & Upgrade Notes), so what the `GROUP BY` change
  protects during that window is not confidential; its real value is defense-in-depth against the
  mismatch predicate being weakened later, the same as everywhere else this pattern shows up. It
  only becomes a true no-op once the NULL-anchor window closes (see Migration & Upgrade Notes). The
  selected column and schema are unchanged, so the
  file-schema hash does not need to bump — but the grouping itself changes, which has its own
  regeneration cost; see Migration & Upgrade Notes, where it is mandatory, not optional.
  `audience` does **not** join the declared merge sort order: `log_stats` is the only view calling
  `with_merge_sort_order` (`[time_bin, process_id, level, target]`), and that builder's own
  contract already tolerates a `GROUP BY` key beyond the declared columns ("extra keys degrade to
  `PartiallySorted`, not a blocking sort" — `sql_batch_view.rs`'s `with_merge_sort_order` doc). A
  fifth, unordered group key makes DataFusion's `AggregateExec` plan the merge query as
  `InputOrderMode::PartiallySorted` instead of `Sorted` (`indices.len() != groupby_exprs.len()` in
  `datafusion-physical-plan`'s aggregate-ordering selection): still a streaming aggregation with no
  blocking `SortExec`, just not the fully-sorted mode. The extract query's top-level `ORDER BY` and
  the recorded partition `sort_order` (both still exactly `[time_bin, process_id, level, target]`)
  are unaffected — that check runs against the extract query's final, already-sorted output, not
  against the grouping. The one thing this does break is `log_stats_ordering_tests.rs`'s
  `log_stats_merge_query_stays_a_streaming_kway_merge`, which pins the shipped merge query's plan
  string to `ordering_mode=Sorted`; that assertion has to relax to accept
  `ordering_mode=PartiallySorted(...)` too, while keeping the no-`SortExec` assertion as the thing
  that actually matters (Implementation Steps step 10, Testing Strategy).

`blocks_view`'s join has no `streams.process_id = processes.process_id` predicate, so an attacker
in audience `alpha` can insert a block naming its own `stream_id` (`streams.audience = 'alpha'`)
but a victim's `process_id` (`processes.audience = 'beta'`). Left unhandled, the row would
materialize with `audience='alpha'` — visible to the attacker — while its `process_id` column
(already part of `blocks_view` today, unchanged by this plan) points at the victim's real process,
turning `SELECT process_id FROM blocks` (and the same projection through `log_entries`/`measures`)
into a cross-audience existence oracle: the attacker learns that a given `process_id` exists, and
that its owning process/stream carry a different audience than the block's own, by probing it into
rows it can read.

The author rejected filtering this at read time — no column, however discovered, should be
suppressed once a row is materialized. Instead, a block whose own `audience` disagrees with the
`streams.audience` or `processes.audience` it joins to is **excluded from materialization** by the
predicate on `blocks_view.rs`'s own `data_sql` (design above) — the predicate compares
`blocks.audience` against `streams.audience`/`processes.audience` directly in the join, with no
need for either to be projected into the Arrow schema — mirrored on `make_batch_partition_spec`'s
`source_count_query` so the two queries agree on how many source rows there are. The comparison
cannot happen earlier: a stream or block routinely arrives before its
process row exists (see [Why not a write-side gate](#why-not-a-write-side-gate)), so there is no
point before materialization where all three columns are reliably present together — but by the
time `blocks_view` materializes, they are, which is why the predicate belongs there and nowhere
else.

This is a single choke point, not one check per consumer. `blocks_view`'s materialized partitions
are what every other consumer of block data reads from — not Postgres again: `log_entries` and
`measures` (`partition_source_data.rs::fetch_partition_source_data`,
`existing_partitions.filter("blocks", "global", ...)`), and the JIT-partitioned `net_spans`,
`otel_spans`, `images`, `async_events`, and `thread_spans`, plus the per-process JIT `view_instance`
path for every view (`jit_partitions.rs::fetch_process_blocks`/`generate_process_jit_partitions`/
`generate_stream_jit_partitions`, all of which query `FROM source` against `blocks`' own partition
files) — all read blocks this same way. A row the predicate excludes was never written into a
`blocks` partition, so it is absent from every one of them for free; none of them needs, or gets,
its own copy of this check. On disagreement, a `blocks` partition simply materializes with fewer
rows than its unfiltered source count; the partition still materializes normally, it simply lacks
that one row.

A SQL predicate cannot emit a per-row `error!` the way a Rust check could. The signal moves to
partition granularity instead — but it has to be attached to the point where a partition is actually
*written*, not to every pass that merely checks whether one needs to be: `materialize_partition`
(`rust/analytics/src/lakehouse/batch_update.rs`) calls `view.make_batch_partition_spec(...)` before
`verify_overlapping_partitions` and its `PartitionCreationStrategy::Abort` early return, and the
second/minute/hour/day tasks all call `materialize_all_views` over overlapping insert ranges — so a
comparison computed inside `make_batch_partition_spec` itself would re-run and re-log/re-increment
on *every* scheduled pass over an affected range, including passes that abort and write nothing,
not just a retry or a re-merge. The comparison instead lives in `MetadataPartitionSpec::write` (the
`PartitionSpec` trait method `materialize_partition` calls only after `verify_overlapping_partitions`
has decided *not* to abort), so it runs once per partition actually written, never on a pass that
turns out to be a no-op.

Today `BlocksView::make_batch_partition_spec` only calls `fetch_metadata_partition_spec`, which owns
`source_count_query` and returns just its (now filtered) count for partition sizing — there is no
second query anywhere in this path. `BlocksView` additionally attaches the unfiltered-vs-filtered
diagnostic query text to the returned `MetadataPartitionSpec` (a new optional field, e.g.
`audience_mismatch_query: Option<Arc<String>>`, `None` for every other view built on this module) —
without running it yet. `MetadataPartitionSpec::write`, when that field is `Some`, runs it once
against the pool it is already writing through:

```sql
SELECT COUNT(*) AS unfiltered,
       COUNT(*) FILTER (WHERE NOT (<audience-mismatch predicate holds>)) AS filtered
FROM blocks, streams, processes
WHERE blocks.stream_id = streams.stream_id AND blocks.process_id = processes.process_id
AND blocks.insert_time >= $1 AND blocks.insert_time < $2
```

— the same three-table join and the same keep predicate as `source_count_query` (§4 above, itself
`NOT (<audience-mismatch predicate holds>)`), both counts read from
a single atomic pass via `FILTER` rather than two independent queries (which would race against
concurrent inserts or a `delete_expired_blocks` sweep and could come out negative), so
`unfiltered - filtered` isolates exactly the rows the predicate excluded rather than also picking up
blocks whose `streams`/`processes` row is absent for unrelated reasons (early arrival, or a
post-sweep orphan — see ["Why not a write-side gate"](#why-not-a-write-side-gate) — both routine and
both out of scope here per ["Out of scope"](#out-of-scope--follow-ups)), and can never itself go
negative. The pure helper behind that subtraction (Testing Strategy) clamps at zero regardless, as a
defensive floor — not because this query can produce a negative delta, but so the type stays a plain
count if the query is ever changed later. When the delta is nonzero, `write` logs one `error!` naming
the view instance and insert-time range and the number of blocks excluded, and increments the
`block_audience_mismatch_excluded` metric (§5) by that count. This is coarser than a per-block log —
it does not name the specific block, process, or the three audiences involved — but that per-row
detail is exactly what §5's hourly `block_audience_mismatch_rows` query
gets by reading Postgres directly, which sees precisely the rows this predicate excludes; the
partition-level `error!`/metric is the signal that a given partition's materialization was affected
at all, and the hourly query is where the detail lives. The two metrics are deliberately named
differently and never summed: both run in the maintenance role process (`materialize_all_views` and
`delete_old_data`/`EveryHourTask::run` share that process, `public/src/servers/maintenance.rs:137-156`),
so an identically-named counter from both sites would land in the same process's `measures` stream
with no tag to tell them apart, and any query over it would silently sum two incompatible
quantities — materialized-partition exclusions vs. live Postgres rows in the last hour.
`block_audience_mismatch_excluded` also double-counts across a materialization retry or a re-merge
over an overlapping insert range, since each partition actually *written* for that range re-runs the
comparison and re-increments (the comparison lives in `MetadataPartitionSpec::write`, precisely so a
pass that aborts without writing — the common case on a range that is already current — does not
also re-count it, per the design above). It is a signal that *some* write saw a mismatch, not a
running total of distinct excluded blocks. `block_audience_mismatch_rows` (§5) has no such
caveat — each hourly tick counts current Postgres state directly.

`OwnershipRewrite` needs no change: every audience-carrying view keeps its existing bare
single-`audience`-column filter, on `blocks`, `log_entries`, and `measures` exactly as on
`processes`, `streams`, and `log_stats`. It is the `blocks_view.rs` predicate above — not
`OwnershipRewrite`, and not any downstream consumer's own logic — that closes the `blocks`
existence oracle described above: the row that would have carried `audience='alpha'` while pointing
at `beta`'s `process_id` is dropped before materialization, instead of ever becoming readable to
`alpha` (or, mislabeled, to `beta`).

#### What remains process-anchored

This plan closes the row-derivation gap only where a row carries its own `audience` column.
Two classes of reader still resolve their audience *label* through the *owning* process/stream row
rather than a genuine per-row column, and this plan leaves both as they are today — but the
cross-audience *injection* scenario that motivates this whole design (an attacker's block naming a
victim's `process_id`/`stream_id`) is closed for both against a victim whose `processes`/`streams`
row carries a real, non-NULL stamp, as a side effect of where §4 puts the mismatch predicate:
`blocks_view`'s own materialized partitions are what both classes below read their blocks from
(`jit_partitions.rs::fetch_process_blocks` and its
`generate_process_jit_partitions`/`generate_stream_jit_partitions` callers query `FROM source`
against those same partitions, exactly as `log_entries`/`measures` do), so a block the predicate
excludes was never written into a `blocks` partition for either of them to find either. It is
**not** closed against a victim whose `processes`/`streams` row is a legacy, pre-v8 NULL-audience
row — the predicate's NULL-tolerant pass-through lets an attacker's block through unchecked in that
case. That is accepted, not left as an open question: a NULL-anchored victim row's data is already
public (see Migration & Upgrade Notes). What is left open is
narrower than the original gap:

- **The five views `OwnershipRewrite` resolves via `per_process_audience()`.** `net_spans`,
  `otel_spans`, and `images` are filtered through the `IN`-subquery built from
  `MAX(audience) GROUP BY process_id` over `__processes__partitions`; `async_events` and
  `thread_spans` are filtered through the equivalent `EXISTS` arms, the latter via `streams`. None
  of these five carries its own `audience` column, so their audience label is still an aggregate
  over a process's/stream's blocks rather than a per-row stamp — architecturally unchanged by this
  plan. Giving them their own columns is a materialization change (they are not block-derived
  `SqlBatchView`s the way `log_entries`/`measures`/`log_stats` are) and is out of scope here.
- **The per-process JIT `view_instance` path.** `view_instance('log_entries'|'measures', pid)`
  (`log_view.rs`, `metrics_view.rs`, and similarly `images_view.rs`,
  `otel/spans_view.rs`) resolves one `ProcessMetadata` via `find_process` and stamps every block
  it fetches with that single `process.audience`
  (`jit_partitions.rs::fetch_process_blocks` sets `process: process.clone()` on each
  `PartitionSourceBlock`; `log_entries_table.rs`/`metrics_table.rs` emit `row.process.audience`).
  Only the global, blocks-view-backed instance is per-block. For a block that does survive into a
  `blocks` partition, this is a difference in *mechanism* rather than *value* only when
  `process.audience` is itself a real, non-NULL stamp — the mismatch predicate then guarantees
  every surviving block's own stamp already agrees with it, so stamping from `ProcessMetadata`
  produces the same label the block's own column would. That equivalence does not hold for a
  process whose own row is a legacy, pre-v8 NULL-audience row: `find_process` resolves
  `process.audience` to the deployment default regardless of what its post-upgrade blocks are
  actually stamped with, so every block this path fetches for such a process is relabelled to the
  default — a *value* difference, not just a mechanism one — on top of `view_instance` being denied
  outright to a reader scoped to the block's real audience; this is accepted for the same
  public-legacy-data reason as elsewhere (see Migration & Upgrade Notes). Carrying the block's own
  stamp into the JIT path (splitting a
  per-block audience out of `ProcessMetadata`) is still worth doing for its own sake — it removes
  the aggregate dependency rather than relying on it staying safe — but it is not attempted here.

Both are pre-existing gaps, not new ones — this plan does not widen either, and narrows what they
actually expose — and both are recorded in [Out of scope](#out-of-scope--follow-ups).

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

### 5. Block/stream `process_id` and `audience` mismatch counters

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

A second counter is needed alongside it, for a divergence that is not always a bug: `blocks.audience`
disagreeing with its stream's or process's own `audience`. Before this plan, `insert_stream`/
`register_otel_stream` have no audience-conflict guard equivalent to `check_process_audience_conflict`
and use `ON CONFLICT (stream_id) DO NOTHING` (`web_ingestion_service.rs:437-440,490-493`), so an
existing stream keeps whatever audience it was first stamped with even if the ingestion credential's
bound audience is later re-pointed — not hypothetical, since streams are opened lazily over a
process's lifetime. Once the materialization-time exclusion (§4) is in place, every subsequent block
on that stream would otherwise be silently dropped from `blocks` — and so from `log_entries`,
`measures`, and every other view built from it — for every audience, with only the per-partition
`error!`/`imetric!` in `blocks_view.rs` and this hourly counter as signal that it happened.

This plan closes that write-time gap rather than only measuring its effect: `insert_stream` and
`register_otel_stream` gain a `check_stream_audience_conflict`, mirroring
`check_process_audience_conflict` exactly (same cache-then-`SELECT audience FROM streams WHERE
stream_id = $1` shape, same "row disappeared concurrently" arm, same
`IngestionServiceError::AudienceConflict`-shaped 403), gated the same way on `rows_affected() == 0`.
The write-side-lookup cost the design rejects elsewhere (§4, "Why not a write-side gate") is a
per-block cost; this guard only ever runs on the rare re-registration path, exactly like the process
guard it mirrors, so it costs nothing on the hot per-block insert. It turns a re-pointed credential
into an immediate 403 at the next `insert_stream`/`register_otel_stream` call for that stream,
instead of a permanent silent drop discovered only after the fact. It does not close the gap
retroactively for a stream re-registration that already happened before this plan shipped, and it
does not help a stream that is never re-registered after its credential is re-pointed (blocks keep
arriving on an already-open stream with no further `insert_stream` call to catch them) — the hourly
counter below stays as the signal for that residual case and for sizing it.

This counter has to count exactly what the §4 predicate excludes, or it cannot honestly claim to
size "how much otherwise-legitimate telemetry the materialization-time skip silently drops" (below).
A naive `b.audience IS DISTINCT FROM s.audience OR b.audience IS DISTINCT FROM p.audience` is the
wrong comparison: it's a raw-column check, not the predicate's NULL-tolerant one (§4), so it
over-reports every legacy row that carries `NULL` on one side against a real, non-default stamp on
the other — a case §4's predicate deliberately lets through, not a case it drops. So the two must
share one expression rather than be maintained as two independently-written SQL strings. Add, next
to `coalesced_audience_column` in `rust/analytics/src/audience.rs`:

```rust
/// True when `left_qualifier.audience` and `right_qualifier.audience` do NOT agree -- the mirror
/// image of the NULL-tolerant mismatch predicate `blocks_view.rs`'s `data_sql` filters on (§4).
/// Both qualifiers must be trusted table names/aliases, never user input.
pub fn audience_column_mismatch(left_qualifier: &str, right_qualifier: &str) -> String {
    format!(
        "{left_qualifier}.audience IS NOT NULL AND {right_qualifier}.audience IS NOT NULL \
         AND {left_qualifier}.audience <> {right_qualifier}.audience"
    )
}
```

`blocks_view.rs`'s `data_sql`/`source_count_query` predicate (§4) is restated as `NOT
(audience_column_mismatch("blocks", "streams") OR audience_column_mismatch("blocks", "processes"))`.
Expanding `audience_column_mismatch` and applying De Morgan's law, `NOT (a IS NOT NULL AND b IS NOT
NULL AND a <> b)` becomes `a IS NULL OR b IS NULL OR a = b` per pair — **logically equivalent** to
the hand-written NULL-tolerant form shown in §4, not textually identical to it (the generated SQL
text is `NOT (blocks.audience IS NOT NULL AND streams.audience IS NOT NULL AND blocks.audience <>
streams.audience OR blocks.audience IS NOT NULL AND processes.audience IS NOT NULL AND
blocks.audience <> processes.audience)`, which a query planner treats the same as §4's `(blocks.audience
IS NULL OR streams.audience IS NULL OR blocks.audience = streams.audience) AND (...)` but which is
different SQL text). Both forms are correct and NULL-safe; what matters is that the predicate is
built from the same function the counter uses below, so the two can never drift independently. The
hourly counter becomes:

```sql
SELECT count(*) FROM blocks b
JOIN streams s ON s.stream_id = b.stream_id
JOIN processes p ON p.process_id = b.process_id
WHERE (<audience_column_mismatch("b", "s")> OR <audience_column_mismatch("b", "p")>)
AND b.insert_time >= $1
```

reported as `imetric!("block_audience_mismatch_rows", "count", n)` and a `warn!` when non-zero.
Unlike the `process_id` counter, a non-zero reading here is not necessarily something to fix in
code — it may be the expected result of a re-pointed credential. This metric is named and reasoned
about separately from the per-partition `block_audience_mismatch_excluded` metric above: both would
otherwise land in the same maintenance-role process's `measures` stream under one name with no tag
to distinguish a live Postgres row count from a per-partition exclusion count, and any query
summing them would be summing two incompatible quantities (§4).

Both counters are a pre-flight, but for different follow-ups. The `process_id` counter sizes how
often that mismatch happens at all, ahead of the deferred hard-reject follow-up below. The
`audience` counter sizes something already live once §4 ships: how much otherwise-legitimate
telemetry the materialization-time skip silently drops — and because it now shares the predicate's
own NULL-tolerant comparison, that claim is accurate rather than only approximately so. The
re-pointed-credential scenario above is exactly the case it exists to catch. A deployment should
watch `block_audience_mismatch_rows` read a flat zero for a representative period before trusting
that the skip is only ever discarding attacker-injected blocks and not its own legitimate
telemetry; a nonzero, non-attack reading is a sign to fix the underlying cause (e.g. stop
re-pointing a credential's audience mid-stream) rather than to treat the drop as expected.

The `process_id` hard reject stays deferred to a follow-up, to be opened once there is a
measurement to justify paying for the write-path lookup. The `audience` mismatch handling is not
deferred in the same way — the skip-and-log in §4 ships as part of this plan — but relies on this
counter, in a given deployment, as the evidence that its drops are safe to leave silent.

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

blocks_view  audience = COALESCE(blocks.audience, default)  -> log_entries, measures, log_stats
                                                             -> processes_view.audience (max, by process_id)
                                                             -> streams_view.audience   (max, by stream_id)

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
   in each; give `insert_block_typed`'s `INSERT` an explicit column list. Add
   `check_stream_audience_conflict` (§5), mirroring `check_process_audience_conflict`, and call it
   from `insert_stream`/`register_otel_stream` on `rows_affected() == 0`. Drop the two known-gap
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
   with `coalesced_audience_column`, add `audience_column_mismatch` (§5), drop the
   `AUDIENCE_PROPERTY` re-export, and write the precedence rule (§1) into the module doc.
9. `rust/analytics/src/lakehouse/blocks_view.rs`: new `data_sql` with the NULL-tolerant
   audience-mismatch predicate (§4) added to both `data_sql` and `make_batch_partition_spec`'s
   `source_count_query` — the predicate references `streams.audience`/`processes.audience` directly
   in plain Postgres SQL, with no new column appended to the Arrow schema, and (as today) only
   `$1`/`$2`, so `rust/analytics/src/lakehouse/metadata_partition_spec.rs`'s
   `fetch_metadata_partition_spec` needs no new bind and `MetadataPartitionSpec::default_audience`'s
   existing doc comment (which already says `source_count_query` gets no `$3` bind) stays accurate
   as written.
   `BlocksView::make_batch_partition_spec` additionally attaches the unfiltered-vs-filtered
   diagnostic query text to the returned `MetadataPartitionSpec` (new field on
   `metadata_partition_spec.rs`'s `MetadataPartitionSpec`, e.g.
   `audience_mismatch_query: Option<Arc<String>>`, `None` for every other view built on this module)
   without running it. `MetadataPartitionSpec::write` — called only after `verify_overlapping_partitions`
   has decided not to `Abort`, i.e. only when a partition is actually about to be written — runs it
   once directly against the pool: the same `blocks, streams, processes` join as `source_count_query`,
   computing both the unfiltered count and the surviving (mismatch-excluded rows kept out) count
   atomically in a single `SELECT ... COUNT(*) FILTER (WHERE NOT (...))` (not two separate `COUNT(*)` queries compared
   across connections, which would be non-atomic and could go negative) — so the delta isolates
   exactly the excluded rows rather than also counting blocks whose `streams`/`processes` row is
   routinely absent (early arrival, post-sweep orphan), logging one `error!` and incrementing
   `imetric!("block_audience_mismatch_excluded", "count", n)` (§5) when the delta is nonzero. Moving
   this into `write` (rather than into `make_batch_partition_spec`, which every pass over an insert
   range calls regardless of whether anything ends up being written) keeps the metric scoped to
   partitions that are actually materialized, not to every scheduled pass over an affected range.
   `blocks_file_schema_hash()` stays `vec![5]` — no bump, no forced rebuild (§4, Migration & Upgrade
   Notes).
   This one predicate is the sole
   exclusion point — `partition_source_data.rs` needs no mismatch-handling code of its own, since
   it (and every JIT-partitioned view) reads blocks from `blocks_view`'s own materialized
   partitions and so never sees a row this predicate excluded (§4).
10. `rust/analytics/src/lakehouse/processes_view.rs` / `streams_view.rs`: no code change — both keep
    plain `max(audience)` (§4, "The `max(audience)` regression": every surviving block agrees with
    its process's/stream's audience whenever that row's own stamp is non-NULL, which is the only
    case `max(audience)` needs to handle correctly; the NULL-anchor exception is accepted, not
    fixed, on already-public-legacy-data grounds).
    `log_stats_view.rs`: add `audience` to the `GROUP BY` in both the transform and merge queries —
    a change independent of the removal above, since `log_entries.audience` is the block's own
    column and was never sourced from either of the columns this plan removes; mandatory
    regeneration of `log_stats` partitions over the retention window is part of this step, not a
    follow-up (see Migration & Upgrade Notes). The declared `with_merge_sort_order` columns and the
    transform query's top-level `ORDER BY` stay exactly `[time_bin, process_id, level, target]` —
    `audience` does not join them (§4). Update `rust/analytics/tests/log_stats_ordering_tests.rs`'s
    `log_stats_merge_query_stays_a_streaming_kway_merge` in the same step: relax its pinned
    `plan_str.contains("ordering_mode=Sorted")` assertion to also accept
    `ordering_mode=PartiallySorted(...)`, which is what the fifth, unordered `GROUP BY` key now
    produces (§4); its no-`SortExec` assertion is unaffected and stays as the real regression
    guard.
    `ownership_rewrite.rs` needs no code change — every audience-carrying view, `blocks` included,
    keeps its existing bare single-`audience`-column filter (§4); only its module doc is touched
    (see [Documentation](#documentation)).
11. `rust/analytics/src/metadata.rs`: `find_process` uses `coalesced_audience_column`.
12. `rust/analytics/src/lakehouse/audience_guard.rs`: rewrite `owner_query_sql`'s three arms, drop
    the `AUDIENCE_PROPERTY` bind, renumber the default to `$2`, and update the module doc's
    "one cache, one question" and fail-closed paragraphs for the orphaned-row behaviour change.

### Phase 4 — integrity measurement

13. `rust/public/src/servers/maintenance.rs`: hourly `block_stream_process_id_mismatch` and
    `block_audience_mismatch_rows` counts, the latter built from `audience_column_mismatch` (§5,
    added in step 8) so it shares the same comparison as the `blocks_view.rs` predicate (step 9,
    restated in terms of it) and the two can't drift apart. Each gets its own `imetric!` + `warn!`
    (§5) — named distinctly from `blocks_view.rs`'s per-partition `block_audience_mismatch_excluded`
    (step 9) since both run in the same maintenance-role process.

### Phase 5 — tests, docs, tooling

14. Tests (see [Testing Strategy](#testing-strategy)).
15. `local_test_env/ai_scripts/import_net_blocks_from_prod.py`: project the single `blocks.audience`
    column (§3) into all three `_project(...)` calls, and add the missing `format` projection.
16. Docs and `CHANGELOG.md` (see [Documentation](#documentation)).
17. `tasks/data_isolation/audience_based_access_control_plan.md` §11b: replace the "residual,
    deferred to Stage 5b" text with what actually shipped.

## Files to Modify

- `rust/ingestion/src/sql_migration.rs`
- `rust/ingestion/src/web_ingestion_service.rs`
- `rust/telemetry/src/property_names.rs`, `rust/telemetry/src/property.rs`
- `rust/public/src/servers/ingestion.rs`, `rust/public/src/servers/maintenance.rs`,
  `rust/public/src/servers/flight_sql_service_impl.rs` (doc comment only — `do_put_statement_ingest`'s
  `is_admin` rationale, see [Documentation](#documentation))
- `rust/otel-ingestion/src/handler.rs`
- `rust/analytics/src/lib.rs` (doc comment only — the `audience` module summary),
  `rust/analytics/src/replication.rs`, `rust/analytics/src/audience.rs`,
  `rust/analytics/src/metadata.rs`
- `rust/analytics/src/lakehouse/blocks_view.rs` (the audience-mismatch predicate on
  `data_sql`/`source_count_query`, plus attaching the per-partition mismatch diagnostic query to
  `MetadataPartitionSpec` in `make_batch_partition_spec`, §4 — the single exclusion point every
  consumer of `blocks` inherits; `partition_source_data.rs` needs no change of its own),
  `metadata_partition_spec.rs` (the new `audience_mismatch_query` field and the `error!`/`imetric!`
  emission moved into `MetadataPartitionSpec::write`, so it fires only when a partition is actually
  written, §4), `log_stats_view.rs`,
  `audience_guard.rs`, `ownership_rewrite.rs` (module doc
  only — the stale "what remains open" paragraph, narrowed per §4; `audience_column_predicate`
  itself is unchanged) — `processes_view.rs`/`streams_view.rs` need no change (§4, "The
  `max(audience)` regression": both keep plain `max(audience)`)
- Tests: `rust/ingestion/tests/audience_stamping_db_test.rs`,
  `rust/ingestion/tests/write_audience_tests.rs`, `rust/analytics/tests/common/db_fixtures.rs`,
  `rust/analytics/tests/audience_guard_tests.rs`, `rust/analytics/tests/prong_b_guard_db_test.rs`,
  `rust/analytics/tests/ownership_rewrite_db_test.rs`,
  `rust/ingestion/tests/insert_block_dedup_db_test.rs` (its raw positional `INSERT INTO blocks`),
  `rust/analytics/tests/log_stats_ordering_tests.rs` (relax the pinned `ordering_mode=Sorted`
  assertion to also accept `PartiallySorted`, §4/step 10),
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

**One audience column on `blocks` vs. two extra anchor columns.** An earlier draft of this plan
appended `stream_audience`/`process_audience` to `blocks_view` so `processes_view`/`streams_view`
could anchor their `max(audience)` on the joined row's own stamp instead of the block's. Once the
`blocks_view` mismatch predicate (§4) is in place, every surviving block in a
`processes_view`/`streams_view` group has `audience == streams.audience == processes.audience`
**for a process/stream whose own row already carries a real stamp** — for that case, bare
`max(audience)` is already correct, so the extra columns and the switch would have been pure
defense-in-depth, not a prerequisite for the fix. The decision taken here is not to pay that cost:
`blocks` keeps exactly one audience column — the block's own stamp — the mismatch predicate is the
single control, and `processes_view`/`streams_view` keep plain `max(audience)` unchanged. The
accepted cost is the case the columns would have hedged against: for a process/stream whose row is
still a legacy, pre-v8 NULL-audience row, a block that survives the predicate can still relabel that
row via `max(audience)` — but what it relabels there is already-public legacy data (Migration &
Upgrade Notes), so the exposure is accepted on the same grounds as the rest of the NULL-anchor
window; see [The `max(audience)` regression](#the-maxaudience-regression) for the full accounting.

**Skipping a mismatched block vs. failing its partition.** The alternative to a per-block skip is
to fail materialization of the whole partition the block falls in when a mismatch is found. That
would make the *integrity* signal loud — a partition simply won't build until the bad row is dealt
with — but it converts the integrity gap into an availability one: an attacker who only guesses a
victim's `process_id` (no read access needed) could wedge materialization of every partition
covering that block's time range, denying every audience the data in it, not just the victim's.
Skipping avoids that: the row that fails the check is dropped from the `blocks` partition, the rest
of the partition materializes normally, and no attacker input can block the pipeline. Because it is
`blocks_view`'s own predicate that drops the row — and every other view materializes its blocks by
reading `blocks`' own partitions rather than Postgres again (§4) — this one skip is what every
consumer inherits; there is no second skip-and-log elsewhere to keep in sync with it. The cost is
that mismatched telemetry — including a legitimate stream's, if its `audience` has gone stale
against a re-pointed credential (§5) — is now silently dropped from every view rather than causing
a visible build failure, and the drop can only be logged at partition granularity: a SQL predicate
has no per-row `error!` the way a Rust check would, so `MetadataPartitionSpec::write` — run only for
a partition that is actually written, not on every pass that merely checks whether one is needed
(§4) — logs one `error!` and one `imetric!("block_audience_mismatch_excluded", "count", n)` per
affected partition, naming the count excluded but not the individual blocks (and note this count can
still double-count across a materialization retry or a re-merge over an overlapping insert range,
since each such write re-runs the comparison, §4). That
partition-level signal and §5's hourly `block_audience_mismatch_rows` counter —
which queries Postgres directly and so does carry the per-block, per-process, per-audience detail —
are therefore not optional instrumentation but the only record that data was discarded at all; the
hourly counter is the pre-flight that sizes how much legitimate telemetry would be affected before
relying on the skip in a given deployment. This is a deliberate choice of a quiet, bounded loss over
a loud failure that an attacker can trigger on demand.

**Measuring the `process_id` mismatch from maintenance vs. at write time.** A write-time check
would reintroduce the cache-and-TTL machinery this design removes, in order to count something
expected to be zero, on the hottest path in the system. An hourly bounded query over an indexed
join gives the same number for nothing. The cost is latency to detection — up to an hour — which is
acceptable for a signal that no longer gates security.

**No backfill.** A NULL column resolving to the deployment default *is* today's semantics for a
row's *own* read, so a legacy row or admin `bulk_ingest` row still resolves to the same audience it
does today when read in isolation. It is not behaviourally inert for the §4 mismatch predicate,
though: a backfilled column would remove the NULL escape hatch that predicate's NULL-tolerant form
relies on, changing what it excludes (see the NULL-tolerant-window discussion in Migration &
Upgrade Notes). Absent that
predicate interaction, backfilling would mean a full table rewrite of `blocks` for a legacy row's
own resolved value.

**No forced rebuild of `blocks`.** `blocks_file_schema_hash()` stays `vec![5]`; nothing bumps it.
The Arrow schema this plan produces is byte-identical to what's already on disk, so this is possible
without breaking anything — the alternative would have been to bump the hash purely to force
re-materialization, not because the schema needed it. Doing that would mean re-materializing `blocks`,
the largest view in the lake, plus the SqlBatchViews built over it, entirely to relabel data that
is already public in this deployment (Migration & Upgrade Notes). Not worth it. The price is a
consistency gap, not a confidentiality one: existing partitions keep old-semantics `audience` values
and may contain rows the §4 predicate would now exclude, alongside new partitions materialized under
the new semantics, until the old ones age out of the retention window — or an operator runs
`regenerate_partitions` over `blocks` deliberately, for anyone who wants uniform semantics sooner.

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
- **NULL-tolerant window on the mismatch predicate.** Because `processes`/`streams` rows are
  immutable (`ON CONFLICT (process_id|stream_id) DO NOTHING`) and there is no property-to-column
  backfill (§2), a process or stream registered before its ingestion binary reached v8 keeps
  `audience = NULL` for the rest of its life, while its post-upgrade blocks carry a real, non-NULL
  stamp. The §4 predicate's NULL-tolerant form (`x.audience IS NULL OR y.audience IS NULL OR
  x.audience = y.audience`) is what keeps those blocks materializing instead of being silently
  dropped for that process's/stream's remaining lifetime — the alternative `COALESCE`-then-compare
  form would exclude them permanently, not just during the rollout. The same pass-through applies
  mid-rolling-upgrade, when a pre-v8 ingestion binary writes NULL-audience blocks onto a stream a
  v8 binary already stamped. **This is not a cosmetic trade-off — it leaves the cross-audience
  injection attack this plan sets out to close fully open against any `processes`/`streams` row
  whose `audience` is NULL.** A block landing on a legacy NULL-audience process/stream is not
  checked against that row's audience at all, so an attacker in audience `zeta` who names such a
  `process_id`/`stream_id` still materializes a row labelled `audience = 'zeta'` and, for
  `log_entries`/`measures`, carrying the victim's real `exe`/`username`/`computer`/
  `process_properties` (§4). A NULL row does not resolve to "the attacker's own audience" in a way
  that makes it safe to target on that account: it resolves to
  `MICROMEGAS_DEFAULT_AUDIENCE` at read time, which is a distinct audience from the attacker's
  whenever the deployment default differs from the attacker's own — so targeting a NULL-anchored
  row is exactly a cross-audience exploit, not a no-op. This window persists for as long as any
  pre-v8 `processes`/`streams` row exists, since those rows are immutable and this plan does no
  backfill (§2).

  **This is a known, accepted residual exposure, not an open question.** A NULL `audience` on a
  `processes`/`streams` row means the row predates per-row stamping — it is pre-AbAC data that
  resolves to `MICROMEGAS_DEFAULT_AUDIENCE`, the deployment's public/legacy audience. The metadata
  an attacker reaches by naming such a `process_id` — `exe`/`username`/`computer`/
  `process_properties` — is therefore already public: the pass-through leaks nothing that was
  confidential to begin with. That is why the alternatives are not worth their cost: a strict
  `COALESCE`-then-compare predicate would permanently drop legitimate post-upgrade telemetry from
  every such process/stream, not just during the rollout, and a one-time backfill of the column
  would cost the full-table rewrite of `blocks` (and `processes`/`streams`) the v8 migration is
  specifically designed to avoid (§2, "No backfill" above) — and would still not fully close the
  window against a mixed-version rolling fleet, since a pre-v8 binary can keep writing NULL-audience
  rows after any backfill pass runs. The NULL-tolerant pass-through as written stays.
- **The reverse direction: a NULL block against a real anchor.** The same NULL-tolerant predicate
  also passes a block whose *own* `audience` is NULL through against a `processes`/`streams` anchor
  that already carries a real, non-NULL stamp — reachable when a pre-v8 ingestion replica (still
  issuing the old positional `INSERT INTO blocks VALUES($1..$11)`) writes a block for a process a
  v8 replica already registered with a real audience. Unlike every other case in this section, the
  anchor here is not NULL, so "the row was already public" does not apply: before this plan the
  block resolves through its process to that real audience and is gated accordingly; after this
  plan it resolves to `MICROMEGAS_DEFAULT_AUDIENCE` until its own column is populated, which — since
  `blocks` rows are immutable — is never. That is a genuine, if narrow, read escalation, not an
  accepted no-op (see [The `max(audience)` regression](#the-maxaudience-regression) for the full
  mechanism). Its bound is on the deployment, not the row: it requires production traffic already
  running under a non-default audience while the ingestion tier is only partway upgraded to v8, and
  #1373/#1482/#1519 — the rest of the audience-stamping stack this plan builds on — are all still
  `## Unreleased`, so no deployment holds a non-default audience as of this plan. The operational
  mitigation is the deploy-order guidance above, generalized: bring every ingestion replica to v8,
  not just one, before relying on audience separation during a rolling upgrade.
- `blocks_file_schema_hash()` stays `vec![5]` — no bump, and deliberately so rather than as a
  side effect. The Arrow schema doesn't change, so nothing would auto-invalidate `blocks` partitions
  on its own either way; on top of that, the author chose not to force a rebuild, to avoid
  re-materializing the largest view in the lake. Existing `blocks` partitions keep the `audience`
  values they were written with under the old per-process-property query, and may contain rows the
  §4 mismatch predicate would now exclude; new partitions get the new per-row semantics as soon as
  the v8 ingestion binary is live, with old- and new-semantics partitions queried side by side in
  between. Because every partition in the lake today holds public data, this is a consistency note
  rather than a confidentiality exposure — the per-row guarantee simply takes effect for everything
  ingested from here on, and the existing partitions age out of the retention window on the usual
  schedule without ever having held anything non-public. `regenerate_partitions` over `blocks`
  remains available to an operator who wants uniform, per-row semantics sooner than that.
- `blocks` joins that same not-auto-invalidated set deliberately (above); `processes`/`streams`/
  `log_entries`/`measures` land there too, but because their schema simply never changed rather than
  by choice. They are all `SqlBatchView`s that hash their inferred
  Arrow schema (`SqlBatchView::get_file_schema_hash`), which does not change for any of them —
  `log_view.rs`/`metrics_view.rs` return a constant `vec![SCHEMA_VERSION]`, and
  `processes_view.rs`/`streams_view.rs` are equally schema-stable — so **none** of the four is
  auto-invalidated, the same as `processes`/`streams`. For a process/stream whose own row already
  carried a real, non-NULL stamp before this change, content is identical to the old materialization
  for a row that was actually attacked: the mismatched block never reaches `blocks` at all (§4), so
  `processes_view`/`streams_view`'s unchanged `max(audience)` has nothing attacker-controlled left
  to aggregate, and `log_entries`/`measures` simply omit the mismatched block's rows rather than
  carrying its joined process metadata — so no regeneration is required on that account.
  **That is not the only source of difference.** A process/stream registered before its ingestion
  binary reached v8 has `audience = NULL` for the rest of its life (rows are immutable, §2 does no
  backfill), while every post-upgrade block of that same process/stream carries the credential's
  real, resolved audience. `find_process` and `processes_view`/`streams_view` resolve the legacy
  row to the deployment default, while its post-upgrade `blocks`/`log_entries`/`measures` rows
  carry the real audience — the two halves of the same process end up under two different
  audiences. This is a genuine break for legitimate operation, not just an attacked row: a reader
  scoped to the real audience sees the process's telemetry but not its `processes`/`streams` row
  (`OwnershipRewrite` filters each view on its own `audience` column), is denied
  `view_instance('log_entries'|'measures', pid)` for it (`AudienceGuard::authorize_view_instance`
  resolves `IdKind::ProcessOrStream` off `processes.audience`, which reads as the default, not the
  real audience), while a reader scoped to the deployment default sees the process row but none of
  its post-upgrade telemetry. This split persists for as long as the legacy row exists and is not
  fixed by regeneration — regenerating `processes`/`streams`/`log_entries`/`measures` partitions
  changes nothing about what `audience` a NULL `processes`/`streams` row resolves to. This split is
  accepted for the same reason as the mismatch-predicate window above: the legacy `processes`/
  `streams` row it stems from is pre-AbAC, public/legacy data, so a reader denied `view_instance`
  for it, or a default-audience reader who can't see its post-upgrade telemetry, is not being denied
  anything confidential — it is an operational rough edge of the NULL-anchor window, not a
  confidentiality gap, and the NULL-tolerant pass-through (as written) is kept rather than traded for
  a strict comparison or a backfill. An
  operator who wants strict consistency for the disjoint, already-real-stamp case above can still
  `regenerate_partitions` over `blocks`, `processes`, `streams`, `log_entries`, `measures` for the
  retention window.
- **`log_stats` is different: regeneration over the retention window is mandatory, not optional.**
  `log_stats_view.rs` is equally schema-stable (`SqlBatchView::get_file_schema_hash` hashes only the
  output schema, not the grouping), so nothing auto-invalidates its partitions either — but adding
  `audience` to its `GROUP BY` (§4) changes the *grouping itself*, not just a value on an existing
  row. Partitions materialized before this change keep rows grouped *without* audience, mixing
  what should now be separate per-audience rows into one, pre-existing row; that is a shape
  disagreement with freshly materialized partitions, not just a stale value, and it is not fixed by
  the mismatch predicate or by time — it persists until those partitions are regenerated. This plan
  therefore requires `regenerate_partitions` to be run over `log_stats` for the retention window as
  part of the v8 rollout, not left to an operator's discretion.
- No client change. No wire-format change. Native and OTLP producers are unaffected.

## Documentation

- `mkdocs/docs/query-guide/schema-reference.md` — the user-facing SQL-surface reference for the
  `audience` column. Retitle the per-view description (`:47,78,138,174,217,290`) from "The
  audience of the owning process" to reflect the per-row stamp (each view's own `audience` for
  `processes`/`streams`/`blocks`, the block's stamp for `log_entries`/`measures`/`log_stats`, and
  the process/stream stamp specifically for the process/stream-anchored views still listed under
  "What remains process-anchored"). No new columns to document — `blocks` keeps exactly one
  `audience` column. Update the `:623-635` paragraph on where the default is
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
    row. It also does not close it against any `processes`/`streams` row whose `audience` is
    still NULL — a row registered before its ingestion binary reached v8 — since the §4 mismatch
    predicate's NULL-tolerant pass-through lets an attacker's block through unchecked against such
    a row for as long as it exists (Migration & Upgrade Notes). This is an accepted, bounded
    limitation, not an open gap: a NULL-anchored row is pre-AbAC data that already resolves to the
    deployment's public/legacy default audience, so what an attacker reaches through it was already
    public. Rewrite the admonition to describe
    that narrower, remaining surface — including the NULL-anchor window and why it is accepted —
    rather than deleting it.
    Keep the process-squatting paragraphs inside it (they describe a different, already-closed
    gap) by lifting them into the surrounding prose.
  - `:205` "the two prongs read different copies of `micromegas.audience`": still true (Prong A
    reads a materialized snapshot, Prong B reads Postgres live), but retitle to the column.
  - `:160`, in "Audience Filtering Activation": describes Prong A as showing "processes whose
    (client-asserted) `micromegas.audience` property resolves to one of their own audiences" —
    update to describe the per-row `audience` column each of the six directly-filtered views now
    carries, not a process property.
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
  non-zero reading is a bug or an attack (§5). Add **two** further reference entries, kept separate
  because they measure different things and must never be summed (§4/§5):
  - `block_audience_mismatch_rows` (`count`) — the hourly Postgres count, alongside
    `block_stream_process_id_mismatch` in the same table row. A non-zero reading is not necessarily
    a bug (it may reflect a re-pointed credential, §5) but always means telemetry was silently
    dropped from `blocks` (and so from `log_entries`/`measures`/`log_stats` and every other view
    built from it), so it belongs in the same alerting shape.
  - `block_audience_mismatch_excluded` (`count`) — emitted from `MetadataPartitionSpec::write` (§4)
    only for a `blocks` partition that is actually written, not on every scheduled pass over an
    affected insert-time range and not on the hourly cadence. Document that it still double-counts
    across a materialization retry or a re-merge over an overlapping insert range — each such write
    re-runs the comparison — so it is a "some write saw a mismatch" signal rather than a running
    total of distinct excluded blocks — `block_audience_mismatch_rows` is the metric to trust for
    sizing the drop.
- `rust/analytics/src/lakehouse/ownership_rewrite.rs`'s module doc:
  - the "What remains open, tracked separately" paragraph is only partly obsolete — narrow it to
    the five `per_process_audience()`-resolved views, the JIT `view_instance` path (§4, "What
    remains process-anchored"), and the NULL-anchor window against a pre-v8 `processes`/`streams`
    row (§4, Migration & Upgrade Notes) — the latter documented as an accepted, bounded limitation
    over already-public legacy data, not an open item; its
    operational-mitigation advice (audience-bound DB-backed credentials only) still applies to
    that narrower surface and should stay.
  - the "One audience per process, not per row" section (`:36-47`) justifies filtering the six
    column-carrying views one row at a time with "a process's audience is write-once and always
    present" — under per-row stamping the *conclusion* still holds (each row's own column is
    sound to filter on directly), but the *reason* changes: it is no longer that a row's audience
    is derived from a process-level invariant, it is that every row now carries its own stamp from
    the moment it was written. Update the section to state that directly, and note the NULL-anchor
    exception (a pre-v8 row's column is absent, not "always present", until it resolves through
    `COALESCE` at read time).
  - the `## micromegas.audience is server-written and authenticated` section (`:74-90`) describes a
    property stamped on `processes` at registration and read through `COALESCE` at query time — the
    whole section now describes a column instead: rewrite "A process is stamped with
    `micromegas.audience` at registration ... resolved to the deployment's
    `MICROMEGAS_DEFAULT_AUDIENCE` where the audience is *read*" to describe the `audience` column
    (§1's precedence rule) rather than a property, and drop the "or one written by the admin
    `bulk_ingest`/replication path ... keeps no property at all" clause — replication now hard-fails
    on a missing `audience` column rather than writing one with none (§3, §6 above). The
    registration-conflict and "what remains open" paragraphs that follow are covered by the two
    bullets above.
- `rust/public/src/servers/flight_sql_service_impl.rs:1282-1290` — `do_put_statement_ingest`'s doc
  comment justifies gating the RPC on `is_admin` entirely in terms of writing `micromegas.audience`
  **properties** verbatim on `processes` rows. After this plan that gate protects a verbatim write
  of the authoritative `audience` **column** on all three tables — a strictly stronger capability
  than a property write (§3, "Admin replication") — so the comment's security rationale needs
  rewriting to name the column, not the property, as what an ordinary authenticated client must not
  be able to set directly.
- `rust/analytics/src/lib.rs:14` — the `audience` module doc comment reads "The single
  `micromegas.audience` property constant and SQL fragment shared by the writer and both
  enforcement prongs"; `PROPERTY_AUDIENCE` and the property-based SQL fragments are removed by this
  plan (§3, §4), so this needs to describe `coalesced_audience_column`/`audience_column_mismatch`
  and the precedence rule (§1) instead.

## Testing Strategy

**Unit / offline** (plain `cargo test`)

- `write_audience_tests.rs`: drop the `finalize_process_properties` assertions, keep the
  `strip_reserved_properties` and `WriteAudience` charset ones.
- `audience.rs` unit tests: `coalesced_audience_column` emits `COALESCE(x.audience, $n)`, keeps the
  default a bind parameter, and honours a caller-chosen placeholder index; `audience_column_mismatch`
  (§5) emits the expected `IS NOT NULL ... AND <> ` text for a given pair of qualifiers.
- `audience_guard_tests.rs`: `is_readable`/`merge_owner_rows` are unchanged, but assert
  `owner_query_sql` no longer mentions `unnest` or `properties` for any `IdKind`.
- `blocks_view.rs` (or a small colocated module) unit test: `mismatch_excluded_count` (§4, Testing
  Strategy) returns `0` when the unfiltered and filtered counts agree, the difference when they
  don't, and `0` (not a negative number) if ever called with `filtered > unfiltered` — the pure
  arithmetic behind the per-partition `error!`/`imetric!`, tested directly instead of through those
  side effects. In practice the two counts come from one atomic `COUNT(*) FILTER (WHERE ...)` query
  (§4), so `filtered > unfiltered` should not occur; the clamp is a defensive floor on the helper's
  contract, not a case this plan expects to hit.
- `log_stats_ordering_tests.rs`: update `log_stats_merge_query_stays_a_streaming_kway_merge` for
  the `GROUP BY audience` change (§4/Implementation Steps step 10) — the shipped merge query now
  plans as `InputOrderMode::PartiallySorted` rather than `Sorted`, since `audience` is a fifth
  group key not covered by the declared `[time_bin, process_id, level, target]` scan ordering, so
  the assertion widens to accept either `ordering_mode=Sorted` or `ordering_mode=PartiallySorted`
  while still asserting no `SortExec` appears. `log_stats_extract_query_satisfies_its_declared_sort_order`
  needs no change — the extract query's top-level `ORDER BY` and declared columns are untouched.

**DB-backed** (`#[ignore]`, live Postgres + object store — the existing harness pattern)

- `audience_stamping_db_test.rs`: rewrite `read_audience_property` → `read_audience_column`, and
  `strip_audience_property` → `UPDATE ... SET audience = NULL` for fabricating a legacy row. The
  conflict-guard cases (same audience, different audience → 403, legacy NULL row vs. default) all
  carry over unchanged in intent.
- **New**: the same three conflict-guard cases (same audience → no-op, different audience → 403,
  legacy NULL-audience row vs. default) for `check_stream_audience_conflict` (§5) via
  `insert_stream`/`register_otel_stream` re-registering an existing `stream_id` under a different
  audience — mirroring the `insert_process`/`register_otel_process` coverage above.
- **New**: stamp round-trip for streams and blocks — `insert_stream`/`insert_block_typed` under
  audience `alpha` land rows whose `audience` column reads back `alpha`.
- **New, `audience_mismatch_skip_db_test.rs`, the actual regression this closes**: a block written
  under audience `alpha` carrying a `process_id` owned by `beta` is absent from the materialized
  `blocks` partition covering its time range — the single exclusion point (§4) — while every other
  block's rows in that same partition are present; the partition materializes successfully, it
  simply omits the offending block. Assert the block is equally absent from `log_entries` and
  `measures` for the same time range, as a consequence of `blocks_view`'s own exclusion rather than
  any check of their own, and that `beta`'s `processes_view`/`streams_view` rows keep
  `audience = 'beta'` — the excluded block never reaches the `max(audience)` aggregate those views
  compute over `blocks`, so plain `max(audience)` (unchanged, §4 "The `max(audience)` regression")
  has nothing attacker-controlled left to relabel it with. Assert `log_stats` too: the victim's
  `log_stats` row (grouped by `process_id, level, target, time_bin`) keeps `audience = 'beta'`
  unchanged and gets no extra `audience = 'alpha'` row from the excluded block — this is the only
  coverage for the `GROUP BY audience` change in `log_stats_view.rs` (a schema-hash mismatch would
  not otherwise catch it, since `SqlBatchView::get_file_schema_hash` hashes only the output schema,
  not the grouping); the predicate makes the injected-block attack structurally unreachable for
  `log_stats` the same way it does for `processes_view`/`streams_view`. Assert the mismatch on
  observable state, not on
  the `error!` log line or the `imetric!` counter: `rust/ingestion/tests/insert_block_dedup_db_test.rs:8-9`
  states the repo's convention explicitly ("Assertions are on observable state ..., not on the
  `imetric!` counters themselves"), and `jit_partition_bounds_tests.rs:118-121` follows the same
  pattern for its own error path; neither `rust/analytics/tests` nor `rust/ingestion/tests` has a
  log/metric capture harness to assert against. Assert instead: the materialized partition's row
  count equals the unfiltered source count minus one (the excluded block), and that
  `make_batch_partition_spec`'s excluded-count computation — the `unfiltered_count -
  filtered_count` comparison, both counts read from the single atomic `COUNT(*) FILTER (WHERE ...)`
  query (§4), that decides whether to log/increment the metric at all — is exercised through a small
  pure helper (e.g. `fn mismatch_excluded_count(unfiltered: i64, filtered: i64) -> i64`, clamped at
  zero) that is unit-tested directly for its arithmetic (zero when they agree, the difference when
  they don't, zero rather than negative if ever called with `filtered > unfiltered`) rather than
  through the `error!`/`imetric!` side effects it drives.
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
