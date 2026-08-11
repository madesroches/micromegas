# Create-only block object writes — Plan

Closes #1465. Root cause diagnosed in #1462.

## Overview

Block payload objects in the lake are assumed **write-once and content-addressed** — the range
cache keys carry no etag, TTL, or generation and are never invalidated
(`rust/object-cache/src/range_cache/mod.rs:53-63`). Nothing enforces that assumption: the
ingestion write is an unconditional `put` that happily overwrites an existing key
(`rust/ingestion/src/web_ingestion_service.rs:164-176`). The OTLP logs path breaks the assumption
on every legitimate webhook redelivery, producing three divergent versions of one logical block.

This plan makes the write **create-only** (`PutMode::Create`), which enforces the invariant
structurally, and makes the dedup paths **observable** — today they are `debug!` only, which is why
#1462 went unnoticed for two months. It then removes the pre/post dual-encode in `split_logs`, which
turns out to be a ~15-line change rather than a refactor: the arrival-time block bounds that the
issue's item 2 asks for are **already implemented** (`logs_bounds` → `build_prepared_block`), so all
that is missing is the read-side substitution in `logs_block_processor`.

## Current State

### The write path

Both the native and OTLP ingestion paths funnel through one function
(`rust/ingestion/src/web_ingestion_service.rs:145-215`) — `insert_block` (native CBOR body, via
`rust/public/src/servers/ingestion.rs:81`) just decodes and delegates to `insert_block_typed`, which
the OTLP handler calls directly (`rust/otel-ingestion/src/handler.rs:136`). So there is a single
change point:

```rust
// web_ingestion_service.rs:164-176
self.lake.blob_storage.put(&obj_path, encoded_payload.into()).await?;   // unconditional overwrite
// :181-200
sqlx::query("INSERT INTO blocks VALUES(...) ON CONFLICT (block_id) DO NOTHING;")
// :203-205
if result.rows_affected() == 0 {
    debug!("duplicate block_id={block_id} skipped (already exists)");   // invisible in practice
}
```

Consequences of PUT-then-INSERT with an unconditional PUT:

| Scenario | Object | Row |
|---|---|---|
| Second arrival, same bytes | rewritten, same content (harmless but wasted) | first arrival's |
| Second arrival, **different** bytes, same `block_id` | **rewritten with new content** | first arrival's |
| Crash between PUT and INSERT | orphaned object, no row | absent |

Row 2 is the #1462 bug. The stale-cache detector that used to catch it — the range cache's
"origin object changed size mid-fetch" length check — no longer fires: since #1464 encoded
`objects` as a CBOR byte string, the envelope length no longer depends on byte *values*, so a
colliding write replaces content at identical length and passes the length check silently.

### Why OTLP logs hit row 2 on every retry

`split_logs_with_extra_hash_input` (`rust/otel-ingestion/src/block.rs:288-345`) hashes the payload
*before* timestamp backfill and stores the payload *after*:

```rust
let pre_mutation_bytes = rl.encode_to_vec();
let block_id = block_id_from_payload(&pre_mutation_bytes);      // :303-313
...
        if record.time_unix_nano == 0 && record.observed_time_unix_nano == 0 {
            record.observed_time_unix_nano = now_nanos;         // :323-325, now_nanos = Utc::now()
...
let payload_bytes = if nb_backfilled > 0 { rl.encode_to_vec() } else { pre_mutation_bytes };  // :334-338
```

`build_webhook_request` leaves both timestamps at 0 on every record by design
(`rust/otel-ingestion/src/handler.rs:243-244`), so for webhook traffic `nb_backfilled > 0`
*unconditionally*: every redelivery re-derives the same `block_id`, backfills a fresh `Utc::now()`,
and overwrites the stored object with different bytes. `split_metrics` (`block.rs:363`) and
`split_traces` hash the same bytes they store, so logs is the only affected signal.

### The arrival-time fallback already exists

The issue's item 2 ("move the timestamp fallback out of the payload and into block metadata") is
mostly already implemented, which was not obvious from #1462:

- `logs_bounds` (`block.rs:41-69`) already returns `Some((0, 0, count))` for an all-zero-timestamp
  resource, with the comment *"All records have zero timestamps — fall back to wall clock at
  handler."*
- `build_prepared_block` (`block.rs:203-208`) already honors that sentinel:
  `if min_nanos == 0 && max_nanos == 0 { let now = Utc::now(); (now, now) }`.

So block `begin_time`/`end_time` land on arrival time **without** the payload mutation. The only
thing the backfill still buys is the downstream read: `logs_block_processor` drops records with no
timestamp rather than substituting one (`rust/analytics/src/lakehouse/otel/logs_block_processor.rs:102-111`):

