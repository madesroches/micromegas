# Stamp the write audience on `streams` and `blocks` Plan (#1518 — AbAC Stage 5b)

## Overview

`insert_stream` and `insert_block` accept any `process_id`/`stream_id` unconditionally: a credential
bound to audience A that discovers ids belonging to audience B can append events to B's process, and
those events inherit B's stamped audience. This is the Stage 5b integrity gap deferred out of #1373
(§7) and recorded as residual in `tasks/data_isolation/audience_based_access_control_plan.md` §11b.

The fix is to **record the caller's resolved write audience on the `streams` and `blocks` rows
themselves**, and to source the lakehouse `audience` column from the block's own stamp instead of
deriving it through a join to the owning process. An injected block then carries the *attacker's*
label, surfaces only to the attacker's readers, and never reaches the victim.

The original proposal on the issue was a write-side authorization gate that resolved the target's
owning audience from `processes` and rejected a mismatch. **That design does not work**: streams and
blocks routinely arrive before their process row exists, so there is frequently nothing to resolve
at the moment the decision must be made (see [Why not a resolve-and-compare
gate](#why-not-a-resolve-and-compare-gate)). Stamping needs no anchor, no cache, and no hot-path
resolution — the caller's audience is already in hand at the HTTP edge.

A gate does return, but in a different role: once the anchor rows carry an `audience` **column**, an
*opportunistic* check against whichever anchor happens to exist is cheap and closes the residual
metadata exposure that stamping alone leaves (Phase 3). It is a hardening layer on top of a design
that is already correct without it, not the mechanism the correctness rests on.

## Current State

### Where the audience lives

`processes.properties` (`micromegas_property[]`) is the single origin, stamped server-side at
registration from the authenticated credential (`insert_process` / `register_otel_process`,
`rust/ingestion/src/web_ingestion_service.rs`). `streams` and `blocks` carry no audience of their
own — DDL at `rust/ingestion/src/sql_telemetry_db.rs:52-66` and `:73-87`.

Three sites read it out of Postgres, all sharing `coalesced_audience_subselect`
(`rust/analytics/src/audience.rs:41-47`), which resolves a never-stamped row to
`MICROMEGAS_DEFAULT_AUDIENCE`:

| Site | File | Shape |
|---|---|---|
| `blocks` view materialization | `lakehouse/blocks_view.rs:71-82` | `COALESCE(<subselect over processes.properties>, $3) AS audience` |
| `metadata::find_process` | `metadata.rs:293-301` | same, bound as `$2` |
| Prong B `AudienceIndex` | `lakehouse/audience_guard.rs:168-204` | `LEFT JOIN LATERAL` per `IdKind` |

### How the audience reaches every view

`blocks_view`'s `data_sql` is the only place the audience enters the lakehouse. It joins all three
Postgres tables and derives the audience from the block's **claimed** `process_id`
(`blocks_view.rs:73-76`):

```sql
FROM blocks, streams, processes
WHERE blocks.stream_id = streams.stream_id
AND blocks.process_id = processes.process_id   -- audience comes from here
```

`streams.process_id` never enters the join. Downstream, `processes` and `streams` are
`SqlBatchView`s aggregating the `blocks` view:

- `processes_view.rs:25-45,49-69` — `GROUP BY process_id`, `arrow_cast(max(audience), …)`
- `streams_view.rs:26-39,42-55` — `GROUP BY stream_id`, `arrow_cast(max(audience), …)`

Today `max(audience)` is **degenerate**: every block of a process inherits that process's single
audience, so the group is uniform. Per-row stamping ends that guarantee — see
[Aggregation collapse](#aggregation-collapse-the-consequence-that-needs-handling).

`OwnershipRewrite` (Prong A) filters the six column-carrying views on `audience IN (...)` directly,
and falls back to a `process_id IN (subquery)` semi-join against
`Aggregate(GROUP BY process_id, MAX(audience))` for `net_spans`, `otel_spans`, `images`
(`ownership_rewrite.rs:36-57,159`).

### The write paths that need stamping

| Path | Function | Insert shape |
|---|---|---|
| Native stream | `web_ingestion_service.rs:422-480` `insert_stream` | explicit column list, 8 binds |
| Native block | `:298-400` `insert_block_typed` | **positional** `INSERT INTO blocks VALUES($1,…,$11)` |
| OTLP stream | `:482-515` `register_otel_stream` | explicit column list, 8 binds |
| OTLP block | `otel-ingestion/src/handler.rs:138` → `insert_block_typed` | same positional insert |
| Replication | `analytics/src/replication.rs:21-82` `ingest_streams` | explicit column list |
| Replication | `:187-235` `ingest_blocks` | **positional** `INSERT INTO blocks VALUES($1,…,$11)` |

Only `insert_process_request` reads auth today (`rust/public/src/servers/ingestion.rs:66-77`):

```rust
let audience = resolve_write_audience(ctx.as_ref(), service.default_audience());
```

`insert_stream_request` (`:81-88`) and `insert_block_request` (`:91-101`) do not take
`ctx: Option<Extension<AuthContext>>` at all, though the routes sit under the global
`auth_middleware` so the extension is present on the request.

### Second, unreported hole: the `(stream_id, process_id)` pair is never checked

`insert_block_typed` binds both ids verbatim from the client payload
(`web_ingestion_service.rs:340-343`). Nothing checks that the block's `process_id` is the one its
stream actually belongs to, and `blocks_view` derives the audience from the claimed `process_id`. A
caller in audience A can therefore send a block carrying their **own** `stream_id` and the
**victim's** `process_id`; the row lands and materializes under audience B. Any gate keyed on
`stream_id` alone would have been bypassable this way. The blob path
`blobs/{process_id}/{stream_id}/{block_id}` uses the claimed `process_id` too.

## Why not a resolve-and-compare gate

The issue's original proposal resolved the target's owning audience (`process_id → audience` for
streams, `stream_id → process_id → audience` for blocks) and rejected a mismatch, mirroring
`audience_guard.rs`. It presumes the process row exists when the stream or block is written. It
frequently does not, for two independent reasons:

1. **Concurrent in-flight requests.** The sink drains up to `max_in_flight_requests` concurrently
   (`Semaphore::new(config.max_in_flight_requests.max(1))`,
   `rust/telemetry-sink/src/http_event_sink.rs:775`) with per-item retry ladders.
   `UploadPriority::Metadata` (`:74-83`) orders the *enqueue*, not the completion — `insert_stream`
   can land while `insert_process` is still retrying.
2. **Retention.** Sweeps run bottom-up: `delete_expired_blocks` → `delete_empty_streams` →
   `delete_empty_processes` (`rust/analytics/src/delete.rs:152-166`). A long-lived process whose
   blocks have all aged out loses its row, and the next block for it arrives with no anchor.

Neither default is acceptable: fail-open leaves the gap exactly as it is today, and fail-closed
rejects ordinary first-blocks and every post-sweep block — data loss on the happy path. There is no
ordering guarantee to lean on.

The one-audience-per-process invariant still holds — it is an invariant of id derivation, not a
policy (see [Appendix](#appendix-one-audience-per-process-is-an-invariant)) — but it is about what is
*true*, not what is *knowable at write time*, and only the latter governs a write-path decision.

## Design

### 1. Resolve the caller's audience in both handlers

`rust/public/src/servers/ingestion.rs` — give `insert_stream_request` and `insert_block_request` the
same shape `insert_process_request` already has:

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

`resolve_write_audience` (`rust/public/src/servers/write_audience.rs`) is single-state since #1519:
a credential with no `bound_audience`, one whose label fails validation, and a deployment with no
auth provider at all all resolve to `service.default_audience()`. No third state reaches the
service, so the stamp is never absent on a freshly written row.

### 2. Schema v8 — an `audience` column on `streams` and `blocks`

`rust/ingestion/src/sql_migration.rs`, `upgrade_data_lake_schema_v8`, bumping
`LATEST_DATA_LAKE_SCHEMA_VERSION` from 7 to 8:

```sql
ALTER TABLE streams ADD COLUMN audience VARCHAR(255);
ALTER TABLE blocks  ADD COLUMN audience VARCHAR(255);
```

**Nullable, and no backfill.** A `NULL` means "written before this shipped, or written by the admin
replication path" — exactly the state the read side already resolves, via the same
`coalesced_audience_subselect` shape. Adding `NOT NULL` would force a rewrite of every existing
`blocks` row; adding a `DEFAULT` would silently relabel legacy rows to whatever the default was at
migration time instead of at read time. `ALTER TABLE ... ADD COLUMN` with no default is a catalog-only
operation in modern Postgres, so this is O(1) on a large `blocks` table.

No `CHECK` constraint: the charset is already enforced by `WriteAudience::new` on every value that
reaches the bind, and `ingestion_api_keys.audience` constrains the upstream source.

### 3. Bind the stamp on every write path

- `insert_stream(body, audience)` and `register_otel_stream(…, audience)` — one extra bind on the
  existing explicit column list.
- `insert_block_typed(block, audience)` — **give the insert an explicit column list**. It is
  currently positional (`INSERT INTO blocks VALUES($1,…,$11)`), which a twelfth column would break
  silently at the type level rather than loudly at the name level.
- `replication.rs::ingest_blocks` — same positional insert, same treatment. The replication path
  writes a source lake's rows verbatim and stamps nothing of its own (it is admin-only,
  `flight_sql_service_impl.rs:1282-1307`); it must carry the source's `audience` column through when
  present and write `NULL` when the source predates it, never substitute the local default.
- `replication.rs::ingest_streams` — same, on an explicit column list.

Per `rust/CLAUDE.md`'s Rust-API stance, `audience: &WriteAudience` is a **required** parameter on
each of these, with no `Default`, so the compiler enumerates every call site.

### 4. Source the lakehouse column from the block's own stamp

`blocks_view.rs`'s `data_sql` — replace the process-property derivation with a two-level coalesce:

```sql
COALESCE(blocks.audience, <subselect over processes.properties>, $3) AS audience
```

Reusing `audience_subselect` for the middle term and keeping `$3` as the bound deployment default.
Read as a rule: *the row's own stamp, else the owning process's stamp, else the deployment default.*

- Same column name, type (`Dictionary(Int32, Utf8)` after
  `metadata_partition_spec::cast_to_file_schema`), position (last), and non-nullability. **Not a SQL
  break.**
- **No backfill required**: for every row written before this ships, `blocks.audience` is `NULL` and
  the second term is exactly today's expression, so existing data reads identically.
- Bump `blocks_file_schema_hash()` (`blocks_view.rs:342`), `vec![5]` → `vec![6]`, to force a rebuild.
  Per `CLAUDE.md` an internal `SCHEMA_VERSION` bump is not a SQL break.

`metadata::find_process` and `audience_guard`'s `IdKind::Process` arm keep reading
`processes.properties` — a process's audience is still its own row's business.

`audience_guard`'s `IdKind::Block` arm simplifies from a two-hop join to a single-table read with the
same fallback:

```sql
SELECT b.block_id AS id, COALESCE(b.audience, a.value, $3) AS audience
FROM blocks b
LEFT JOIN processes p ON p.process_id = b.process_id
LEFT JOIN LATERAL (SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1) a ON TRUE
WHERE b.block_id = ANY($1::uuid[])
```

Note the `JOIN` → `LEFT JOIN`: a stamped block now resolves even when its process row is absent, which
is precisely the out-of-order and post-sweep case the old shape dropped to `Unknown` (deny). Same for
`IdKind::ProcessOrStream`'s `streams` arm against `streams.audience`.

### Aggregation collapse: the consequence that needs handling

`processes_view` and `streams_view` aggregate `max(audience)` over the `blocks` view grouped by
`process_id` / `stream_id`. That is safe today only because the group is uniform by construction.
Once blocks carry their own stamp, a process with an injected block has a **mixed** group, and
`max()` picks the lexicographically greater label. Two distinct failures follow, and the second is
strictly worse than the gap this plan closes:

1. `processes` / `streams` would relabel a victim's row to the attacker's audience (or the reverse),
   depending only on string ordering.
2. `OwnershipRewrite`'s semi-join for `net_spans`, `otel_spans`, `images` — the three views with no
   `audience` column of their own — resolves through that same collapsed aggregate. A victim process
   collapsing to the attacker's label would make **every one of the victim's rows in those three
   views readable by the attacker**: a read escalation, where today's gap is integrity-only.

Both are fixed by refusing to collapse:

- **`processes_view` / `streams_view`**: `GROUP BY process_id, audience` and `GROUP BY stream_id,
  audience`, dropping the `max()` wrapper (keep the `arrow_cast` for the declared dictionary type). A
  mixed id yields one row per audience, each labeled correctly, each carrying `first_value(...)`
  drawn only from its own audience's blocks. After Prong A filters `audience IN (...)`, every caller
  still sees at most one row per id — the duplicate exists only in the unfiltered view, and only for
  an id that is already the subject of an integrity violation.
- **`OwnershipRewrite`'s semi-join**: admit a `process_id` only when **every** audience present for
  it is in the caller's scope, rather than when *any* is — `process_id NOT IN (SELECT process_id
  FROM per_process_audience WHERE audience NOT IN (<caller audiences>))`. This is the same
  fail-closed rule `audience_guard`'s `OwnerAudience::Ambiguous` already applies to a
  `process_id`/`stream_id` collision: readable only when every interpretation is independently
  readable, never by picking one arm. `per_process_audience` becomes a distinct
  `(process_id, audience)` projection instead of a `MAX` aggregate.

The clean end state is giving those three views their own `audience` column, the way #1482 did for
the other six — see [Out of scope](#out-of-scope--follow-ups).

### 5. Phase 3 hardening: the opportunistic gate

Stamping alone leaves one residual. A block injected under audience A against victim process P
materializes in the `blocks` view labeled A, but its `processes.*` columns are joined from P's row —
so the attacker sees P's `exe`, `username`, `computer`, `distro`, `cpu_brand` under their own label.
That is a small, new confidentiality exposure where today the injected row simply became B's.

Close it with a gate that is now cheap, because the anchor rows carry a column instead of a property
array: **when an anchor row exists and its audience differs from the caller's, reject; when no anchor
exists, accept and stamp.**

- `insert_block`: anchor is `streams.audience` for the block's `stream_id`.
- `insert_stream`: anchor is the resolved audience of the row's `process_id`.

Fail-open on absence is safe here in a way it was not for the original gate, because absence
correlates with *"this is your own brand-new stream"*, not with *"this is someone else's"*. An
attacker targeting an existing victim stream hits the present-anchor case and is rejected. Slipping
through requires predicting and racing a stream id before its owner registers it — and even then the
block is still stamped A and still never reaches B, because stamping, not the gate, is what carries
correctness.

Cache keyed on `stream_id` — not `block_id` — in the same TTL'd `moka` shape as the existing
`process_audience_cache` (`web_ingestion_service.rs`, `rust/ingestion/Cargo.toml:20`). Blocks per
stream is large, so this amortizes to roughly one point query per stream per TTL rather than one per
block. A resolution *error* (Postgres unreachable) must not become a denial on the ingestion path the
way it does on the read path — ingestion availability outranks this hardening layer; log and accept.

### 6. The `(stream_id, process_id)` pair check

Reject a block whose `process_id` disagrees with its stream's `process_id`. With per-row stamping
this is no longer security-critical — the block's own stamp governs its label regardless of what
`process_id` it claims — so it is a plain data-integrity check. Land it as `warn!` + counter first
and measure the real mismatch rate before flipping it to a hard reject; per the invariant it should
be zero. Free once Phase 3 is resolving the stream row anyway.

### 7. Precedence rule, stated once

Two anchors now exist. Document in `rust/analytics/src/audience.rs`, beside
`coalesced_audience_subselect`: **a row's own stamp wins**, because it is the authenticated fact at
write time; the process's stamp governs the process row only and serves as the fallback for rows
written before this shipped. `check_process_audience_conflict` is unaffected — it compares a
re-registration against the existing *process* row and stays exactly as it is.

## Implementation Steps

### Phase 1 — Write path (stamp)

1. `rust/ingestion/src/sql_migration.rs`: add `upgrade_data_lake_schema_v8` (two `ALTER TABLE ... ADD
   COLUMN audience VARCHAR(255)`), the `if 7 == current_version` arm in `migrate_db`, and bump
   `LATEST_DATA_LAKE_SCHEMA_VERSION` to 8. Doc-comment it in the style of v7.
2. `rust/ingestion/src/sql_telemetry_db.rs`: add the column to the fresh-install `create_streams_table`
   / `create_blocks_table` DDL so a new lake and a migrated lake converge.
3. `rust/ingestion/src/web_ingestion_service.rs`: add `audience: &WriteAudience` to `insert_stream`,
   `insert_block`, `insert_block_typed`, and `register_otel_stream`; bind it. Convert
   `insert_block_typed`'s positional `INSERT INTO blocks VALUES(...)` to an explicit column list.
4. `rust/public/src/servers/ingestion.rs`: thread `ctx: Option<Extension<AuthContext>>` into
   `insert_stream_request` and `insert_block_request`; call `resolve_write_audience`.
5. `rust/otel-ingestion/src/handler.rs` (and `cloudwatch_logs.rs`): pass the already-resolved
   `audience` — these paths hold a `WriteAudience` for `id_namespace` at `handler.rs:161,185,220,318`
   and `cloudwatch_logs.rs:223`, so nothing new needs resolving.
6. `rust/analytics/src/replication.rs`: carry the source lake's `audience` column through
   `ingest_streams` and `ingest_blocks`, writing `NULL` when the source batch has no such column.
   Convert `ingest_blocks`' positional insert to an explicit column list.
7. Remove the known-gap doc comments on `insert_block_typed` (`:287-296`) and `insert_stream`
   (`:417-421`).

### Phase 2 — Read path (source the column, stop collapsing)

8. `rust/analytics/src/audience.rs`: add the two-level coalesce helper and the precedence-rule doc
   comment (§7).
9. `rust/analytics/src/lakehouse/blocks_view.rs`: use it in `data_sql`; bump
   `blocks_file_schema_hash()` to `vec![6]`.
10. `rust/analytics/src/lakehouse/processes_view.rs` and `streams_view.rs`: `GROUP BY <id>, audience`
    in both the transform and merge queries; drop `max(audience)`, keep `arrow_cast`.
11. `rust/analytics/src/lakehouse/ownership_rewrite.rs`: rebuild `per_process_audience` as a distinct
    `(process_id, audience)` projection and invert the semi-join to the fail-closed `NOT IN … WHERE
    audience NOT IN (…)` shape. Update the module table at `:36-57` and the doc comment at `:159`.
12. `rust/analytics/src/lakehouse/audience_guard.rs`: `IdKind::Block` and the `streams` arm of
    `IdKind::ProcessOrStream` read the stamped column with the coalesce fallback; `JOIN processes` →
    `LEFT JOIN processes`. Update the "Fail-closed" module comment — a stamped block or stream now
    resolves without its process row.
13. Regenerate the six global views over the retention window (`regenerate_partitions`, Maintenance
    role) — required by the file-schema bump, same as #1482.

### Phase 3 — Opportunistic gate + pair check (hardening)

14. `web_ingestion_service.rs`: add a `stream_audience_cache: Cache<Uuid, (Uuid, WriteAudience)>`
    (stream → its process_id and audience) alongside `process_audience_cache`, in the same TTL'd
    `moka` shape.
15. Gate `insert_block` on the stream anchor and `insert_stream` on the process anchor: reject a
    present-and-different audience with `IngestionServiceError::AudienceConflict`, accept an absent
    anchor, and accept-with-a-log on a resolution error.
16. Add the `(stream_id, process_id)` pair check as `warn!` + `imetric!` counter, reusing the stream
    lookup from step 14. Do not reject yet.
17. `rust/public/src/servers/ingestion.rs`: the existing `AudienceConflict → IngestionError::Forbidden`
    arm (`:55-57`) already produces the sanitized 403, so the handlers need no new error mapping.

### Phase 4 — Docs and changelog

18. `mkdocs/docs/admin/authentication.md:303-313`: delete the "Residual gap: cross-audience write
    injection" warning admonition's first paragraph and replace it with a description of the stamped
    column and the precedence rule. Keep the process-squatting paragraphs below it — they describe a
    different, already-closed gap.
19. `tasks/data_isolation/audience_based_access_control_plan.md:1370-1379` (§11b): mark landed,
    replacing the "write-side authorization gate sharing Stage 3's cache layer" description with what
    was actually built.
20. `CHANGELOG.md` under `## Unreleased`, an **Ingestion** and an **Analytics** entry. Include the
    **Minor breaking change** clause: `WebIngestionService::insert_stream`, `insert_block`,
    `insert_block_typed`, and `register_otel_stream` (published,
    `micromegas_ingestion::web_ingestion_service`) each gain a required `&WriteAudience` parameter.
    Include an **Upgrade note** for the file-schema bump and the required regeneration pass.
21. Fold the Phase 3 gate back into #1518's body — the issue as it stands describes stamping only.

## Files to Modify

| File | Change |
|---|---|
| `rust/ingestion/src/sql_migration.rs` | v8 migration, version bump |
| `rust/ingestion/src/sql_telemetry_db.rs` | fresh-install DDL for both columns |
| `rust/ingestion/src/web_ingestion_service.rs` | audience params + binds, explicit column list, stream cache, gate, pair check, drop gap comments |
| `rust/public/src/servers/ingestion.rs` | `AuthContext` into both handlers |
| `rust/otel-ingestion/src/handler.rs`, `cloudwatch_logs.rs` | pass the resolved audience |
| `rust/analytics/src/replication.rs` | carry the column through both ingest paths |
| `rust/analytics/src/audience.rs` | two-level coalesce helper, precedence doc |
| `rust/analytics/src/lakehouse/blocks_view.rs` | `data_sql` source, schema-hash bump |
| `rust/analytics/src/lakehouse/processes_view.rs`, `streams_view.rs` | group by `(id, audience)` |
| `rust/analytics/src/lakehouse/ownership_rewrite.rs` | fail-closed semi-join |
| `rust/analytics/src/lakehouse/audience_guard.rs` | stamped-column resolution, `LEFT JOIN` |
| `mkdocs/docs/admin/authentication.md` | replace the residual-gap admonition |
| `tasks/data_isolation/audience_based_access_control_plan.md` | §11b landed |
| `CHANGELOG.md` | Ingestion + Analytics entries |

## Trade-offs

**Stamping vs. the resolve-and-compare gate.** The gate cannot produce a correct default under
out-of-order arrival, as shown above. Stamping also happens to be the smaller change: no cache, no
TTL, no fail-closed policy, and no hot-path database read in Phase 1–2 at all, which retires §7's
deferred performance question rather than answering it.

**Nullable column with a read-side coalesce vs. `NOT NULL` + backfill.** #1482's addendum already
settled this exact question the same way for `processes` and reverted an earlier write-side backfill:
resolving where the audience is *read* keeps legacy and replicated rows correct without rewriting
them, and keeps the materialized Arrow column non-nullable regardless.

**`GROUP BY (id, audience)` vs. keeping `max()`.** Keeping `max()` is a one-line non-change but turns
an integrity gap into a read escalation for the three column-less views. Grouping produces a
duplicate row per mixed id in the unfiltered view — acceptable, because a mixed id only exists under
an integrity violation and every caller still sees one row after Prong A's filter.

**Dropping mismatched rows in `blocks_view` instead.** A `WHERE blocks.audience = <process audience>`
filter would make injected blocks invisible to everyone rather than isolating them to the attacker.
Rejected: silent row-dropping at materialization time is invisible to operators and would mask
ordinary bugs, and the maintenance path has no good place to log it per-row. Isolation plus a denial
counter is more debuggable.

**Phase 3 as a separate phase.** It is genuinely optional for the integrity fix, and it reintroduces
a (cached, amortized) hot-path query. It is recommended rather than optional only because stamping
alone opens the small process-metadata exposure described in §5, which did not exist before.

## Security

- **What closes**: cross-audience write injection no longer reaches the victim's readers. An injected
  stream or block carries the writer's own label at every downstream view.
- **What does not change**: no read escalation is introduced or removed for existing data; reading B
  still requires a read grant on B. Process registration's conflict guard is untouched.
- **New exposure, mitigated**: the `processes.*` columns of an injected block row are the victim's.
  Phase 3 reduces this to the case where the attacker predicted and raced an unregistered
  `stream_id`.
- **Existence oracle**: Phase 3's rejection reuses `AudienceConflict`'s sanitized 403
  (`rust/otel-ingestion/src/error.rs:115-117`), matching the reasoning `audience_guard.rs` documents
  under "No existence oracle".
- **Denial signal**: every rejection is a bug or an attack — there is no benign-mismatch class — so
  `warn!` plus an `imetric!` counter is the right level, and the counter's healthy baseline is a flat
  zero.

## Performance

- Phases 1–2 add **no** database round trip to the ingestion path: one extra bind per insert and one
  extra `VARCHAR` per row on `streams` and `blocks`. Postgres-side the cost is transient (retention
  sweeps both tables); lakehouse-side the column is dictionary-encoded, one distinct value per
  partition in practice.
- `ALTER TABLE ... ADD COLUMN` with no default and no `NOT NULL` is catalog-only — no table rewrite,
  so the migration is O(1) even on a large `blocks` table.
- Phase 3 adds one cached point query per **stream** per TTL, not per block. Measure cold-miss
  latency and the added p99 on `insert_block` before and after, as §7 asked; the `stream_id` key is
  what keeps this off the per-block path.
- `audience_guard`'s `IdKind::Block` resolution loses a join.
- `ownership_rewrite`'s semi-join for the three column-less views changes from `IN` to `NOT IN` over
  a distinct projection instead of an aggregate — one less `Aggregate` node, an anti-join instead of
  a semi-join.

## Testing Strategy

- **Unit, no DB**: `resolve_write_audience` already covered; add the two-level coalesce helper's SQL
  shape and the precedence rule (row stamp > process stamp > default) as string/expression tests
  beside the existing `audience.rs` tests.
- **Migration**: v7 → v8 on a populated lake leaves every existing row's resolved audience unchanged
  (the coalesce fallback is today's expression) and both columns present and `NULL`.
- **Write path, DB-backed** (`rust/ingestion/tests/`, beside `write_audience_tests.rs`): a stream and
  a block written under audience A land with `audience = 'A'`; an unaudienced caller lands with the
  deployment default; a client-supplied audience property on a stream is not trusted.
- **Isolation, end-to-end**: register process P under B; write a block under A carrying P's
  `process_id`; assert a `ReadScope::Audiences(["B"])` session sees zero rows for it in
  `blocks`/`log_entries`, and an `["A"]` session sees it. This is the regression test for the gap
  itself.
- **Aggregation**: a process with blocks under two audiences yields two `processes` rows, one per
  audience, each with its own `first_value` columns — and a `["B"]` caller sees exactly one.
- **Escalation regression**: same mixed process, assert a `["A"]` caller sees **zero** of the
  victim's `net_spans` rows (the fail-closed semi-join).
- **Phase 3**: block against an existing foreign-audience stream → 403 with the sanitized text; block
  against an absent stream → accepted and stamped; mismatched `(stream_id, process_id)` → accepted
  with the counter incremented.
- Keep the tests proportionate — assert on returned rows and audiences, not on elaborate
  side-effect queries.

## Migration and Deployment

- Roll the ingestion role first: v8 is additive and nullable, so an older analytics binary reading a
  migrated lake simply never sees the column.
- Then the analytics/maintenance roles, then `regenerate_partitions` over the retention window for
  the six global views. Until regeneration completes, un-regenerated partitions are invisible
  (fail-closed, never fail-open) — the same window #1482 documented and accepted.
- No client change and no re-instrumentation: the stamp is server-side.

## Out of scope / follow-ups

- **`audience` column on `net_spans`, `otel_spans`, `images`.** #1482 left these three out, which is
  why they still resolve through a per-process semi-join and why the fail-closed rule in step 11 is
  needed at all. Giving them their own column removes the semi-join entirely and makes mixed
  processes a non-event.
- **Relaxing `blocks_view`'s inner join.** Once blocks are self-describing, the
  `blocks ⋈ streams ⋈ processes` join is what still hides early-arriving and post-sweep blocks from
  every view. Relaxing it becomes possible and is worth its own issue.
- **Flipping the pair check to a hard reject**, once the counter has run long enough to confirm zero.

## Open Questions

1. **Is Phase 3 in scope for this issue, or its own?** It is the difference between "the gap is
   closed" and "the gap is closed and the residual metadata exposure is closed too". It also
   contradicts the issue body as currently written, which describes stamping only — step 21 assumes
   we fold it in.
2. **Should the pair check ship as a hard reject immediately?** Per the invariant the mismatch rate
   should be zero; a query against a live lake would settle it before implementation starts and skip
   the staged rollout.
3. **Does the replication path need an audience of its own?** This plan carries the source's column
   through verbatim and writes `NULL` when absent, matching how it already treats
   `processes.properties`. An alternative is stamping replicated rows with the destination
   deployment's default — a behaviour change for `processes` too, and out of scope here.

## Appendix: one-audience-per-process is an invariant

Every legitimate producer ends up with a distinct `process_id`, by construction rather than by
convention:

- **OTLP**: `process_id_from_resource` salts the namespace per audience —
  `Uuid::new_v5(&NS_OTEL_PROCESS_V1, audience)` becomes the namespace before the resource key is
  hashed (`rust/otel-ingestion/src/identity.rs:270-276`). Two audiences submitting byte-identical
  resource attributes derive *different* `process_id`s. An audience equal to the deployment default
  keeps the un-salted namespace, so resolved-to-default and explicitly-bound-to-default agree — the
  only collapse that is wanted (`WriteAudience::id_namespace`,
  `rust/ingestion/src/write_audience.rs`).
- **Native**: `process_id` is client-generated, so a collision is a deliberate act.

This is why there is no legitimate cross-audience write to a shared process to design for, why a
mixed-audience group is always an integrity violation rather than a case to support, and why the pair
check's expected mismatch count is zero.
