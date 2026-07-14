# Partition `blocks` by `insert_time` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1245
**Sub-issues**: #1240 (schema foundation), #1241 (ingestion forward-provisioning), #1242 (daemon roll-off), #1243 (monitoring)

## Overview

The Postgres metadata store runs on Aurora Serverless v2. Today retention deletes old `blocks`
rows with batched row `DELETE`s (`delete_expired_blocks_batch`), which generate dead tuples,
follow-up autovacuum, and WAL — spiking CPU. Aurora scales up fast but down slowly, so a short
cleanup burst leaves a long, expensive ACU tail.

This plan converts `blocks` to a `PARTITION BY RANGE (insert_time)` table with **hourly UTC**
partitions, so retention becomes a partition `DROP` (an O(1) catalog operation) instead of row
deletes — no dead tuples, no autovacuum, no WAL storm, nothing for Aurora to scale up for. The
ingestion service rolls the window **forward** (provisioning future partitions ahead of need); the
daemon rolls it **off** (enumerate blob keys → delete blobs → `DROP TABLE`). A `DEFAULT` partition
is a safety net during rollout and against buffer starvation.

Global `block_id` dedup — which a partitioned table cannot enforce with a unique index — moves to
the **object store**: the payload blob's key is already deterministic per block, so an atomic
conditional PUT (`PutMode::Create`) makes the blob itself the global uniqueness arbiter. Retention
stays 100% drop-based; there is **no residual row-DELETE churn anywhere**.

Partitioning on `insert_time` is safe because `insert_time` is the ingestion server's `Utc::now()`
at insert (`web_ingestion_service.rs:177`), never a client timestamp — it only moves forward, so a
partition whose window is fully in the past is immutable and safe to read-then-drop. It is also
well-suited because the dominant Postgres read of `blocks`, the global `BlocksView` materialization
(`blocks_view.rs:44-45`, `78-85`), already filters `insert_time >= $1 AND insert_time < $2`, so
hourly partitions prune cleanly. Heavy event-time / `process_id` / `stream_id` analytics run on
DataFusion/parquet, not on this table.

## Current State

### Schema (`rust/ingestion/src/sql_telemetry_db.rs:70-91`)
`blocks` is a plain table with columns `block_id, stream_id, process_id, begin_time, begin_ticks,
end_time, end_ticks, nb_objects, object_offset, payload_size` (created v1), plus `insert_time
TIMESTAMPTZ` (added in v2, `sql_migration.rs:34`). Indexes: `blocks_block_id_unique` (unique, on
`block_id`, created in the v2→v3 migration via `CREATE UNIQUE INDEX CONCURRENTLY`,
`sql_migration.rs:179`), `block_stream_id`, `block_begin_time`, `block_end_time`,
`block_insert_time`. Current schema version `LATEST_DATA_LAKE_SCHEMA_VERSION = 4`
(`sql_migration.rs:8`).

### Migration mechanism (`rust/ingestion/src/sql_migration.rs`, `remote_data_lake.rs:22-42`)
`execute_migration` steps versions forward one at a time inside transactions;
`migrate_db` wraps it in advisory lock key `0` (`acquire_lock`, `remote_data_lake.rs:29`) so only
one instance migrates; `migrate_db` is itself invoked by `connect_to_remote_data_lake`
(`remote_data_lake.rs:60`), the actual data-lake migration entry point. `CREATE UNIQUE INDEX
CONCURRENTLY` runs outside any transaction (it cannot run inside one). Callers: ingestion/flight-sql
and the monolith (`monolith/src/main.rs:~180`, gated by `roles.needs_lakehouse()`) both go through
`connect_to_remote_data_lake`; a separate, unrelated app-db migration runs in `analytics-web-srv`'s
`seed_local_data_source` (`monolith/src/main.rs:343` calls `app_db::execute_migration`, a
different function of the same name, run only when the web and flightsql roles are both enabled
and seeding is enabled — i.e. `--no-seed-data-source` is not set (`monolith/src/main.rs:289,
298-303`)).

### Insert path (`rust/ingestion/src/web_ingestion_service.rs:142-214`)
`insert_block_typed` puts the payload blob at `blobs/{process_id}/{stream_id}/{block_id}`
(**deterministic key, no time component**) via an unconditional `BlobStorage::put`
(`blob_storage.rs:75`, overwrite semantics), sets `insert_time = Utc::now()` (`:177`), then:
```sql
INSERT INTO blocks VALUES($1..$11) ON CONFLICT (block_id) DO NOTHING;
```
(`:178-179`). The `ON CONFLICT (block_id)` dedups client retries of the same block. Note the
ordering: **blob first, then metadata** — the blob is written before the row exists.

### Replication path (`rust/analytics/src/replication.rs:144-219`)
Replication (expert feature) is driven by `bulk_ingest`, which dispatches per Arrow Flight
`table_name` to a table-specific function (`:222-234`) — `"payloads"` and `"blocks"` are **separate
calls**, with no shared transaction or per-block interleaving between them. `ingest_payloads`
(`:144-170`) puts each payload blob at the same deterministic key (`:160-165`) via an unconditional
`put`, in its own function. Separately, `ingest_blocks` (`:172-219`) opens **one transaction for the
whole batch** (`begin()` at `:176`, `commit()` at `:216`) and, inside it, loops over every block in
the stream running `INSERT INTO blocks VALUES(...) ON CONFLICT (block_id) DO NOTHING` (`:197`) per
row — unlike `insert_block_typed`'s per-block autocommit, this is one batch-wide transaction — and
binds `insert_time` **from the source data** (`:208-210`), not `Utc::now()`.

### Retention path (`rust/analytics/src/delete.rs`)
`delete_old_data` (called hourly by `EveryHourTask::run`, `rust/public/src/servers/maintenance.rs:102`) computes
`expiration = now - retention_days` and calls, in order:
`delete_expired_blocks` → `delete_empty_streams` → `delete_empty_processes` →
`retire_expired_partitions`. `delete_expired_blocks_batch` (`delete.rs:13-48`) loops:
```sql
DELETE FROM blocks WHERE block_id IN
  (SELECT block_id FROM blocks WHERE insert_time <= $1 LIMIT 1000)
RETURNING process_id, stream_id, block_id;
```
then deletes the returned blobs via `lake.blob_storage.delete_batch`. This is the DELETE/vacuum
churn we are eliminating. `delete_empty_streams`/`delete_empty_processes` are low-volume
conditional deletes with `NOT EXISTS` guards — **unchanged** by this plan.

### Ingestion server lifecycle (`rust/telemetry-ingestion-srv/src/main.rs`, `rust/public/src/servers/ingestion.rs:108`)
`serve_ingestion` runs the HTTP server until SIGTERM. There is no existing background task in the
ingestion service — the forward-provisioner is new. The `ingestion` crate is a dependency of
`analytics` (e.g. `delete.rs` imports `micromegas_ingestion::data_lake_connection`), so a shared
helper placed in `ingestion` is callable from both the ingestion service and the analytics daemon.