```rust
} else {
    // No timestamp at all — skip so it doesn't anchor the partition at 1970-01-01.
    nb_dropped_no_timestamp += 1;
    continue;
};
```

## Design

Three changes, shipped as **one PR** in two parts. Part 1 is the correctness fix and stands on its
own; Part 2 restores the "stored object is a pure function of `block_id`" invariant on top of it. They
land in the same commit series, but they have a **rollout order** between services — see
"Rollout order" below, which is an operational constraint, not a PR-boundary one.

### Part 1 — create-only write + observable dedup

#### 1a. `BlobStorage::put_if_absent` (`rust/telemetry/src/blob_storage.rs`)

`put` must keep overwrite semantics — it is public API of the published `micromegas-telemetry` crate
and `analytics/tests/test_helpers.rs:69` still relies on it — so add a sibling rather than changing
it. `replication.rs:168` moves to the new sibling too (see 1b); after this change, no production code
path uses `put` — its only remaining caller is that test helper. `put` returns
`anyhow::Result`, which erases `object_store::Error`, so the create-only variant must classify the
collision itself and return it as a value:

```rust
/// Outcome of a create-only write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutIfAbsent {
    /// The object did not exist and now holds the supplied bytes.
    Created,
    /// The object already existed; it was left untouched and the bytes were discarded.
    AlreadyExists,
}

/// Writes a blob only if the key is absent, leaving an existing object untouched.
///
/// The lake's object keys are write-once and content-addressed — the range cache keys
/// carry no etag or generation and are never invalidated (see `range_cache`'s module
/// docs) — and this is what enforces that: a colliding write is reported, never applied.
pub async fn put_if_absent(&self, obj_path: &str, buffer: bytes::Bytes) -> Result<PutIfAbsent>
```

Implemented with `put_opts(&Path::from(obj_path), buffer.into(), PutOptions::from(PutMode::Create))`,
mapping `Err(object_store::Error::AlreadyExists { .. })` to `Ok(PutIfAbsent::AlreadyExists)` and
`Err(object_store::Error::NotImplemented { .. })` to an error whose message names the cause and the
fix (see Configuration risk below). All other errors propagate.

#### 1b. `insert_block_typed` (`rust/ingestion/src/web_ingestion_service.rs:164-215`)

```
put_if_absent(obj_path, payload)
  ├── Created        ─┐
  └── AlreadyExists  ─┴──► INSERT ... ON CONFLICT (block_id) DO NOTHING
                                 │
                                 └──► classify (object_outcome, rows_affected) → one log + counter
```

**On `AlreadyExists`, fall through to the INSERT — do not return early.** A crash between the PUT
and the INSERT leaves an orphaned object with no row, and only a later attempt that still runs the
insert heals it. Returning early on `AlreadyExists` would make that orphan permanent.

The four (object, row) combinations are distinct facts and get one log line and one counter each,
rather than two independent warnings on the common retry path:

| Object | Row | Meaning | Log | Counter |
|---|---|---|---|---|
| `Created` | inserted | normal first write | `debug!` (as today) | — (`put_duration` counts these) |
| `AlreadyExists` | conflict | **retry, or two distinct events with identical bytes** | `warn!` | `block_object_duplicate` |
| `AlreadyExists` | inserted | orphaned object healed — a prior attempt died between PUT and INSERT; **or** the losing side of a concurrent-duplicate race (its INSERT committed before the winner's) | `warn!` | `block_orphan_object_healed` |
| `Created` | conflict | row existed but object did not — object lost or deleted out from under its row; **or** the winning side of a concurrent-duplicate race (its PUT committed after the loser's INSERT already had) | `debug!` | `block_object_recreated` |

`insert_block_typed` runs per request with no serialization, so two in-flight deliveries of the same
`block_id` (e.g. a webhook producer retrying on timeout) race PUT against INSERT independently: the
PUT loser gets `AlreadyExists`, and if its INSERT commits first it lands in `AlreadyExists`+inserted;
the PUT winner then gets `Created`+conflict on its own INSERT. Neither row is an anomaly in that case
— it's the ordinary concurrent-duplicate path, just interleaved. Retention's
`delete_expired_blocks_batch` (`rust/analytics/src/delete.rs:12-47`) can *also* land a row in either
bucket: it deletes the row and the object together inside one transaction (`DELETE ... RETURNING`,
then `blob_storage.delete_batch`, then commit), so a row surviving without its object requires either
a commit failure after the blob delete, or a partial `delete_batch` failure — `delete_batch`
propagates on the first non-`NotFound` error (`delete.rs:36`), so a transient object-store error
mid-batch can leave some objects deleted and then roll back the whole transaction, restoring rows
whose objects are already gone. So each of these two counters measures "anomaly or concurrent
duplicate," not anomaly alone, and cannot on its own distinguish the two. `AlreadyExists`+inserted
keeps `warn!` since the retention-failure case is the more consequential one to surface promptly;
`Created`+conflict drops to `debug!` since the concurrent-duplicate race is the common case for a
retrying producer and the retention-failure case is already caught via `AlreadyExists`+inserted or
via retention's own logging.

All three counters follow the existing convention — `imetric!("<name>", "count", 1_u64)`, matching
`object_warm_requested` (`rust/ingestion/src/data_lake_connection.rs:68`) and the `range_cache_*`
family. `block_object_duplicate` is the detector the issue asks for: with Part 1 in place it should
sit near zero apart from genuine producer retries, and any sustained rate means a producer supplying
no event identity is losing data (out of scope here; the protocol answer is #1466).

`warn!` on the retry path is a deliberate accepted risk: a producer retrying aggressively will log
one line per redelivery. That is the point — it is currently invisible — and the volume is bounded by
the request rate, which is already bounded by the body-size cap and auth. Include `block_id`,
`process_id`, and `stream_id` in the message so an operator can go straight from a warn to a query
against the `blocks` view.

`rust/analytics/src/replication.rs:168` writes into the same `blobs/{process_id}/{stream_id}/{block_id}`
namespace via the same `BlobStorage::put`, inside `ingest_payloads`, which streams only the
`payloads` table. The block-row `INSERT ... ON CONFLICT (block_id) DO NOTHING` lives in a separate
function, `ingest_blocks` (`replication.rs:~204`), driven by an independent `bulk_ingest(..., "blocks",
...)` stream over a different record batch — the two are not a PUT-then-INSERT pair, so there is no
fall-through and no orphan-heal ordering concern here. Switch `ingest_payloads`'s write to
`put_if_absent` anyway, so the write-once guarantee holds for every writer into this namespace, not
just the ingestion path; simply ignore (or `debug!`-log) the `AlreadyExists` outcome and continue the
stream — `ingest_blocks`'s own `ON CONFLICT DO NOTHING` INSERT already handles row-level dedup
independently. `async_parquet_writer.rs:30` is a different namespace (partition files) and correctly
keeps `Overwrite`.

#### Configuration risk: `PutMode::Create` support

Verified against the pinned `object_store` 0.13.2:

- **S3**: `S3ConditionalPut::ETagMatch` is the `#[default]`
  (`aws/precondition.rs:117-130`); `PutMode::Create` sends `If-None-Match: *` and maps
  412/304 to `Error::AlreadyExists` (`aws/mod.rs:188-202`). Works against AWS S3 itself with no
  configuration; see below for the S3-*compatible* case.
- **`LocalFileSystem`**: `std::fs::hard_link` from the staging file, `ErrorKind::AlreadyExists`
  → `Error::AlreadyExists` (`local.rs:372-384`).
- **`InMemory`**: `storage.create(...)` (`memory.rs:213`) — so unit tests can cover the collision
  without a real backend.
- **`PrefixStore`** forwards `put_opts` verbatim (`prefix.rs:99-107`), and both cache layers
  delegate `put_opts` straight to the origin (`object-cache/src/l1_store.rs:167-173`,
  `object-cache/src/client.rs:961-967`). Nothing in the stack strips the mode.

Two failure modes, and one silent-acceptance risk, all on S3-*compatible* (non-AWS) stores:

- An S3-compatible store explicitly configured with `aws_conditional_put=disabled`: `Create` then
  returns `Error::NotImplemented` (`aws/mod.rs:183-187`) and **every block write fails** rather than
  degrading. Since `parse_object_store_url` feeds the process env vars into `parse_url_opts`
  (`rust/telemetry/src/blob_storage.rs:11-21`), an operator can set that var. Mitigation is a clear
  error message from `put_if_absent`, not a fallback — silently falling back to overwrite would
  restore the bug this plan exists to fix.
- A store that *accepts* `If-None-Match: *` but doesn't enforce it — i.e. it returns 200 and
  overwrites regardless of the header. `object_store`'s own docs on `S3ConditionalPut` say only
  "*Some* S3-compatible stores, such as Cloudflare R2 and minio support conditional put," which
  implies others don't. Against such a store, `put_opts` returns `Ok(PutMode::Created)` on every
  call — no `Error::NotImplemented`, no `Error::AlreadyExists` — so `put_if_absent` cannot distinguish
  this from a real create, and the write silently degrades back to plain overwrite with no error and
  no log line. This is exactly the silent-fallback outcome rejected under Trade-offs, except it
  arrives from store behavior rather than from a deliberate code path, so no config flag names it.
  There's no code-level detection for this (a store can lie about `If-None-Match` support
  indefinitely); the mitigation is operational: before depending on `PutMode::Create` against a new
  S3-compatible endpoint, do a one-time verification — write a key, write different bytes to the same
  key, read it back, and confirm either an `AlreadyExists` error on the second write or (if it
  succeeded) that the read still returns the *first* write's bytes. If neither holds, the store does
  not honor conditional put and this plan's invariant does not hold against it.

Separately, a smaller behavior change: `object_store` marks `Overwrite` puts `idempotent(true)` but not
`Create` (`aws/mod.rs:181-182`), so a transient network error on the PUT is no longer retried
internally and surfaces to the producer as a 503. The producer's retry then lands on the
`AlreadyExists` path if the first attempt actually committed, so the outcome is correct — just
one more error visible to clients than before.

### Part 2 — hash the bytes that get stored

Delete the mutation loop and the second `encode_to_vec` in `split_logs_with_extra_hash_input`
(`block.rs:314-338`): encode once, hash those bytes, store those bytes.

This removes the `observed_time_unix_nano` backfill that #1123/#1124 added to satisfy the OTLP spec's
requirement that "the collecting system" supply an observed timestamp
(`rust/otel-ingestion/src/block.rs:263-265`). That requirement bound the record as OTLP data; it no
longer applies once the record stops being OTLP data at rest. The stored payload is never re-exported
as OTLP — it is read back only by `logs_block_processor`, internal to this codebase — so the
observed-timestamp obligation is satisfied at the block-bounds/query layer (`begin_time`/`end_time`
and, after this change, the processor's substitution) rather than by mutating the record itself.

Behavior change for **mixed** blocks (some records timestamped, some not): today `logs_bounds` sees
the *post-backfill* records, so a zero-timestamp record's freshly-stamped `Utc::now()` can push
`end_time` out to arrival time even though every other record is old. After this change,
`logs_bounds` sees only the original timestamps, so a mixed block's bounds come solely from its
timestamped records — `end_time` no longer gets stretched to arrival time by an untimestamped record
sharing the block. Only fully zero-timestamp blocks still get the arrival-time sentinel.

This is small because `logs_bounds` + `build_prepared_block` already substitute arrival time for an
all-zero-timestamp resource (see Current State). The only functional gap is downstream, in
`logs_block_processor.rs:102-111`: substitute the block's `begin_time` instead of dropping the
record.

```rust
let time_nanos = if record.time_unix_nano != 0 {
    record.time_unix_nano as i64
} else if record.observed_time_unix_nano != 0 {
    record.observed_time_unix_nano as i64
} else {
    // No timestamp in the payload: the block's begin_time carries the arrival-time
    // fallback that ingestion applied in build_prepared_block. Using begin_time (not
    // insert_time) keeps the row inside [block.begin_time, block.end_time], which
    // partition bounds and min/max stats depend on.
    nb_substituted_block_time += 1;
    begin_time_nanos
};
```

`BlockMetadata` carries `begin_time` (`rust/telemetry/src/types/block.rs:7`) and the processor
already reads `insert_time` off `src_block.block` (`logs_block_processor.rs:59-64`), so this is a
sibling `timestamp_nanos_opt()` next to the existing one.

**Why `begin_time` and not `insert_time`.** For a webhook block, both are within milliseconds of
each other. For a *mixed* block — some records timestamped, some not — `begin_time` is the min over
the timestamped records, so a substituted record lands inside the block's declared range;
`insert_time` would land outside it. Records whose real time is unknown getting the oldest known
time in their batch is arbitrary, but it is bounded, in-range, and monotone with the block.

What Part 2 buys:

1. **`payload_size` becomes provably correct.** Today the stored bytes are only *incidentally* a
   function of `block_id`: on the orphan-heal path the row's `payload_size` comes from arrival #2's
   encoding while the object holds arrival #1's bytes. Those lengths happen to be equal —
   `observed_time_unix_nano` is a proto `fixed64`, prost omits it at 0 and emits exactly 9 bytes when
   set, and two arrivals with the same `block_id` have the same set of zero-timestamp records — so
   the mismatch is unreachable *today*. It is unreachable by accident, and a mismatch means
   inaccurate metadata: `payload_size` feeds `get_max_payload_size` → the `nb_tasks` partition-sizing
   heuristic (`block_partition_spec.rs:106`), so a wrong value skews ETL concurrency rather than
   making the block unreadable — reads go through `fetch_block_payload`/`read_blob`
   (`rust/analytics/src/payload.rs:19-38`), a plain unranged `ObjectStore::get`, so nothing in the
   read path ever consults the row's `payload_size`. Part 2 removes the reasoning entirely for two
   arrivals encoded by the *same build*: identical `block_id` ⇒ identical bytes ⇒ identical length.
   This is why the design needs no `head()` call on the collision path to recover the true size.
   The invariant is scoped to same-encoding arrivals, not unconditionally: a `block_id` orphaned by a
   pre-#1464 build and healed by a post-#1464 build (or the mirror `Created`+conflict case) still
   produces a row/object pair from two different CBOR envelope encodings of the same underlying
   bytes, since #1464 changed the `objects`/`dependencies` envelope from one array item per byte to a
   byte string. This is a known, bounded exception — one deploy-boundary window, and, per (1) above,
   harmless even when hit, since a `payload_size` mismatch only skews metadata, not readability.
2. Removes the second `encode_to_vec()` on every webhook request, and with it the accepted-tradeoff
   note at `block.rs:270-276` and `handler.rs:223-232`. The CPU saving is real but minor — one proto
   encode of a body bounded by the ~300 MiB decompressed cap; the invariant in (1) is the actual
   reason to do this.
3. `block_object_duplicate` becomes cleanly interpretable: a collision now always means "these exact
   bytes were stored before," with no "same id, different content" case to disambiguate.

No CloudWatch fixture is affected: every fixture in `cloudwatch_logs_tests.rs` uses a non-zero
millisecond `timestamp`, and a negative one is rejected outright
(`negative_log_event_timestamp_is_parse_error`), so no existing fixture produces a record with both
`time_unix_nano` and `observed_time_unix_nano` at 0. `cloudwatch_logs.rs:173-174` maps
`timestamp * 1_000_000` into `time_unix_nano` with `observed_time_unix_nano: 0`, so only a literal
`timestamp: 0` would reach the substitution path — with the same observable result (the block's
`begin_time`) either way.

### Rollout order

Both parts land in one PR, so the source is always self-consistent. The constraint is at **deploy**
time, and only in split mode, where `telemetry-ingestion-srv` and `telemetry-maintenance-srv` are
separate processes that can run different builds during a rolling deploy:

> **Deploy the maintenance/analytics services before the ingestion service.**

Reversed, there is a window where new ingestion writes zero-timestamp payloads while an old
maintenance daemon is still building partitions, and that daemon drops every such record — silently,
aggregated into one log line (`nb_dropped_no_timestamp`). Deploying the ETL side first is close to
inert for *new* writes, but not strictly so for existing ones: OTLP logs ingestion shipped in `3f1cf089e`
(#1031) while the `observed_time_unix_nano` backfill only landed a month later in `17bb18505`
(#1124), so blocks written in that window contain records with both timestamps at 0. Those blocks are
only a few months old and can still be inside an operator's retention window
(`delete_old_data(min_days_old)`), so once commit 2 deploys, any such surviving block that gets
rebuilt starts yielding those previously-dropped rows at their arrival-time `begin_time` — a benign
row-count increase, not a hazard (their `begin_time` is arrival time, not 1970, per the #1031
fallback), but not a true no-op either. Monolith mode (`micromegas-monolith --roles all`) has no
window at all.

If the window is missed, it is recoverable but not self-healing: the affected records are in the
stored payloads and only missing from already-built partitions, so
`regenerate_partition_range` over the affected time range recovers them once the new processor is
live. Worth knowing, not worth planning around — ordering the deploy is cheaper.

This plan is also the resolution of a deployment gate already recorded in `CHANGELOG.md`: the
`## Unreleased` → `**Ingestion:**` entry for #1463 (landed in #1464 / commit `645d67286`, the CBOR
byte-string envelope encoding) says its ingestion-server rollout "waits on #1462, to avoid a
re-delivery window where a redelivered `block_id`'s payload object is overwritten with the smaller
encoding while `blocks.payload_size` keeps the stale, larger value." Commit 1's create-only write is
that unblocking change — once it ships, a redelivered `block_id` can no longer overwrite an existing
object with a differently-encoded body, so the #1463/#1464 rollout is no longer gated on this plan.
Commit 4 amends that CHANGELOG entry to say so (see item 11 below).

Existing blocks written after the #1124 backfill carry backfilled timestamps and keep taking the
`observed_time_unix_nano != 0` branch forever. Blocks from the #1031→#1124 window are the exception
(see above): they will start surfacing rows they previously dropped once commit 2 deploys and they
are rebuilt. There is no migration and no other mixed-state hazard beyond that deploy window.

## Implementation Steps

One PR. Commit order below is chosen so each commit is independently reviewable and so the
read-side change precedes the write-side change it enables — the same ordering the rollout needs.

**Commit 1 — create-only write (Part 1).**

1. `rust/telemetry/src/blob_storage.rs`: add `PutIfAbsent` enum and `put_if_absent`. Import
   `object_store::{PutMode, PutOptions}`.
2. `rust/ingestion/src/web_ingestion_service.rs:164-215`: switch the PUT to `put_if_absent`, keep the
   `put_duration` metric around it, fall through to the INSERT on `AlreadyExists`, and replace the
   `debug!` at `:203-205` with the four-case classification (three `imetric!` counters + one log
   line per outcome).
2b. `rust/analytics/src/replication.rs:168`: switch `ingest_payloads`'s block-payload PUT to
    `put_if_absent`, ignoring (or `debug!`-logging) the `AlreadyExists` outcome and continuing the
    stream — there is no INSERT to fall through to in this function; `ingest_blocks` is a separate
    function with its own independent `ON CONFLICT DO NOTHING` INSERT over a different stream, and
    needs no change.
3. `rust/telemetry/tests/`: unit tests for `put_if_absent` over `InMemory` — create, collide,
   original bytes preserved after a collision. `rust/telemetry/Cargo.toml` has no
   `[dev-dependencies]` section and no `tokio` anywhere today; add `tokio` (features `macros`, `rt`)
   as a `[dev-dependencies]` entry. Either add the tests to the existing `tests/blob_storage_tests.rs`
   (already registered via its own `[[test]]` block with `required-features = ["server"]`) or, if
   placed in a new file, give it its own `[[test]]` block with the same `required-features =
   ["server"]` — `object_store`/`bytes` are optional and server-gated, so a new test file without that
   entry would fail to build under default features.
4. `rust/ingestion/tests/insert_block_dedup_db_test.rs` (new, env-gated on
   `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI` like
   `rust/analytics/tests/*_db_test.rs`): the four-case matrix, including the orphan-heal path
   (write the object directly via `blob_storage`, then call `insert_block_typed` and assert one row
   appears).

**Commit 2 — read-side substitution (Part 2, must precede commit 3).**

5. `rust/analytics/src/lakehouse/otel/logs_block_processor.rs:100-111`: substitute
   `src_block.block.begin_time` for records with no timestamp; rename
   `nb_dropped_no_timestamp` → `nb_substituted_block_time` and adjust the aggregate log line at the
   end of the loop. Effectively inert for new writes until commit 3 ships, but see "Rollout order":
   surviving blocks from the #1031→#1124 window already have zero-timestamp records and will start
   yielding rows once this deploys and they are rebuilt.

**Commit 3 — single encode (Part 2).**

6. `rust/otel-ingestion/src/block.rs:288-345`: delete the backfill loop, the `nb_backfilled`
   counter, and the conditional re-encode; single `encode_to_vec`, hash and store the same bytes.
   Rewrite the `split_logs` doc comment (`:261-276`) — the "hashed from pre-backfill bytes" and
   "accepted tradeoff" paragraphs both go away.
7. `rust/otel-ingestion/src/handler.rs:212-235`: rewrite the `build_webhook_request` doc comment.
   Both timestamps still stay at 0; the reason changes from "so `split_logs` backfills" to "so the
   payload is byte-stable across redeliveries; the arrival-time fallback lives in the block bounds."
8. `rust/otel-ingestion/tests/split_tests.rs:232-241`: flip the assertion — the stored proto now
   keeps `observed_time_unix_nano == 0`. Keep the `:228-231` envelope-time assertions unchanged;
   `build_prepared_block` still stamps arrival time on the bounds.
   Also update `logs_split_mixed_timestamps_all_survive` (`:262-283`): with the backfill gone,
   `logs_bounds` no longer sees a stamped `now_nanos` for the untimestamped record, so `end_time`
   is now the same as `begin_time` — both equal `known_ts`. Replace the
   `assert!(b.end_time... > sentinel_ns)` check with `assert_eq!(b.end_time.timestamp_nanos_opt().unwrap(), known_ts as i64)`.
9. Add a test asserting `split_logs` on a timestamp-less request produces a payload whose
   `block_id` hash matches the stored bytes (`block_id_from_payload(&block.payload.objects) ==
   block.block_id`) — the invariant Part 1 enforces and Part 2 restores.

**Commit 4 — docs.**

10. `mkdocs/docs/otlp/index.md`: Idempotency section (create-only, first write wins, new counters)
    and the two webhook-specific "wrinkles" bullets (`:344-359`) — the second one (hash computed
    before backfill) no longer exists. See Documentation below.
11. `CHANGELOG.md`: add an entry under `## Unreleased` → `**Ingestion:**` covering the create-only
    block write, the three new counters (two `warn!`, one `debug!`), the hard failure on
    S3-compatible stores
    configured with `aws_conditional_put=disabled`, the "deploy maintenance/analytics before
    ingestion" rollout constraint, and the benign row-count increase for surviving #1031→#1124 blocks
    once rebuilt. Also amend the existing #1463 entry's **Deployment note** to say the create-only
    write in this plan is the change that unblocks its rollout (replacing "waits on #1462" with a
    pointer at the shipped fix), so the CHANGELOG no longer asserts an open gate that this PR closes.

## Files to Modify

| File | Change |
|---|---|
| `rust/telemetry/src/blob_storage.rs` | new `PutIfAbsent` + `put_if_absent` |
| `rust/ingestion/src/web_ingestion_service.rs` | create-only write, fall-through, 3 counters, 4-case log |
| `rust/analytics/src/replication.rs` | create-only block-payload write, fall-through to existing INSERT |
| `rust/analytics/src/lakehouse/otel/logs_block_processor.rs` | substitute block `begin_time` instead of dropping |
| `rust/otel-ingestion/src/block.rs` | delete backfill + dual encode; rewrite doc comments |
| `rust/otel-ingestion/src/handler.rs` | rewrite `build_webhook_request` doc comment |
| `rust/otel-ingestion/tests/split_tests.rs` | flip backfill assertions |
| `rust/telemetry/tests/` (new or existing) | `put_if_absent` unit tests over `InMemory` |
| `rust/ingestion/tests/insert_block_dedup_db_test.rs` | new env-gated integration test |
| `mkdocs/docs/otlp/index.md` | Idempotency + webhook wrinkles sections |
| `CHANGELOG.md` | `## Unreleased` → `**Ingestion:**` entry for the create-only write |

## Trade-offs

**Create-only vs. keeping the dual-encode fix as the primary fix.** The issue framed "hash the bytes
you store" as fix #1 and create-only as fix #3. Create-only is the load-bearing one: it enforces
*object bytes for a key never change* for **every writer into the block-object namespace** (ingestion
and, after 1b's `replication.rs` change, replication too) and every future mutation, including ones
nobody has written yet, whereas hashing-the-stored-bytes only fixes the one call site that currently
violates it. Part 2 is worth doing anyway (see its three benefits), but as hardening and cleanup on
top of a structural guarantee, not as the guarantee itself. Both ship together; the distinction
matters for how the code is commented and for what a reviewer should hold each part to.

**`head()` on the collision path to recover the true `payload_size`.** Considered and rejected, on
cost grounds: a mismatch only skews metadata (`payload_size`, and via it the `nb_tasks`
partition-sizing heuristic — see "What Part 2 buys" item 1), not readability, so paying one extra
round trip per duplicate arrival to keep that number exact is not worth it. Both cache layers pass
`head:true` straight through (`l1_store.rs:189-199`, `client.rs:1005-1013`), so it is viable if the
calculus ever changes. Part 2 achieves the same guarantee with no request at all for the common,
same-encoding case (see the cross-encoding exception noted above), so the round trip buys little even
then. Worth revisiting only if a future writer makes stored length depend on something outside
`block_id`.

**Silently falling back to `Overwrite` when the store lacks conditional put.** Rejected: it makes the
system quietly lose the invariant on exactly the deployments where an operator went out of their way
to disable conditional put. A loud startup/first-write failure with an actionable message is the
correct outcome.

**`warn!` on every duplicate arrival.** Accepted (see Part 1b). The alternative — `debug!` plus a
counter — is what shipped originally and is why #1462 sat unnoticed for two months.

**Out of scope, per the issue.** Repairing existing broken rows (blocks whose `payload_size`
disagrees with their object are unreadable; the affected entries are not needed) and stopping data
loss for producers that supply no event identity (unfixable ingestion-side — a retry and a recurrence
are byte-identical; tracked producer-side, protocol answer in #1466).

## Documentation

`mkdocs/docs/otlp/index.md` is the only affected doc page (see Commit 4 for the `CHANGELOG.md`
entry):

- **Idempotency** (`:222-224`): add that the object write is create-only, so a retried POST leaves
  the stored payload untouched — first write wins — and that both the object collision and the row
  conflict are counted (`block_object_duplicate`).
- **Webhook wrinkles** (`:344-359`): keep the first bullet (`block_id` folds in the full header set).
  Delete the second bullet ("the hash is computed before the server backfills the
  record's timestamp") and replace it with: the record's timestamps stay 0 in the stored payload, the
  arrival-time fallback lives in the block's `begin_time`/`end_time`, and the query layer surfaces
  `log_entries.time` from the block bounds for such records.
- The "Because no per-record timestamp is known, `time` is the server's ingestion wall-clock time"
  sentence (`:344-345`) stays true, but should say *the block's begin_time* rather than implying a
  value inside the record.

No new metrics doc page exists to update (`put_duration`/`insert_duration` are undocumented today;
`doc/design-presentation/design.md` mentions them only in passing).

## Testing Strategy

### Unit (no services required)

- `put_if_absent` over `InMemory`: `Created` on first write; `AlreadyExists` on second; the object
  still holds the *first* payload afterwards (the assertion that actually encodes the invariant).
  Requires adding `tokio` (features `macros`, `rt`) to `rust/telemetry/Cargo.toml`'s
  `[dev-dependencies]` (none exist today) and either landing in the existing
  `tests/blob_storage_tests.rs` or, if a new file, registering it with its own `[[test]]` block and
  `required-features = ["server"]` (see Commit 1, item 3).
- `split_logs` on a timestamp-less request: `block_id_from_payload(&block.payload.objects) ==
  block.block_id`, and `begin_time`/`end_time` still land at arrival time, not 1970.
- `split_logs` twice on the same timestamp-less request: identical `block_id` **and** identical
  `payload.objects` (today the second differs).
- Existing `split_tests.rs` / `webhook_tests.rs` / `identity_tests.rs` must stay green apart from two
  flipped assertions in `split_tests.rs`: the `observed_time_unix_nano` backfill check, and
  `logs_split_mixed_timestamps_all_survive`'s `end_time` check (now equal to `begin_time` rather than
  stretched to arrival time — see Commit 3 item 8).

### Integration, env-gated (`rust/ingestion/tests/insert_block_dedup_db_test.rs`)

The four-case matrix against a real PG + object store, following the harness pattern in
`rust/analytics/tests/net_spans_retire_overlap_db_test.rs`:

Assertions are on observable state (object bytes, row presence, `payload_size`), not on the
`imetric!` counters themselves: reading a named counter back out of `micromegas_tracing::test_utils`
requires `parse_block` + `StreamMetadata` from `micromegas-analytics`, which `micromegas-ingestion`'s
dev-deps cannot reach without a dependency cycle (`micromegas-analytics` already depends on
`micromegas-ingestion`). The precedent (`analytics/tests/metrics_test.rs:71-79`) only checks that a
metrics stream exists, not a counter's value; this plan doesn't go further than that.

1. Same block twice → one object, one row; the object's bytes equal the first write's.
2. Object pre-written directly, then `insert_block_typed` → row appears (orphan healed).
3. Row pre-inserted, no object, then `insert_block_typed` → object appears, and its bytes plus the
   row's `payload_size` are consistent with the second write.
4. Two blocks differing in one payload byte → two objects, two rows.

### End-to-end against the local test env

New test functions in `python/micromegas/tests/test_otlp_e2e.py`, reusing its existing
`WEBHOOK_ENDPOINT` and `log_entries` polling helpers (it already covers this exact surface, e.g.
`test_webhook_ingestion_e2e`, `test_webhook_ingestion_block_id_folds_in_full_header_set`,
`test_webhook_ingestion_missing_headers_tolerated`) rather than a standalone script:

1. POSTs the same webhook body twice to `/ingestion/webhook` with identical headers and asserts
   both requests succeed and exactly one row exists in the `blocks` view for that `block_id` — the
   row-level half of the #1462 regression, observable via SQL alone. `python/micromegas/tests/`
   has no object-store access (no `boto3`/`s3fs`/`fsspec` reference, no SQL surface returning block
   bytes), so the object-byte-equality half of the regression is asserted in the Rust env-gated
   integration test instead (see below), which already has `blob_storage` access.
2. Queries `log_entries` for the webhook record and asserts it is present (not dropped) with `time`
   equal to the block's `begin_time` — the Part 2 processor substitution. Requires the maintenance
   daemon to have built the partition, so run it after the ETL catches up (or force a partition
   build) rather than immediately after the POST.
3. POSTs two bodies differing only in a per-event attribute and asserts two blocks, both readable.

`micromegas-query "SELECT block_id, payload_size, begin_time FROM blocks WHERE block_id = '...'"`
covers the row-side assertions.
