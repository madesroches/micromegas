# Crypto-based data isolation for telemetry ("strong" plan)

> **Alternative to** [`policy_based_data_isolation_plan.md`](policy_based_data_isolation_plan.md), not a
> supersession. Both are candidate designs; this document is the stronger-threat-model option and has
> not been chosen over the policy plan. It **reuses that plan's vocabulary** — `audience`, `ReadScope`,
> `ReadPolicy`/`MintPolicy`, the `user:`/`group:` value shape — and where it needs query-path access
> control on cleartext data it **layers on top of** that plan rather than replacing it. Where they
> differ: the policy plan rests confidentiality entirely on OIDC + a per-query filter over *plaintext*
> parquet; this plan additionally makes the **bytes at rest useless without keys**.

## Motivating threat

The policy plan states plainly (its §5): *"an operator with lakehouse/object-store access can read the
raw parquet directly."* That is the assumption this plan removes. **Assume the parquet files and raw
blocks comprising the lakehouse leak** (public-bucket misconfig, a copied backup, an RMA'd disk, a
stolen snapshot). Can we make that leak useless? For **bodies** (log message text, metric values):
yes. For **metadata** and against a **compromised server**: only partially, and this plan is explicit
about which.

### Threat model — three rows, defended differently

| Threat | Policy plan | This plan |
|---|---|---|
| Storage/backup/bucket leak — attacker holds ciphertext only | **not defended** (plaintext at rest) | **defended** for bodies (encrypted); metadata still readable |
| Authenticated user reading another audience via SQL | defended (ReadPolicy per query) | defended (same ReadPolicy; for bodies, key-gating adds defense in depth) |
| Compromised ingestion / query / maintenance server (holds keys transiently) | not defended | not defended by default; **partially** defended if KMS enforces caller→audience unwrap |

The central, non-negotiable fact: **a server-side SQL engine must hold plaintext to run aggregations**,
so crypto cannot *replace* the query-path access control — it composes with it. What crypto buys is
row 1 (the storage leak) and a start on row 3.

## Design overview

Two structural moves, then encryption:

1. **Split the lakehouse into two planes.**
   - **Metadata plane** — `processes`, `streams`, `blocks`, `log_stats` and derivatives. Cleartext,
     globally materialized, queried freely by DataFusion. This is the index. It carries **no bodies**:
     block metadata is `(process_id, stream_id, insert_time, nb_objects, object_offset, payload_size)`
     (`rust/ingestion/src/sql_telemetry_db.rs:73`, `blocks_view.rs:153-175`), and the payload is located
     in object storage by `block_id`/`object_offset`, which point at *encrypted* payloads. `log_stats` is
     derived from `log_entries` but only ever emits `count(*)` grouped by `process_id`/`level`/`target`/
     `time_bin` (§1) — counts, not message bodies — so it belongs here, not in the body plane.
   - **Body plane** — `log_entries`, `measures` (the only two **body** view sets with a global instance;
     confirmed spans have none — see §1 for the metadata-side views that also have one), plus the **raw
     block payloads** they derive from. Encrypted, single-audience, only ever reached audience- or
     process-addressed.
2. **Make every body artifact single-audience** by replacing the all-audience `'global'` body instance
   with **global-per-audience** instances (§2). Once no artifact mixes audiences, each file is
   encryptable under exactly one audience key.
3. **Envelope-encrypt the body plane** under a per-audience KEK held in a KMS (§3–4). A leaked body
   file is ciphertext whose DEK is wrapped by a key the attacker doesn't have.

The metadata plane stays cleartext at rest, so **the policy plan's query-path filtering (Prong A on
the `processes` view, Prong B on `list_partitions`) is still required for metadata** — this plan does
not make it redundant, it narrows what it has to protect (§6).

## §1 — Two-plane split, grounded