### Object store capabilities
The workspace's `object_store` dependency is compiled with only the `aws` feature
(`object_store = { version = "0.13", features = ["aws"] }`, `rust/Cargo.toml:66`) — GCS and Azure
support are not built into this workspace. Storage is constructed via
`parse_object_store_url`/`parse_url_opts` from `MICROMEGAS_OBJECT_STORE_URI`, so the only
backends this codebase can target are S3 (the compiled `aws` feature) and the local filesystem
backend (always available, no feature flag). Both support `put_opts(..., PutMode::Create)` — an
atomic create-if-absent that maps to `If-None-Match: *` on S3 (supported by AWS since Nov 2024) and
is natively supported on local FS. The object-cache layers forward `put_opts` straight through to
the origin store (`object-cache/src/client.rs:447`, `object-cache/src/l1_store.rs:167`), so
conditional PUTs work through the full ingestion stack. Conditional PUT is therefore safe for
every in-repo deployment target; the only residual caveat is an externally-operated, older
S3-compatible store (e.g. old MinIO, which `local_test_env/ai_scripts/start_minio.py` uses only as
a local-test S3 origin, never a production target) lacking `If-None-Match: *` — a deployment-time
check, not a code gap (see "Cost — honest accounting" below). This also depends on the S3 store
keeping `object_store`'s default conditional-put mode (`S3ConditionalPut::ETagMatch`): an
`aws_conditional_put=disabled` override in `MICROMEGAS_OBJECT_STORE_URI`'s options would make
`put_if_absent` return `Error::NotImplemented` on S3.

## Critical Design Decisions

Three constraints are under-specified in the sub-issues and must be nailed down before
implementation, because they reshape the schema, both insert call sites, and the daemon.

### 1. Global `block_id` dedup moves to the object store — the blob is the uniqueness arbiter

Today `ON CONFLICT (block_id) DO NOTHING` (`web_ingestion_service.rs:179`, `replication.rs:197`)
gives **global** dedup: a resend of an already-ingested block is a no-op no matter how much later it
arrives. This is load-bearing — the sink's upload queue is an in-memory `VecDeque`
(`http_event_sink.rs:155`) with capped retries (`ExponentialBackoff::from_millis(10).take(N)`,
`:347-350`), and a single retry after a lost ack is enough to re-send a block. The gap between the
original and the resend is **unbounded**:

- A laptop **sleeps** with an in-flight block; the process survives the sleep and the sink resends on
  wake, hours or days later.
- A **local telemetry proxy** (a planned feature) buffers/persists telemetry and forwards it **days**
  later, and may re-forward on its own retry.

We cannot bound this from the client side: instrumented app clocks drift arbitrarily, so there is no
trustworthy client timestamp to reject "too-old" blocks by. `insert_time` must stay **server-assigned
at the final ingestion hop**, and dedup must be **global across the whole retention window**.

Postgres cannot provide both a global unique index on `block_id` **and** partition-drop cleanup on
the same table — a unique index on a partitioned table must include the partition key
(`insert_time`), and `insert_time` is regenerated per attempt, so folding it in defeats dedup.
Options that were ruled out:

- **Composite `UNIQUE (block_id, insert_time)` on the parent** — allowed, but two attempts get
  different `insert_time`s and never conflict. No dedup.
- **Per-partition local `UNIQUE (block_id)`** — only dedups *within one partition*; the laptop/proxy
  resend lands in a different partition and duplicates. `BlocksView` would materialize the block twice
  → double-counted logs/metrics/spans downstream. **Widening the partition or bounding the resend
  window does not fix this**: for any partition width and any window, a resend can straddle a
  boundary (original at `boundary − ε`, resend at `boundary + δ`) and land the two copies in adjacent
  partitions. No partition-local scheme can be correct — correct dedup must be partition-independent.
  Rejected (kept only as a belt-and-suspenders guard, below).
- **Emulating a cross-partition global unique index** (trigger tricks, extensions) — there is no
  index-level hack: cross-partition uniqueness is enforced by the executor requiring the partition
  key, not by anything a userland index can bypass. No Aurora-available extension supplies it either.
- **Deterministic partition routing** (embed a client-side timestamp in `block_id`, UUIDv7-style, and
  partition the dedup key on it so retries always route to the same partition) — trusts client
  clocks; drift beyond any chosen slack silently breaks dedup. Rejected.
- **A separate unpartitioned dedup table** (`block_ids (block_id PK, insert_time)`, upsert-gated
  inserts) — correct, and the arbiter shares Postgres's transactional domain with the data, which
  makes idempotency trivial (gate + insert in one transaction). But it reintroduces the very thing
  this plan eliminates: a per-block row INSERT plus a recurring batched row DELETE (with dead tuples,
  vacuum, and WAL) on every retention pass, forever — plus a one-time backfill sized to the live
  `blocks` row count. Rejected in favor of the object store, which provides the same global
  atomicity with zero additional Postgres writes. This is the fallback design if conditional PUT
  turns out to be unavailable on a required deployment target (see Open Question #3).

**Chosen: the payload blob is the global dedup record; a conditional PUT is the arbiter.**

The key insight: the blob at `blobs/{process_id}/{stream_id}/{block_id}` — a key that is
deterministic across retries and contains no timestamp — is already written before its metadata row
lands in `blocks`. On the ingestion path (`insert_block_typed`) this happens inside one call, blob
then row, per block. On the replication path it happens across **two separate `bulk_ingest` calls**
(`ingest_payloads` writes blobs; a later `"blocks"` call's `ingest_blocks` inserts metadata for a
whole batch in one transaction, `replication.rs:144-219`) — the ordering still holds because
callers send the payloads stream before the corresponding blocks stream, but the two writes are not
tied together the way they are in `insert_block_typed` (see "Insert paths" below for how the
gated-insert flow accounts for this). And the retention daemon *already* deletes blobs when their
metadata expires, so the blob's lifetime is exactly the documented dedup window ("dedup holds within
the retention window"). The blob is a global, deterministic, retention-scoped record of every
ingested block. The only missing ingredient is atomicity, and `PutMode::Create` supplies it.

The insert operation becomes "**ensure blob, then ensure metadata row**" — every arm converges to
the same end state regardless of where a previous attempt died:

```
put_opts(key, payload, PutMode::Create)
├─ Ok(created) ────────────► tx {
│                              pg_advisory_xact_lock(BLOCKS_LOCK_CLASS, hash32(block_id));
│                              probe: SELECT EXISTS(SELECT 1 FROM blocks
│                                     WHERE block_id=$1 AND insert_time > '<now-1h>'::ts);
│                              if missing: INSERT (ON CONFLICT DO NOTHING);
│                            }   -- hot path; literal bound → plan-time prune to ≤2 partitions
└─ Err(AlreadyExists) ─────► tx {
                               pg_advisory_xact_lock(BLOCKS_LOCK_CLASS, hash32(block_id));
                               staged probe (bounds are inlined literals, not now()/params —
                                 see "Bounding the recovery probe"):
                                 1. block_id=$1 AND insert_time > '<now-25h>'::ts
                                 2. miss → HEAD blob → block_id=$1
                                           AND insert_time >= '<last_modified-slack>'::ts
                               if found: skip (true duplicate — previous ingest COMPLETED);
                               if missing: INSERT (recovers a prior attempt that died
                                           between PUT and INSERT);
                             }   -- recovery path; rare
```

**`AlreadyExists` is a recovery path, not a skip path.** The S3 response alone never decides
whether to skip — it only means "an ingest of this block was *started* at some point." Postgres is
the sole source of truth for whether it *completed*. Skipping is only ever the outcome of finding
the row in Postgres. This is what makes the insert idempotent under client retry:

1. Attempt dies **before the PUT** → retry redoes everything from scratch.
2. **PUT succeeds, PG insert fails** (transient DB error, crash) → client retries →
   `AlreadyExists` → global probe finds no row → this retry performs the INSERT. If PG is still
   down, the client gets another 5xx and retries again — each retry re-enters the same recovery arm
   until one succeeds. No state ever gets stuck.
3. **Everything succeeds, ack lost** → retry → `AlreadyExists` → probe finds the row → no-op ack.

**Why the hot path also carries the lock+probe:** a client-timeout retry can arrive while the
*original* request is still in flight server-side. Attempt A gets `Created`; concurrent attempt B
gets `AlreadyExists`, probes before A commits, sees nothing, and inserts. If A then inserted
blindly, the two rows could land in adjacent hourly partitions (the boundary-straddle case) where
per-partition `ON CONFLICT` cannot catch them. The shared advisory xact lock serializes the two
probe+insert pairs, so the loser always sees the winner's committed row. The hot-path probe is
cheap: any competitor's row was inserted seconds ago, so a `insert_time > '<now - 1h>'::timestamptz`
lower bound prunes it to at most two partitions. As in the recovery arm, this bound is **inlined as a
literal `timestamptz` constant** (server-computed `now() - 1h`), not `now()` or a bind parameter, so
the pruning — and therefore the partition locking — happens at *plan time*; a bare `now() - '1 hour'`
is a stable expression that only prunes at executor-startup runtime, which would leave the hot path
holding an `AccessShareLock` on all ~2,160 partitions on every ingest.

**Bounding the recovery probe — the create-once blob yields a correct time bound.** A naive
unpredicated probe (`WHERE block_id = $1` alone) cannot prune and is the worst query in the design:
at 90-day × hourly = ~2,160 partitions it means enumerating every partition at plan time, taking an
`AccessShareLock` on every partition *and* its block_id index (4,000+ locks per query — far past
the per-backend fast-path slots, so a retry storm running concurrent recovery probes would contend
on the shared lock manager), and executing ~2,160 index descents (20–100ms) to prove "not found".
The invariant from the hard rule below eliminates this: metadata is only inserted while its blob
exists, and the blob is create-once — never overwritten, never recreated within the retention
window — so `last_modified` *is* the original creation time, and **no `blocks` row for this block
can have `insert_time` earlier than the blob's creation time** (minus object-store↔Postgres clock
skew). The recovery probe is therefore staged:

1. Probe `block_id = $1 AND insert_time > '<now - 25h>'::timestamptz` (~25 partitions, sub-ms) —
   again a plan-time literal constant, not `now()`, so the ~25 surviving partitions are the only ones
   locked. Found → duplicate, done. Covers the common duplicate class (same-day retries, laptop wakes)
   with no object-store round trip.
2. Miss → `HEAD` the blob (~10–20ms, rare arm only; **must bypass any configured object cache and
   read the origin store directly** — see "Conditional blob PUT" below) → probe
   `block_id = $1 AND insert_time >= '<blob_last_modified - slack>'::timestamptz` (slack ≈ 1h,
   generous for AWS-to-AWS skew). **The lower bound is inlined into the SQL as a literal
   `timestamptz` constant — not a bind parameter — so Postgres prunes partitions at *plan time*.**
   Plan-time pruning drops the below-bound partitions from the plan entirely, so they are neither
   scanned nor **locked**; lock count and probe count scale with the *surviving* partitions, not all
   2,160. This is what makes hourly partitions safe here without falling back to a coarser scheme:
   plan-time pruning is deterministic Postgres behavior for a literal constant and does **not** depend
   on executor-startup runtime pruning (which reduces scans but leaves the plan-time locks on every
   surviving partition — the murky, version-dependent behavior we deliberately avoid relying on). The
   inlined value is a server-computed timestamp derived from object-store metadata, never
   client-supplied text, so there is no injection surface; format it with a fixed `timestamptz`
   rendering.
3. Miss → the block was never fully ingested → INSERT (still under the advisory lock).

Per-case cost (all figures are both the scan *and* the plan-time lock count, since the literal bound
prunes them together): **crash recovery** (blob minutes old, needs a fast "not found") prunes to 1–2
partitions — near-free; **recent duplicate** resolves in stage 1; **old duplicate** (proxy
re-forwarding a weeks-old block) prunes to the [blob age → now] partitions and `EXISTS` exits early at
the row, which sits near the start of that range. A weeks-old old-duplicate therefore locks a few
hundred partitions for the duration of one probe — acceptable because the recovery arm is rare
(stage 1 already absorbed every same-day retry) and each such lock is a cheap `AccessShareLock`. The
only case that touches all ~2,160 partitions is a full-range *not-found* scan, which requires a blob
at the 90-day retention edge whose row vanished — essentially never, and still correct (just slower)
when it happens. That pathological one-off is not worth permanently coarsening the scheme to daily
partitions; hourly partitions stay the design's choice.

Correctness caveat: the bound must be a **lower bound only** (`insert_time >= blob_time - slack`),
never a window *around* blob time — a row inserted by the recovery arm can carry an `insert_time`
days after blob creation (the recovering retry stamps its own arrival time).

**Correctness caveat 2: the HEAD must read the origin's real `last_modified`, never the object
cache's synthesized one.** When `MICROMEGAS_OBJECT_CACHE_URL`/`MICROMEGAS_OBJECT_CACHE_API_KEY` are
set, the ingestion path's `BlobStorage` wraps `CacheClientStore` (`make_cache` in
`data_lake_connection.rs`), and a HEAD served by `CacheClientStore` returns a synthesized
`ObjectMeta { last_modified: Utc::now(), .. }` (the `options.head` branch of `get_opts` in
`object-cache/src/client.rs`) rather than the origin's true creation time. Bounding the probe on
that synthesized time collapses the lower bound to `now() - slack` (~1h): an old duplicate (e.g. a
proxy re-forwarding a weeks-old block) has its real row weeks in the past, below the collapsed
bound, so stage 2 misses it and stage 3 fires a spurious INSERT — a duplicate `blocks` row that
`BlocksView` double-counts. The recovery-arm HEAD must therefore always resolve against the origin
store, cache or no cache (see "Conditional blob PUT").

**Hard rule: the insert path never deletes blobs; only the retention daemon deletes blobs.**
Deleting a blob on PG-insert failure would race a concurrent retry's recovery arm (probe says
"missing" → insert metadata → blob deleted out from under it → dangling row). Leaving the blob
costs nothing: the next retry's recovery arm heals it. Invariant: a blob may transiently exist
without metadata (healed by the next retry); metadata is only inserted while its blob is known to
exist, and nothing but retention removes blobs.

`blocks` itself carries only a **per-partition local `UNIQUE (block_id)`** index (created per
partition; it does not cascade from the parent) as a belt-and-suspenders guard, and the `blocks`
insert uses `ON CONFLICT DO NOTHING` (no target).

**Cost — honest accounting.**
- Zero additional Postgres writes on the hot path (the PUT was already happening; the conditional
  header is free; the bounded probe is 1–2 index probes; the advisory lock is one fast function call
  per *block*, and block rate ≪ event rate).
- Zero residual DELETE churn — no side table, no backfill, nothing to trickle-clean. The entire
  retention path is partition DROP + blob delete.
- Duplicates still pay upload bandwidth before being rejected (the PUT completes before
  `If-None-Match` semantics apply on some backends, and the payload is sent regardless) — same as
  today, where the unconditional PUT re-uploads too. No regression.
- Blobs become create-once: a resend no longer silently overwrites the stored payload. (Payloads
  are identical across retries of the same block, so nothing is lost; a hypothetically corrupt blob
  no longer self-heals via resend — marginal.)
- Dedup holds **within the retention window**, structurally: the blob *is* the dedup record and its
  lifetime is the retention window. A resend whose original is already past retention (blob deleted)
  is treated as a fresh ingest — acceptable, since the original data was already dropped. Document
  this bound (relevant to proxies with multi-day delay).