- The body view sets are exactly `log_entries` and `measures`: they are the only **body** view sets
  pushed into `global_views` with a real `get_update_group()` (`log_view.rs:221-227`,
  `metrics_view.rs:225-231`). They are not the only *entries* in `global_views` with one — the
  metadata/stats views also return a real update group: `processes` (`Some(2000)`), `streams`
  (`Some(2000)`), `blocks` (`Some(1000)`), and `log_stats` (`Some(3000)`, `log_stats_view.rs:69`,
  `view_factory.rs:295-301,316`). What distinguishes the body pair is that they carry message/measure
  content, not that they're uniquely materialized. Every span view set returns
  `get_update_group() -> None` and has no global instance (`thread_spans_view.rs:353`,
  `net_spans_view.rs:368`, `async_events_view.rs:215`, `otel/spans_view.rs:200`); `process_spans` is a
  UDTF, never materialized. **So the body-plane change is contained to two view sets** plus the
  raw-block write/read seams.
- `log_stats` is a global, all-audience aggregate derived from `log_entries`, so it needs an explicit
  classification rather than riding along unclassified. Its transform query
  (`log_stats_view.rs:30-38`) groups by `process_id, level, target, time_bin` and emits only
  `count(*)` — no `msg`/log-body column is read or aggregated. It carries counts/rates, not content, so
  it classifies as **metadata-plane**: cleartext, globally materialized like `processes`/`streams`/
  `blocks`, no per-audience instance, no encryption. (If a future version of this view ever folds in
  message content rather than just counts, it would have to move to the body plane like `log_entries`.)
- The raw block payloads are the *source* the body views materialize from (both the `'global'` and
  per-process paths derive from the `blocks` view — `partition_source_data.rs:266-267`,
  `jit_partitions.rs:393-401`). They contain the actual events, so **they are body-plane and must be
  encrypted at the ingestion write seam** (§4), independently of the materialized parquet.

## §2 — Global-per-audience via instance-id overload

`view_instance_id` is already a free-form key that today holds a `process_id`, a `stream_id`, or the
literal `'global'` (`view.rs:56`). Add a **fourth kind: an audience** (`user:<email>` / `group:<id>`).
`view_instance('log_entries', 'group:teamA')` materializes only teamA's bodies into a single-audience,
teamA-key-encrypted partition set.

**Per-process body instances are kept (decided, not open).** `view_instance('log_entries', <pid>)` /
`view_instance('measures', <pid>)` are load-bearing across the web app for process drilldown
(`analytics-web-app/src/routes/ProcessLogPage.tsx`, `ProcessMetricsPage.tsx`,
`routes/perf-analysis/queries.ts`, `hooks/useMetricsData.ts`,
`lib/screen-renderers/notebook-utils.ts`, plus their tests) — dropping them would break process
drilldown, so the codebase decides this, not a design preference. Keeping them costs nothing new here:
each `Process` instance is already single-audience (a process belongs to exactly one audience) and
encryptable exactly like an `Audience` instance, with its KEK derived via `process → audience` (§4).

**a. Constructor seam (the main code change).** `LogView::new`/`MetricsView::new` currently do a binary
`if id == "global" { None } else { Uuid::parse_str(id)? }` (`log_view.rs:80-93`,
`metrics_view.rs:82-95`) — an audience string hard-errors on the UUID parse. Replace the
`process_id: Option<Uuid>` field with:

```rust
enum InstanceKind { Global, Process(Uuid), Audience(String) }
```

Classification is unambiguous: `"global"` → `Global`; a `user:`/`group:`-prefixed string → `Audience`;
otherwise parse as a `Process(Uuid)`. The `user:`/`group:` prefix (already mandated by the policy plan
to prevent user/group collisions) is exactly what makes this parse-free to disambiguate. The same
UUID-parse assumption lives in the span makers (`thread_spans_view.rs:86`) but they gain no audience
kind, so they're untouched.