- Conditional-PUT support becomes a hard deployment requirement. Every backend this workspace can
  target (S3 via the compiled `aws` feature, and local FS) supports it; the only caveat is an
  externally-operated, older S3-compatible store (e.g. old MinIO) lacking `If-None-Match: *` — a
  deployment-time check against that specific target, not a code gap. On S3 this also requires
  keeping the store's default conditional-put mode (`S3ConditionalPut::ETagMatch`, the
  `object_store` 0.13.2 default) — an `aws_conditional_put=disabled` override would silently make
  `put_if_absent` return `Error::NotImplemented`.

### 2. Late-arriving data (proxies, replication) — `insert_time` is stamped at the final hop

Because a proxy can deliver data days late and `insert_time` is the *final* ingestion server's
`Utc::now()`, late data lands in a **current** hourly partition (which exists) — never in a past,
already-dropped one. Event ordering is preserved separately: `begin_time`/`end_time` come from the
payload, so time-based queries still place events correctly in the past; only the lakehouse
processing window (keyed on `insert_time`) treats them as newly arrived. This is already how the
system handles late data, and partitioning by `insert_time` is consistent with it.

The hard rule this imposes: **no ingestion path may carry a client- or proxy-origin `insert_time`.**
The forward-provisioning buffer therefore only needs to cover the future (current + N hours); it
never needs to keep or recreate old partitions for late data. The replication/proxy paths must stamp
`insert_time = now()` at final ingestion (or otherwise guarantee it falls within a live,
non-dropped partition), or the row hits the `DEFAULT` partition and trips the monitoring alarm
(#1243). This supersedes #1240's "replicated rows must carry an insert_time within the retention
window" note: the safe form is to (re)stamp on arrival, not to require the source to have done so.
(Whichever attempt completes an interrupted ingest — original or recovery arm — stamps a fresh
`insert_time`; that is consistent with this rule.)

### 3. Cutover of the existing populated `blocks` table

Postgres cannot convert a populated table to partitioned in place. Recommended approach —
**attach the existing table as a bounded legacy partition** — keeps reads transparent (still one
`blocks` relation, so `BlocksView` SQL is untouched) and turns all legacy rows into a single
droppable partition, so there is **zero row-DELETE churn** even for the pre-existing data:

1. `CREATE TABLE blocks_partitioned (LIKE blocks INCLUDING DEFAULTS) PARTITION BY RANGE (insert_time);`
   with `insert_time` `NOT NULL` declared explicitly in the parent's definition (partition key must be
   non-null; `insert_time` is nullable in the source table — added as plain `TIMESTAMPTZ` in
   `upgrade_data_lake_schema_v2`, with no later migration constraining it — so `LIKE ... INCLUDING
   DEFAULTS` alone does not carry a `NOT NULL` into the new parent; values are already non-null in
   practice, but the constraint must be added here).
2. Cascade the non-unique indexes onto the parent (`CREATE INDEX ON blocks_partitioned (stream_id)`,
   `(begin_time)`, `(end_time)`, `(insert_time)`) — these auto-propagate to all current and future
   partitions.
3. On the existing `blocks`: add `ALTER TABLE blocks ALTER COLUMN insert_time SET NOT NULL` (one-time
   validation scan) and `ALTER TABLE blocks ADD CONSTRAINT blocks_legacy_bound
   CHECK (insert_time < <cutover_hour>)`, so the subsequent ATTACH can skip its own validation scan —
   a range-partition bound implies `insert_time IS NOT NULL`, which the CHECK alone does not
   establish, so both constraints are needed for ATTACH to trust the data without re-scanning; and
   ensure it has a local `UNIQUE (block_id)` index (the existing `blocks_block_id_unique` already
   satisfies this).
4. In a transaction: `ALTER TABLE blocks RENAME TO blocks_legacy;`
   `ALTER TABLE blocks_partitioned RENAME TO blocks;`
   `ALTER TABLE blocks ATTACH PARTITION blocks_legacy FOR VALUES FROM (MINVALUE) TO (<cutover_hour>);`
   `CREATE TABLE blocks_default PARTITION OF blocks DEFAULT;` (+ its local unique-block_id index).
5. Provision the current hour + forward buffer of hourly partitions so inserts immediately after
   cutover have a home (the `DEFAULT` backstops any gap).

**No dedup backfill is needed.** Pre-cutover blocks already have their blobs at the deterministic
key, so a post-cutover resend of an old block hits `AlreadyExists` immediately; the recovery arm's
global probe queries the partitioned parent, which includes `blocks_legacy`, finds the row, and
skips. Dedup for pre-existing data works on day one with zero migration work — this removes what
would otherwise be a bulk insert sized to the live `blocks` row count.

`<cutover_hour>` = the hour boundary at/after migration time (from the shared boundary function). The
`blocks_legacy` partition ages out as one unit: once `<cutover_hour> <= now - retention`, the daemon
drops it wholesale (after blob cleanup) — no per-row deletes for legacy data.

Cost/risk: step 3's `SET NOT NULL` and `ADD CONSTRAINT ... CHECK` each do one table scan (`SHARE
UPDATE EXCLUSIVE` / validation passes, back-to-back); the rename+attach in step 4 takes a brief
`ACCESS EXCLUSIVE` on `blocks`. All are one-time. This is called out in #1240 as the riskiest part;
validate timing against a production-sized `blocks` in staging before rollout.

Alternative considered — **new partitioned table + drain old via existing retention**: point writes
at the new table, keep the old one readable until it drains. Rejected because `BlocksView`'s
`data_sql`/`source_count_query` use `FROM blocks, streams, processes` (join) (filter is on
`blocks.insert_time`) and would need a temporary `UNION` across two relations for the whole
retention window; the attach approach avoids touching read SQL entirely.

## Design

### Shared boundary/naming function (new — `rust/ingestion/src/blocks_partition.rs`)

The one strict coupling: ingestion (forward roll) and the daemon (roll-off) must agree on width,
boundary, and name. Put it in the `ingestion` crate so both depend on the same code.

```rust
/// Hourly UTC partition width.
pub const BLOCKS_PARTITION_WIDTH: TimeDelta = TimeDelta::hours(1);

/// Floor a timestamp to its hourly UTC partition boundary.
pub fn partition_lower_bound(t: DateTime<Utc>) -> DateTime<Utc>;   // duration_trunc(1h)

/// Deterministic partition name for the hour containing `t`, e.g. "blocks_2026071409".
pub fn partition_name(t: DateTime<Utc>) -> String;                 // format "blocks_%Y%m%d%H"

/// [lower, upper) bounds for the hour containing `t`.
pub fn partition_bounds(t: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>);

/// SQL to create the partition (+ its local unique block_id index) IF NOT EXISTS.
pub fn create_partition_sql(name: &str, lower: DateTime<Utc>, upper: DateTime<Utc>) -> String;
```

Naming uses UTC `%Y%m%d%H` so names sort chronologically and a name is reversible to its bound —
the daemon can decide expiry from the name alone (or from `pg_inherits` + `pg_get_expr(relpartbound)`
as a cross-check).

### Conditional blob PUT (new — `rust/telemetry/src/blob_storage.rs`)

Add to `BlobStorage`:

```rust
pub enum PutIfAbsentResult { Created, AlreadyExists }

/// Atomic create-if-absent via PutMode::Create (If-None-Match on S3; natively
/// supported on local FS — the only backends this workspace compiles in, see
/// "Object store capabilities" above). Maps object_store::Error::AlreadyExists
/// to PutIfAbsentResult::AlreadyExists; all other errors propagate.
pub async fn put_if_absent(&self, obj_path: &str, buffer: bytes::Bytes)
    -> Result<PutIfAbsentResult>;