**b. Source-filter seam.** The per-process materialization scopes its source with a single-value
predicate `WHERE process_id = '{id}'` (`jit_partitions.rs:266-275`, and the range probe at `:406-413`).
For an `Audience(A)` instance this becomes a semi-join / `IN` against the processes belonging to A:

```sql
process_id IN (SELECT process_id FROM processes
               WHERE property_get(properties,'micromegas.audience') = '{A}')
```

There is no process→audience mapping table today, so v1 reuses the `micromegas.audience` **property**
(exactly as the policy plan's v1 does); Phase 5 promotes it to a column for pruning. This is the one
place that needs a "set of processes" abstraction the single-`process_id` path lacks.

**v1 re-scans `blocks` filtered by audience** (this is a decided point, not an open fork) — it matches
how the `'global'` instance works today, at the cost of some duplicate scanning if per-process instances
also exist for the same data (same duplication pattern as today). Merging existing single-audience
per-process partitions instead would avoid the double scan but needs new view-from-partitions machinery
that doesn't exist today; that's a future optimization, not v1.

**c. Materialization is two-tier: pinned (scheduled) + JIT (lazy).**

- **Pinned tier — a config-declared finite set of audience instances, materialized continuously.** An
  operator lists the audiences worth keeping warm (typically the important groups); the daemon
  materializes their `log_entries`/`measures` instances on the existing cron cadence so they are always
  ready with low query latency. The daemon's materialization *loop* is reused unchanged
  (`materialize_all_views` → `materialize_partition_range` over a rolling insert range,
  `maintenance.rs:30-66`) — it already iterates arbitrary `Arc<dyn View>` objects. What changes is the
  *set of views handed to it*: it must include the config-derived audience instances, constructed via
  `make_view("group:teamA")` at startup. `get_update_group()` is currently `if view_instance_id ==
  "global" { Some(...) } else { None }` (`log_view.rs:221-227`, `metrics_view.rs:225-231`), and the
  daemon's scheduled set is exactly the views for which this returns `Some`
  (`get_global_views_with_update_group`, `maintenance.rs:271-278`) — so `get_update_group()` must be
  extended to also return `Some(...)` for `Audience(A)` instances, not only `"global"`, or pinned
  instances silently fall out of `is_some()` and never get scheduled.
- **JIT tier — every other audience, lazy.** Instances not pinned spring into existence on first query
  via `MaterializedView::scan -> jit_update` (`materialized_view.rs:69-72`), the same path per-process
  instances use today, kept fresh query-driven. `jit_update`'s trait signature takes no process/audience
  argument — it's `jit_update(&self, lakehouse, query_range: Option<TimeRange>)` (`view.rs:76-80`) and
  each view resolves its own instance internally (as `thread_spans_view.rs:269-295` already does for
  per-process instances). So the `Audience` handling is added **inside** `LogView`/`MetricsView`'s
  `jit_update` implementation, keyed off `self.get_view_instance_id()`'s `InstanceKind`, not by changing
  the trait signature. This is the pay-per-use path that keeps the "thousands of self-mode audiences"
  case cheap (nothing pinned by default).
- **The two tiers coexist safely on one instance:** the per-instance advisory lock
  (`write_partition.rs:232-245` hashes the instance id) serializes scheduled and query-time writes, and
  the JIT up-to-date check (`jit_partitions.rs:472-558`) makes a query-time `jit_update` on an
  already-warm pinned instance a cheap no-op — so JIT doubles as a **catch-up safety net** if the
  scheduler falls behind.

**The structural change this forces (the part "no scheduler change" glossed over):** today
`get_global_views()` serves *two* roles — the daemon's scheduled set
(`get_global_views_with_update_group`, `maintenance.rs:271-278`) **and** bare-table registration on the
query path (`query.rs:214`). Pinned audience instances belong in the first but **not** the second — a
bare `SELECT * FROM log_entries` must be the ReadScope union of §2d, never a specific pinned audience.
So **decouple the two sets**: a `scheduled_instances()` (metadata globals + pinned audience instances)
for the daemon, distinct from the bare-registered globals for the query path. The materialization loop
itself is untouched; only the construction of the scheduled set becomes config-driven and decoupled
from bare registration.

**Config:** `MICROMEGAS_PINNED_AUDIENCES` (JSON array of strings, default empty), parsed the same way
as its precedent — `MICROMEGAS_ADMINS` is `serde_json::from_str::<Vec<String>>` over the env var, not
comma-separated (`oidc.rs:265-266`). Empty → pure JIT (the self-mode default; nothing eagerly
materialized, fail-cheap). v1 applies the list to both body view sets (`log_entries`, `measures`) as a
flat list, read once at startup like `MICROMEGAS_ADMINS` (`load_admin_users`, `oidc.rs:264-269`, no
reload path); per-`(view_set, audience)` granularity and hot-reload are a future improvement, not v1
(see Open forks for the one-line note).

**d. Bare-name resolution → union rewrite.** `SELECT * FROM log_entries` (bare) binds to the
pre-built `'global'` instance today (`query.rs:214` iterates `get_global_views()`;
`view.rs:90-99` registers the bare name). With no all-audience global body instance, the bare name must
resolve to the **union of the caller's readable audience instances**:
`log_entries → UNION ALL over view_instance('log_entries', A) for A ∈ ReadScope`. Implement as an
analyzer rewrite (fits alongside the policy plan's Prong A) or a custom table provider for the bare
`log_entries`/`measures` names. This is the main *new* query-planning piece.

**e. Fate of the `'global'` body instance.** Retire the persisted all-audience `'global'` instance for
`log_entries`/`measures` — it is precisely the mixed-audience artifact we set out to delete, and it
cannot be encrypted under a single audience key. If the maintenance daemon (`ReadScope::All`) ever
needs true platform-wide body analytics, compute them **transiently** by scanning the per-audience
instances — never persist a mixed-audience body file. `'global'` stays only for the cleartext metadata
views (`processes`/`streams`/`blocks`).

**f. Enforcement simplifies for body scans.** Because an `Audience(A)` instance *is* single-audience,
the policy plan's Prong A semi-join over the mixed global `log_entries` collapses to an O(1) check on
the scan: **allow `view_instance('log_entries', A)` iff `A ∈ ReadScope`, else empty** — no
`property_get`, no per-row join, no in-partition filtering. The expensive semi-join existed only to
cope with the mixed artifact.

## §3 — Envelope encryption of the body plane

- **KEK per audience**, held in a KMS/Vault; never leaves the KMS. **DEK per file** (AES-GCM),
  wrapped by the audience KEK, wrapped-DEK stored in the file's key metadata. Leaked file = ciphertext
  + a DEK you can't unwrap.
- **Materialized parquet** uses **Parquet Modular Encryption (PME)** — parquet-rs's native
  column/footer AES-GCM with a KMS-client key-retrieval abstraction that is built for exactly this
  double-wrapping model. AES-GCM also gives **tamper-evidence** (a modified-and-reinjected file fails
  auth), which incidentally hardens the write-key integrity concern from the policy plan.
- **Raw block payloads** are (likely) a custom transit format, not parquet, so PME does not apply —
  envelope-encrypt the block object under the same audience KEK at the ingestion write path.
- **Rotation:** rotate the KEK (re-wrap DEKs, cheap); avoid DEK rotation (would re-encrypt data).
- **Fan-out cost:** cache unwrapped DEKs in memory, mirroring the immutable `process_id → audience`
  cache the policy plan already specifies. Cold miss = one KMS unwrap (~ms); warm = O(1).

## §4 — Key seams (where encryption/decryption actually wire in)

- **Write / encrypt (materialized parquet):** `write_partition.rs` already knows the
  `view_instance_id` (it builds `views/{view_set}/{instance_id}/{date}/…parquet` at `:546-552`). For an
  `Audience` instance the audience is the instance id; for a `Process` instance derive it via
  `process → audience`. Select that audience's KEK and hand PME the wrapped DEK. Metadata-plane views
  (`Global` metadata) get no encryption. **Sharp edge:** the instance id becomes an object-store path
  segment through `Path::parse` (`write_partition.rs:557`). `:` and `@` are not actually a hazard here —
  `object_store` 0.13.2's `Path::parse` (`path/parts.rs`) only rejects `/`, whole-segment `.`/`..`, and
  ASCII control chars; the reserved-character set (`% { } # [ ] < > | \ " * ?` …) is enforced by the
  encoding `From` impl, not by `parse`, so `user:alice@example.com` parses and round-trips fine. The
  genuine hazards are embedded `/`, `.`/`..` segments, control chars, `%`/`{}`, and cross-backend/OS
  portability (S3 tooling, Windows) — mainly for arbitrary `group:` ids. **v1 constrains the audience
  charset to path-safe at the MintPolicy/ingestion boundary** (where audience ids are already prefixed
  and validated) rather than percent-encoding at the path builder — `percent-encoding` is already a dep
  of `rust/auth` but not of `rust/analytics`, so charset-constraining at the boundary avoids adding that
  dependency to the analytics crate and is the lower-surprise choice. The advisory-lock key hashes the
  instance id (`write_partition.rs:232-245`) and is fine for any string.
- **Write / encrypt (raw blocks):** the HTTP ingestion handlers (`rust/public/src/servers/ingestion.rs`,
  `otlp.rs`) are thin shims — the actual object-store write is one choke point, deeper down:
  `rust/ingestion/src/web_ingestion_service.rs::insert_block_typed` (~line 165), which CBOR-encodes
  `block.payload` and `put`s it to `blobs/{process_id}/{stream_id}/{block_id}`. Both native and OTLP
  ingestion funnel through `WebIngestionService`, so thread `bound_audience` (per the policy plan's
  key model) down to `insert_block_typed` and encrypt the payload under that audience's KEK there,
  rather than in the handler files.
- **Read / decrypt:** `MaterializedView::scan` is where partitions are read and already carries the
  per-request `ReadScope` (the policy plan threads it there). Configure the parquet reader's
  decryption key-retriever here; raw-block reads (during JIT materialization) decrypt via the same
  KEK. **ReadScope-gated unwrap:** only request the DEK for a partition whose audience `∈ ReadScope`.
  A caller naming another audience's process/instance → the key is never fetched → the ciphertext is
  useless *even to the query server, for that request*. This is the row-3 upgrade: it starts to
  constrain a **compromised** query server — but only to the extent the **KMS itself** enforces
  caller→audience (Vault identity-scoped transit keys / per-request grants). With a blanket
  server unwrap role you get row-1 protection only.

## §5 — Where the TCB actually sits

Encryption concentrates trust rather than eliminating it — state this plainly:

- **Ingestion** receives plaintext over the wire and must encrypt → holds the audience KEK for what it
  stamps. Already trusted with the plaintext it receives.
- **Maintenance daemon** materializes the *pinned* audience instances (§2c) → over time unwraps every
  pinned audience's KEK to encrypt on write (one at a time; never mixes two audiences in one artifact).
  Only under a non-empty `MICROMEGAS_PINNED_AUDIENCES` is this "the residual full-trust root" for those
  audiences. A leak of its KMS credentials = total compromise of the pinned set — but that is a far
  tighter, more defensible surface than "the object store."
- **Query server** is not decrypt-only: under the JIT default (`MICROMEGAS_PINNED_AUDIENCES` empty, §2c)
  it is the one that runs `MaterializedView::scan -> jit_update` for unpinned audience instances
  (`materialized_view.rs:69-72`), and JIT materialization writes encrypted parquet via
  `write_partition.rs` — so the query server also needs **encrypt** (write-KEK) access, not just
  decrypt, for every audience a request touches. It is therefore itself a materialization trust root for
  those audiences, on top of decrypting what it serves. Both encrypt and decrypt access are gated by
  ReadScope (and, for real row-3 strength, by the KMS). Only in the fully-pinned-set limit does the
  query server become purely decrypt-only and the daemon the sole encrypt-side trust root.
- **Ceiling (out of scope for v1):** run the query/maintenance engine in a **TEE / confidential VM**
  (Nitro Enclaves, SEV-SNP) so keys live only inside an attested enclave and the *operator* can't peek
  at the transient plaintext. This closes row 3 fully; heavy (attestation, ops). North star, not v1.

## §6 — Composition with the policy plan (not a replacement)

- **Body plane:** protected by keys + the simplified `A ∈ ReadScope` instance check (§2f). Defense in
  depth: even a query-path enforcement bug can't leak bodies whose key was never unwrapped.
- **Metadata plane:** cleartext at rest, so it **still needs the policy plan's query-path filtering** —
  Prong A on the `processes` view and Prong B row-filtering `list_partitions` — or an authenticated
  user (and a storage leak) sees every audience's process names, host names, and volume/timing. This
  plan therefore *includes* the policy plan's enforcement for metadata; it only removes the need for
  the body-view semi-join.
- **`retire_partitions`/`materialize_partitions`:** unchanged from the policy plan — maintenance-only,
  `ReadScope::All`-gated. Note `materialize_partitions` takes no instance-id arg today
  (`materialize_partitions_table_function.rs:47-49`); if the daemon ever needs to pre-warm a specific
  audience instance it must gain one, but JIT (§2c) means v1 doesn't require it.

## §7 — Bonus: crypto-shredding for erasure

Because each audience's body plane is encrypted under one KEK, **"delete an audience's data" = destroy
its KEK**. The ciphertext bytes can stay where they are and become permanently unreadable, no bulk
re-scan. `retire_partitions` is already per-`(view_set, instance_id)`, so retiring the metadata plane
entries rides existing machinery. This is a cheap, strong GDPR/offboarding erasure story the policy
plan can't offer.

## Honest limits

- **Metadata cleartext at rest.** Process/host/owner names, `otel.resource.*` properties, and
  per-process event counts/sizes/timing are readable from a metadata-plane leak. The last set is a
  volume/activity **side channel** — and it is in tension with the effort the policy plan's §4 spends
  row-filtering `list_partitions` to hide exactly that from authenticated callers. The at-rest and
  query-time postures should be a conscious pair. If metadata sensitivity matters, column-encrypt the
  process `properties` (and owner/host) — a further step, not v1.
- **PostgreSQL is a separate leak.** Metadata rows live in PG; "parquet leak useless" ≠ "storage leak
  useless" until PG is covered (TDE / column encryption) or declared trusted.
- **Daemon full-trust** (§5) — unavoidable without a TEE.
- **Load shifts daemon → query time** for bodies (JIT), first-query latency per audience/range.
- **Bare-table semantics change** — `SELECT * FROM log_entries` becomes "your audiences' union"; an
  API/UX change (the access-control behavior already matches the policy plan's semi-join).

## Parquet Modular Encryption + DataFusion read integration (confirmed supported)

This is not an open feasibility gate: the workspace's pinned versions (`parquet = "58.0"`,
`datafusion = "54.0"`, `rust/Cargo.toml`) already support encrypted parquet reads first-class.
`parquet` 58 has a stable (non-experimental) `encryption` feature (Parquet Modular Encryption, pulling
in `ring`), and DataFusion 54 has a `parquet_encryption` feature plus an `EncryptionFactory` trait
registered on `RuntimeEnv` via `register_parquet_encryption_factory`, along with
`TableParquetOptions.crypto` — exactly the KMS-retriever seam this plan needs. **Action for v1:** enable
`parquet/encryption` and `datafusion/parquet_encryption` (note the added `ring` build dependency) and
implement a bring-your-own `EncryptionFactory` that resolves the audience KEK and unwraps the DEK. No
custom `TableProvider` fallback is needed — the engine reads/writes encrypted parquet transparently
through the standard scan/write paths.

## Code seams / files

- **Instance-id overload:** `rust/analytics/src/lakehouse/log_view.rs`,
  `metrics_view.rs` (`InstanceKind` enum, constructor classification, `jit_update` audience branch);
  `rust/analytics/src/lakehouse/jit_partitions.rs` (source filter → audience semi-join);
  `rust/analytics/src/lakehouse/view_factory.rs` (retire `'global'` body pre-build; maker changes;
  **decouple `scheduled_instances()` from the bare-registered `get_global_views()`**; construct pinned
  audience instances from `MICROMEGAS_PINNED_AUDIENCES`).
- **Scheduler wiring:** `rust/public/src/servers/maintenance.rs` (feed the daemon the decoupled
  `scheduled_instances()` set instead of `get_global_views_with_update_group` alone); config parsing
  next to the isolation-policy factory.
- **Bare-name union rewrite:** `rust/analytics/src/lakehouse/query.rs` (bare-table registration) +
  an analyzer rule or custom provider.
- **Encryption write seam:** `rust/analytics/src/lakehouse/write_partition.rs` (KEK selection, PME
  wiring, path-segment encoding); `rust/ingestion/src/web_ingestion_service.rs::insert_block_typed`
  (the single object-store put covering both native and OTLP ingestion) for raw-block encryption —
  thread `bound_audience` down from the HTTP handlers (`rust/public/src/servers/ingestion.rs`,
  `otlp.rs`) to this choke point rather than encrypting in the handlers themselves. Note also the
  separate `rust/otel-ingestion` crate.
- **Decryption read seam:** `rust/analytics/src/lakehouse/materialized_view.rs` (key-retriever,
  ReadScope-gated unwrap).
- **KMS/DEK cache:** new module mirroring `metadata_cache.rs` (moka).
- **Shared with policy plan:** `ReadScope`/`ReadPolicy`, audience value shape, `bound_audience`,
  Prong A/B for the metadata plane.

## Open forks (undecided)

1. **KMS policy enforcement** (blanket server unwrap role vs. identity-scoped per-caller unwrap) —
   decides whether row 3 of the threat table is defended at all.
2. **Metadata column-encryption** — whether to also encrypt sensitive process properties, closing the
   metadata side channel, or accept cleartext metadata at rest.

(Decided for v1, not open: audience-global source is re-scan-`blocks`, not merge — future optimization:
merge per-process partitions once view-from-partitions machinery exists, §2b. Audience-id path encoding
is charset-constraining at the MintPolicy boundary, not percent-encoding, §4. Pinned-set granularity is
a flat list applied to both body view sets, startup-only like `MICROMEGAS_ADMINS`, §2c — future: per-
`(view_set, audience)` granularity + hot-reload.)

## Relationship summary

| | Policy plan | This (strong) plan |
|---|---|---|
| Confidentiality root | OIDC + per-query filter over plaintext | + per-audience keys; bodies useless without KMS |
| Parquet/block leak | readable | bodies ciphertext; metadata readable |
| Body query-path enforcement | semi-join over mixed global | O(1) `A ∈ ReadScope` on single-audience instance |
| Metadata query-path enforcement | Prong A + Prong B | **same** (still required) |
| Compromised server | not defended | partially, iff KMS enforces caller→audience |
| Erasure | retire partitions | + crypto-shred (destroy KEK) |
| Cost | none at runtime | KMS unwraps (cached), pinned + JIT materialization, PME read integration |