```

Also add a `head_origin(obj_path) -> Result<ObjectMeta>` method — the recovery arm's stage-2 probe
needs the blob's *true* `last_modified`, and that must come from the origin store, never from a
configured object cache (a cache-served HEAD synthesizes `last_modified = Utc::now()`; see Decision
1's "Bounding the recovery probe", correctness caveat 2). `BlobStorage::inner()` cannot be reused
for this: it returns the same (possibly cache-layered) store the lakehouse read paths already rely
on (`write_partition.rs`, `query.rs`, `jit_partitions.rs`, `lakehouse_context.rs`'s `l1_wrap`), so
when the object cache is configured, `inner()` is a `PrefixStore` over `CacheClientStore`. Instead,
`BlobStorage` must retain a handle to the pre-cache-layer origin store alongside the layered one:
`make_cache` clones `direct` (a free `Arc` clone) before moving the original into
`CacheClientStore::new` and returns the clone too; `connect_to_remote_data_lake` and
`connect_to_data_lake` wrap that clone in its own `PrefixStore` with the same lake root (`blob_store_root`)
that `BlobStorage::new` applies to the layered store — matching the prefixing `put`/`read_blob` go
through (`blob_storage.rs:34-38`) — and thread the wrapped result into `BlobStorage` as a second
field; `head_origin` reads from that field exclusively, unconditionally bypassing the cache. Without
this prefix, `head_origin` would look up an un-prefixed key against the raw origin store and miss
every object the lake actually wrote under its root, turning every recovery-arm probe into a false
"not found" and silently breaking the old-duplicate correctness argument above. The existing
unconditional `put` and `inner()` remain unchanged for all other callers (parquet partitions,
lakehouse reads, etc.).

### Insert paths (`web_ingestion_service.rs`, `replication.rs`)

Both call sites switch to the ensure-blob-then-ensure-row flow from Decision 1. Factor the shared
gated-insert logic (lock → probe → insert) into one helper (in the `ingestion` crate; `analytics`
already depends on it) so the two call sites cannot diverge. The helper must **render each time bound
as an inlined `timestamptz` literal in the SQL text** (the value is a server-computed timestamp, so
format it with a fixed rendering — never bind it as a parameter), because only a plan-time literal
gives plan-time partition pruning of both scans and locks (Decision 1); `block_id` stays a bind
parameter as usual:

- **Hot arm** (`Created`): transaction { advisory xact lock on the block; bounded probe
  (`insert_time > '<now-1h>'::timestamptz`, an inlined literal bound so pruning/locking happen at
  plan time — see Decision 1); insert if missing (`ON CONFLICT DO NOTHING`, no target) }.
- **Recovery arm** (`AlreadyExists`): transaction { same lock; **staged** probe — recent window
  first, then `HEAD`-bounded lower-bound probe (Decision 1, "Bounding the recovery probe"); insert
  if missing, else skip-as-duplicate }.
- `web_ingestion_service.rs`'s `insert_block_typed` calls `put_if_absent` and the helper inline, per
  block — the hot/recovery split above applies directly, unchanged in shape.
- `replication.rs` cannot apply the split directly: `ingest_payloads` (blob PUT) and `ingest_blocks`
  (metadata insert) are separate `bulk_ingest` calls, so `ingest_blocks` never sees the
  `PutIfAbsentResult` that `ingest_payloads` produced for a given block. Restructure `ingest_blocks`
  to drop its single whole-batch transaction (today's `begin`/loop/`commit`, `:176-216`) and instead
  invoke the shared helper **once per block**, always taking the **recovery arm** (staged probe):
  this is correct regardless of whether the blob PUT was actually `Created` or `AlreadyExists` (a
  genuinely fresh block still passes the staged probe, just via the recent-window stage rather than
  the tightest 1-hour hot-path bound), at the cost of forgoing the hot path's cheapest probe for
  replicated rows — acceptable given replication is a low-volume expert feature. `ingest_payloads`
  switches to `put_if_absent` for the create-once blob semantics; its `Created`/`AlreadyExists`
  result is otherwise unused since `ingest_blocks` always takes the safe arm.
- Advisory lock: `pg_advisory_xact_lock(int4, int4)` — the two-int4 overload. Postgres keeps this
  overload's lock space separate from the single-argument `pg_advisory_xact_lock(bigint)` form used by
  the migration lock (key `0`, `remote_data_lake.rs:13-18,29`) and by the partition-write locks
  (`generate_partition_lock_key`'s full-range `i64` hash, `write_partition.rs:233-245,273-277`), so
  collision with either is structurally impossible regardless of which constants are chosen — no
  need to coordinate key values across the three call sites. First key is a fixed `BLOCKS_LOCK_CLASS`
  constant; second key is a 32-bit hash of `block_id`. A 32-bit hash collision merely serializes two
  unrelated block inserts briefly — harmless.
- `replication.rs` additionally stamps `insert_time = Utc::now()` on arrival (Decision 2) instead
  of binding the source value.
- Neither path ever deletes a blob (Decision 1 hard rule).

### Schema (#1240) — migration v5

Add `upgrade_data_lake_schema_v5` implementing the cutover from *Critical Design Decision 3*; the
`LATEST_DATA_LAKE_SCHEMA_VERSION` bump to `5` ships separately, in a later deploy (see
"Rolling-deploy hazard" below for why the two cannot ship together). Because `ATTACH PARTITION`/`CREATE INDEX` on
a partitioned parent take heavier locks and some steps (per-partition index creation) may be better
run outside a transaction, model this like the existing v2→v3 migration: run the non-transactional
DDL (index creation) first with `IF NOT EXISTS`, then the transactional swap. Keep it idempotent and
retry-safe (guard on `to_regclass('blocks_legacy')` etc.). No dedup table, no backfill.

**Rolling-deploy hazard: the cutover must not run until the new insert code is live everywhere.**
`migrate_db` runs automatically at binary startup (`connect_to_remote_data_lake`,
`remote_data_lake.rs:60,29`), so during a rolling deploy the *first* new-binary instance to start
triggers the v5 cutover while old instances — still running today's targeted
`INSERT INTO blocks ... ON CONFLICT (block_id) DO NOTHING` (`web_ingestion_service.rs:178-179`,
`replication.rs:197`) — are still live. A partitioned parent cannot carry a unique index on
`block_id` alone (Decision 1), so those old instances' inserts start failing immediately with "no
unique or exclusion constraint matching the ON CONFLICT specification." The new no-target
`ON CONFLICT DO NOTHING` insert code (Implementation Steps, step 5) is schema-agnostic and works
against both the plain and the partitioned table, so a safe order exists: **the new insert code
must be live on every ingestion/replication instance before the v5 cutover runs — never the
reverse, and never interleaved.** Concretely: `upgrade_data_lake_schema_v5` is **never** wired into
`execute_migration`'s automatic `if N == current_version` chain — that chain runs unconditionally
inside `migrate_db` on every process startup (`connect_to_remote_data_lake`,
`remote_data_lake.rs:60,29`), so any step placed there runs the instant the first new binary
starts, which is exactly the ordering this section forbids. Instead, expose
`upgrade_data_lake_schema_v5` through a standalone, explicitly-invoked entry point (a dedicated CLI
subcommand) that an operator runs once, out-of-band, after confirming every
ingestion/replication instance is already running the new insert code (Implementation Steps, step
5) — never as a side effect of a service starting up. This also means
`LATEST_DATA_LAKE_SCHEMA_VERSION` cannot be bumped to 5 in the same deploy as the standalone
subcommand: `execute_migration`'s own chain has no step that advances a database past version 4, so
if a binary declaring `LATEST_DATA_LAKE_SCHEMA_VERSION == 5` starts against a still-v4 database,
`migrate_db`'s post-migration `assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION)` (and
`execute_migration`'s own internal one) would fail and crash the process. The bump therefore ships
in a **later**, separate deploy, made only after an operator has run the standalone cutover and
confirmed `current_version` is already 5 in the database — see Implementation Steps step 4.

New `create_blocks_table` (fresh v1 installs go straight to partitioned) creates
`PARTITION BY RANGE (insert_time)`, the parent non-unique indexes, and a `DEFAULT` partition with
its local unique-block_id index. For fresh installs to skip the v2/v3/v4 upgrade steps — which are
not idempotent against the new partitioned schema (e.g. `upgrade_data_lake_schema_v2`'s `ALTER
TABLE blocks ADD insert_time` has no `IF NOT EXISTS` and would fail against a table that already
has the column) — `create_migration_table` must stamp the `migration` row directly to
`LATEST_DATA_LAKE_SCHEMA_VERSION` (5), not `1`, when invoked from the fresh-install path in
`create_tables`. Because `execute_migration` gates each upgrade step on an exact `if N ==
current_version` check (not `>=`), stamping straight to 5 causes the `if 1/2/3 ==
current_version` branches to be skipped naturally — no other change to `execute_migration`'s
control flow is needed for fresh installs. Existing v4 databases are untouched by this stamping
path: `execute_migration` gets no new `if 4 == current_version` branch at all, so a v4 database only
reaches v5 through the standalone, operator-invoked cutover described in "Rolling-deploy hazard"
below — never automatically.

### Ingestion forward-provisioning (#1241 — new background task)

A task in the ingestion service ensures the current hour + next `N` hours of partitions exist.
Spawn it alongside `serve_ingestion` (from `telemetry-ingestion-srv/src/main.rs`, tied to the same
shutdown signal).

- Loop on an interval (e.g. every few minutes). Maintain an in-process high-water mark "partitions
  through hour H exist"; if the buffer edge is still far away, it is a **zero-DB no-op**.
- Near the buffer edge, take `pg_try_advisory_xact_lock(int4, int4)` — the same two-int4 overload
  as the gated-insert lock (not the single-argument `pg_advisory_xact_lock(bigint)` `acquire_lock`
  pattern used by the migration lock), with its own fixed `PARTITION_PROVISION_LOCK_CLASS` constant
  (distinct from `BLOCKS_LOCK_CLASS`) and a fixed second key. Being the two-int4 overload, it is
  automatically in a lock space collision-free against the migration key `0` and the write-partition
  hashes. If another
  instance holds it, skip this cycle — the forward buffer covers the gap, so no instance ever
  blocks. Transaction-scoped, so a crashed holder can't leak it.
- Inside the lock, for each not-yet-present hour in the buffer window run
  `create_partition_sql(...)` (`CREATE TABLE IF NOT EXISTS <name> PARTITION OF blocks FOR VALUES
  FROM (lower) TO (upper)` + `CREATE UNIQUE INDEX IF NOT EXISTS ... ON <name>(block_id)`). Treat
  "already exists" as success. `IF NOT EXISTS` alone has a TOCTOU race under concurrency; the
  advisory lock closes it. On success advance the high-water mark.

Note: `CREATE ... PARTITION OF` takes a brief `ACCESS EXCLUSIVE` on the parent (catalog change +
scan of the empty `DEFAULT`); at most once/hour, ahead of need, on an empty default → sub-ms.
Buffer size `N` and cadence should be config/env with sane defaults (e.g. 4–6 hours ahead).

### Daemon roll-off (#1242 — replace `delete_expired_blocks`)

Replace `delete_expired_blocks(lake, expiration)` in `delete_old_data` (`delete.rs:157`) with a
partition-drop pass. `delete_empty_streams`/`delete_empty_processes`/`retire_expired_partitions`
stay as-is.

1. Enumerate `blocks` child partitions (via `pg_inherits` join to `pg_class`, or list by name
   prefix) and select those whose entire range is `<= expiration` — i.e. `upper_bound <= expiration`.
   Derive the bound from `pg_get_expr(relpartbound, oid)` or from the name via the shared function;
   **never** consider the `DEFAULT` partition for drop.
2. For each fully-expired partition, interlocked and in this order:
   a. `SELECT block_id, process_id, stream_id FROM <partition>` (paginate for large partitions),
   b. delete the corresponding `blobs/{process_id}/{stream_id}/{block_id}` objects via
      `lake.blob_storage.delete_batch` (payload keys are **not** time-prefixed, so an object-store
      lifecycle rule can't reap them — enumeration is required),
   c. only after blobs are deleted: `DROP TABLE <partition>`.
   Never drop before blobs are gone; if blob deletion fails, leave the partition for the next pass.
   The `SELECT` is read-only, so it avoids DELETE/vacuum churn regardless of partition size.
3. The one-time `blocks_legacy` partition from cutover is enumerated and dropped by exactly this
   path once `<cutover_hour> <= expiration`.

Known narrow race (pre-existing, not a regression): blob keys are time-independent, so a resend of
a ≥retention-old block arriving in the seconds between the daemon's blob-delete (2b) and the table
drop (2c) can recreate the key and insert a fresh row; if the daemon's `delete_batch` for that key
lands after the recreation, the fresh row briefly points at a deleted blob. This race exists
identically under today's row-DELETE scheme and under any side-table design, because it is a
property of the deterministic key, not of the arbiter. Rare² — accept and document.

### Monitoring (#1243)

- **Non-empty `DEFAULT` alarm**: `SELECT count(*) FROM blocks_default` (or the default child) — alarm
  on `> 0`. Rows there mean the forward buffer starved or data was imported with an `insert_time`
  outside the retention window. A filled default also blocks creating that hour's partition later
  (Postgres scans the default on partition create and fails if it holds rows for the new range).
- **Low forward-buffer alarm**: count provisioned future hourly partitions
  (`> now`); alarm when below a threshold (provisioning falling behind, before it starves).
- **Recovery-arm counter**: count `AlreadyExists` outcomes split by resolution (duplicate-skipped
  vs. recovered-insert) and by probe stage (recent-window hit vs. HEAD-bounded hit vs. miss). A sustained rise in recovered-inserts means PG insert failures are
  happening upstream; a sustained rise in duplicates is retry-storm telemetry. Cheap and directly
  validates the dedup design in production.
- Emit these as metrics on the existing tracing/metrics path so they can be alarmed in the standard
  dashboards. Exact surfacing (log metric vs. a maintenance-task gauge) is an implementation detail
  for #1243.

## Implementation Steps

**Phase 1 — Foundation (#1240)** — must deploy together with Phase 2. Within Phase 1, deploy order
matters and differs from the step-list order below: the insert-code change (step 5) must reach
every ingestion/replication instance before the v5 migration cutover (steps 3–4) is allowed to run
— see "Rolling-deploy hazard" under Schema (#1240) above.
1. Add `rust/ingestion/src/blocks_partition.rs` (shared boundary/naming/SQL helpers); export from
   `ingestion/src/lib.rs`. Unit tests under `ingestion/tests/`.
2. Add `BlobStorage::put_if_absent` (`rust/telemetry/src/blob_storage.rs`) with
   `PutMode::Create`; unit-test the `AlreadyExists` mapping against the in-memory/local backend.
3. Rewrite `create_blocks_table` (`sql_telemetry_db.rs`) to create the partitioned parent + parent
   indexes + `DEFAULT` partition (with local unique-block_id index) for fresh installs.
4. Add `upgrade_data_lake_schema_v5` (`sql_migration.rs`) implementing the attach-legacy cutover
   (mirror the v3 non-transactional-DDL-then-transaction structure). Expose it through a standalone,
   explicitly-invoked entry point (e.g. a dedicated CLI subcommand) — **not** an `if 4 ==
   current_version` step inside `execute_migration` — so it never runs as a side effect of
   `migrate_db`/`connect_to_remote_data_lake`'s automatic startup path (see "Rolling-deploy hazard"
   above). Bump `LATEST_DATA_LAKE_SCHEMA_VERSION` to 5 only in a later, separate deploy, made after
   an operator has run the standalone cutover and confirmed `current_version` is already 5 —
   bumping it any earlier would make every instance's automatic `migrate_db` assert fail at startup,
   since `execute_migration`'s chain no longer has a step that advances a database past 4.
5. Implement the shared gated-insert helper (lock → probe → insert, hot + recovery arms) in the
   `ingestion` crate; switch `web_ingestion_service.rs`'s `insert_block_typed` to `put_if_absent` +
   the helper (hot/recovery split per block, unchanged shape). Switch `replication.rs`'s
   `ingest_payloads` to `put_if_absent`, and restructure `ingest_blocks` to drop its single
   whole-batch transaction in favor of calling the helper once per block, always via the recovery
   arm ("Insert paths", above); change the `blocks` insert to `ON CONFLICT DO NOTHING` (no target);
   stamp `insert_time = now()` on arrival in `replication.rs` (Decision 2). Enforce the "insert path
   never deletes blobs" rule.
6. Document that late data (proxies/replication) lands in current partitions and that dedup holds
   within the retention window (blob lifetime).

**Phase 2 — Ingestion forward-provisioning (#1241)** — deploys with Phase 1; `DEFAULT` backstops.
7. Add the provisioning background task (module in `ingestion/`), using the shared helper and
   `pg_try_advisory_xact_lock(int4, int4)` with the `PARTITION_PROVISION_LOCK_CLASS` constant;
   in-process high-water-mark cache; buffer size/cadence config.
8. Spawn it from `telemetry-ingestion-srv/src/main.rs` (that binary only ever runs the ingestion
   role, so this is unconditional) and, in the monolith, from inside the existing
   `if roles.ingestion` block (`monolith/src/main.rs:234`) alongside `serve_ingestion` — **never**
   unconditionally: the monolith only builds a lakehouse/db pool at all under
   `roles.needs_lakehouse()` (`monolith/src/main.rs:92-94,177`), and the provisioner needs a pool, so
   it must be gated at least as tightly as ingestion itself. Tie it to the same shutdown signal as
   `serve_ingestion`.

**Phase 3 — Daemon roll-off (#1242)** — parallel with Phase 2 after Phase 1.
9. Add a `drop_expired_block_partitions` function (new, in `analytics/src/delete.rs` or a sibling
   module) implementing enumerate → blob-delete → `DROP TABLE`, using the shared helper.
10. Replace the `delete_expired_blocks` call in `delete_old_data` with it; delete
    `delete_expired_blocks`/`delete_expired_blocks_batch` (dead after cutover). Update tests.

**Phase 4 — Monitoring (#1243)** — after 2 and 3.
11. Add the default-non-empty, low-forward-buffer, and recovery-arm metrics + alarm wiring.

## Files to Modify
- `rust/ingestion/src/blocks_partition.rs` — **new** shared helper.
- `rust/ingestion/src/lib.rs` — export new modules.
- `rust/telemetry/src/blob_storage.rs` — `put_if_absent` (conditional PUT) + `head_origin`
  (cache-bypassing origin HEAD).
- `rust/ingestion/src/sql_telemetry_db.rs` — partitioned `create_blocks_table`.
- `rust/ingestion/src/sql_migration.rs` — `upgrade_data_lake_schema_v5` (cutover), version bump,
  wiring.
- `rust/ingestion/src/<gated_insert>.rs` — **new** shared lock→probe→insert helper.
- `rust/ingestion/src/web_ingestion_service.rs` — conditional PUT + gated insert.
- `rust/analytics/src/replication.rs` — same; stamp `insert_time` on arrival.
- `rust/ingestion/src/<provisioner>.rs` — **new** forward-provisioning task.
- `rust/telemetry-ingestion-srv/src/main.rs`, `rust/monolith/src/main.rs` — spawn provisioner
  (gated on `roles.ingestion` in the monolith, alongside `serve_ingestion`).
- `rust/analytics/src/delete.rs` — replace block deletion with partition drop; drop dead fns.
- Monitoring surfacing (#1243) — location TBD in Phase 4.
- Tests under `rust/ingestion/tests/`, `rust/telemetry/tests/`, and `rust/analytics/tests/`.

## Trade-offs
- **Object-store conditional PUT vs. a Postgres side table as the dedup arbiter** — see Critical
  Design Decision 1. The blob already exists, its key is deterministic across retries, and its
  lifetime already equals the retention window, so making its creation atomic gives global dedup
  with zero extra Postgres writes and zero residual DELETE churn; the migration backfill disappears
  too (pre-cutover blobs already serve as dedup records). Costs: a hard dependency on
  conditional-PUT support in the object store; the arbiter and the data no longer share a
  transactional domain, so idempotency requires the probe-based recovery arm instead of a single
  transaction; blobs become create-once. The side table remains the documented fallback if a
  deployment target lacks conditional PUT.
- **Lock+probe on the hot path** — one advisory-lock call and a 1–2-partition index probe per block
  insert buys full correctness against in-flight-retry races (including the boundary-straddle
  case). Block rate ≪ event rate, so this is noise; the recovery arm's probe is bounded by the
  blob's creation time (create-once ⇒ no row predates its blob), so even the rare arm never pays
  an unpruned scan over all partitions.
- **Attach-legacy cutover vs. drain-old-table** — chose attach to keep read SQL untouched and avoid
  any row-DELETE churn for legacy data, at the cost of a one-time validation scan + brief exclusive
  lock during migration.
- **Provisioning on the write path vs. in the daemon** — chose write path so write availability does
  not depend on the daemon being up; the trade is a new background task in the ingestion service.
- **Hourly width** — matches `BlocksView`'s incremental windows: `get_max_partition_time_delta`
  returns 1h for the `Abort`/`CreateFromSource` strategies and 1 day for `MergeExisting`
  (`blocks_view.rs:140-147`), so create-from-source partitions prune 1:1 and merge windows still
  prune to a bounded (≤24-partition) set; finer widths multiply catalog objects, coarser widths
  coarsen retention granularity.

## Documentation
- `mkdocs/docs/admin/` — document the partitioned-`blocks` retention model (drop-based, hourly), the
  blob-as-dedup-arbiter design (conditional PUT, recovery arm, "insert path never deletes blobs"),
  the conditional-PUT requirement on the object store (S3 ≥ Nov 2024 and local FS — the only
  backends this workspace compiles in; GCS/Azure are not enabled features; older MinIO
  unsupported), the forward-buffer/`DEFAULT` invariants and their alarms, and the operational
  runbook for a filled default.
- Replication/proxy docs — `insert_time` is stamped at final ingestion, so late data (proxies
  buffering for days) lands in current partitions; dedup is guaranteed within the retention window
  (the blob's lifetime) and a resend whose original has already aged out is re-ingested. Note the
  corollary constraint: no path may carry a client/proxy-origin `insert_time`.
- `cost-effectiveness.md` may warrant a note that block retention no longer drives Aurora ACU spikes
  (fully drop-based; no residual row deletes).

## Testing Strategy
- **Unit**: shared boundary function (floor/name/bounds round-trips across DST-free UTC hours,
  boundary instants); `create_partition_sql` output; `put_if_absent` Created/AlreadyExists mapping.
- **Migration**: on a seeded v4 database with populated `blocks`, run `execute_migration`; assert
  `blocks` is partitioned, `blocks_legacy` attached with the right bound, `DEFAULT` present, all
  indexes (incl. per-partition unique-block_id) present, and existing rows still queryable via
  `BlocksView`. Assert idempotency (re-run is a no-op) and retry-safety.
- **Insert/dedup**:
  - A resend of the same `block_id` is deduped even when the two attempts fall in **different**
    hourly partitions (simulate the laptop-sleep / proxy-delay case) — the recovery arm's probe
    must make the second a no-op with no duplicate `blocks` row.
  - **Staged probe**: a duplicate of a block older than the stage-1 window (25h) is found via the
    `HEAD`-bounded probe; a row inserted **late by the recovery arm** (`insert_time` ≫ blob
    creation) is still found by a subsequent resend (lower-bound-only correctness — the probe must
    never window *around* blob time).
  - **Idempotency after PG failure**: PUT the blob, make the metadata insert fail (kill the
    transaction), retry the full insert; assert the retry takes the recovery arm and ends with
    exactly one row and one blob. Assert a second retry after success is a pure no-op.
  - A resend after the original's partition and blob have aged out is re-ingested (documented
    bound).
  - **Concurrent race**: two tasks inserting the same block concurrently (one hot arm, one recovery
    arm, serialized by the advisory lock) end with exactly one row.
  - Dedup of a **pre-cutover** block: seed via migration test, resend post-cutover, assert
    recovery arm finds the `blocks_legacy` row and skips (no backfill needed).
  - Assert the insert path never deletes blobs on failure.
- **Provisioning**: task creates the buffer of future partitions; concurrent instances don't error
  under `pg_try_advisory_xact_lock` (simulate contention); no-op when buffer is full.
- **Roll-off**: seed partitions with blobs, run retention; assert expired partitions' blobs deleted
  then `DROP`ped, non-expired untouched, `DEFAULT` never dropped, blob-delete failure leaves the
  partition intact for retry.
- **End-to-end** via `local_test_env` split-mode services: ingest → materialize `BlocksView` →
  age past retention → confirm partition dropped and query still correct.
- Run `python3 build/rust_ci.py` (fmt + clippy + tests).

## Decisions & Rollout Validation
1. **Buffer size `N` and provisioning cadence** — Decision: maintain partitions **12 hours ahead**
   of now, with the provisioner running **every hour** plus a **large random jitter — on the order of
   the full check interval** (each instance sleeps a random duration up to ~1h, so its attempts land
   at an independent random offset within the hour); all values configurable via env/config.
   Rationale: a 12h horizon comfortably exceeds any plausible ingestion-instance restart gap while
   keeping the pre-created partition count small, and it is 12× the hourly cadence so several
   consecutive missed runs — including the ~1h extra spread the jitter can add — are still harmless.
   The large jitter spreads each instance's attempt uniformly across the interval so multiple
   ingestion servers almost never run the create-partition path at the same moment. This matters
   because `CREATE TABLE ... PARTITION OF IF NOT EXISTS`, while idempotent, still takes a short
   `ACCESS EXCLUSIVE` lock on the parent `blocks` table; synchronized attempts across N instances
   would serialize on that lock and briefly contend with the ingestion write path, whereas a
   wide random spread makes concurrent attempts rare and each one cheap. If deployed instance count
   or restart cadence ever demand more headroom, the horizon can simply be raised.
2. **Cutover lock duration** — Pre-rollout validation gate, not an open question: the exclusive-lock
   window from `ADD CONSTRAINT ... CHECK` scan + `ATTACH` must be measured on a production-sized
   `blocks` in staging before the cutover. Decision: proceed with the attach approach; the staging
   measurement is the go/no-go. If the measured exclusive window exceeds an acceptable maintenance
   window budget, fall back to the already-documented drain-old-table alternative (temporary
   `UNION` in `BlocksView`).
3. **Recovery-arm probe on Aurora** — Decision: **hourly partitions, no daily fallback**, with a
   **1h** skew slack. Hourly is made robust by construction rather than by validated luck: every
   time-bounded probe (both hot-path and recovery stages 1–2) inlines its bound as a **literal
   `timestamptz` constant**, so Postgres prunes at *plan time* and locks only the surviving
   partitions (see Decision 1, "Bounding the recovery probe"). This does **not** depend on
   executor-startup runtime pruning happening before lock acquisition — the version-dependent Aurora
   behavior the earlier draft hedged with a daily-partition fallback. Because plan-time pruning on a
   literal constant is deterministic Postgres behavior, the fallback is removed. Remaining pre-rollout
   check is a confirmation, not a go/no-go on the partitioning scheme: `EXPLAIN` a representative
   stage-1 and stage-2 probe on the target Aurora version and confirm the plan lists only the
   expected partitions (no `Append` over all ~2,160), and observe stage-1 hit rate and `HEAD` latency
   at real retry rates to tune the slack. The only unbounded case — a full-range not-found scan for a
   ~90-day-old blob whose row vanished — is essentially never and remains correct if it occurs (see
   Decision 1's per-case cost note).
4. **pg_partman** — Decision: **do not adopt**. Partition creation stays app-controlled (the forward
   provisioner from item 1), consistent with roll-off, which must stay app-controlled anyway because
   pg_partman's built-in retention drop is incompatible with the blob-before-drop interlock. This
   avoids a dependency on the Aurora extension allowlist and keeps creation and drop under one
   mechanism.

**Resolved during research:** the only direct Postgres readers/writers of the `blocks` table are
`BlocksView` materialization (`blocks_view.rs`), `delete.rs`, `replication.rs`,
`web_ingestion_service.rs`, and the one-time `UPDATE` in `sql_migration.rs:37`. Other `FROM blocks`
occurrences (`processes_view.rs`, `streams_view.rs`, `log_stats_view.rs`, `process_streams.rs`,
`parse_block_table_function.rs`, `query_processes.rs`, `frame_budget_reporting.rs`) query the
materialized DataFusion `blocks` **view** (they reference joined columns like `"processes.exe"` and
run via `client.query`/DataFusion contexts), not the Postgres table — so they are transparent to
this change.

**Resolved during design discussion:** `object_store` 0.13 supports `PutMode::Create`, and this
workspace compiles it with only the `aws` feature (`rust/Cargo.toml:66`) — GCS/Azure support is not
built in — so the only in-repo deployment targets are S3 and local FS, both of which support
conditional PUT; the former conditional-PUT support matrix (Open Question) is resolved by this,
leaving only an externally-operated older S3-compatible store (e.g. old MinIO) as a residual
deployment caveat, not an open design question. The object-cache layers
(`object-cache/src/client.rs:447`, `l1_store.rs:167`) forward `put_opts` to the origin, so the
conditional PUT works through the full ingestion stack. The ingestion call site (`insert_block_typed`) already writes the blob before the
metadata row within a single call, so the ensure-blob-then-ensure-row flow requires no reordering
there. The replication call site does not have this property: `ingest_payloads` (blob PUT) and
`ingest_blocks` (metadata insert) are separate `bulk_ingest` calls with no shared transaction, and
`ingest_blocks` inserts a whole batch in one transaction today rather than per block. Reordering
isn't needed — callers already send the payloads stream before the blocks stream — but
`ingest_blocks` must still be restructured to a per-block, always-recovery-arm gated insert ("Insert
paths", above) so correctness does not depend on that call-ordering assumption holding.
