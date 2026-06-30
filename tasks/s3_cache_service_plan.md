# Object Range Cache Service Plan

## Overview

Build a standalone, **range-aware** read cache as a separate process, shared by
multiple `flight-sql-srv` instances and the maintenance daemon. It caches
arbitrary byte ranges keyed by `(path, range)` on **local SSD** (Foyer hybrid
RAM+disk), fetching from S3 on miss. Clients reach it over a **minimal internal
HTTP range protocol — not the S3 protocol** (no SigV4, no S3 API surface, no
write semantics): the binary only answers "give me bytes of key X, range Y" and
"what's the size of key X".

Reads route through the cache; **writes bypass it** (delegated straight to S3 by
the client), so the binary stays read-only and simple. The range-caching logic
is factored into a reusable core library so the *same* code can later back an
in-process `ObjectStore` decorator with no rewrite.

This unblocks retiring the PostgreSQL `partition_metadata` footer cache (#1121):
footer cold-misses hit a cheap SSD-backed cache instead of Postgres.

### Why this shape (decision trail)

- **Separate process, not in-process:** the cache is shared across many
  `flight-sql-srv` replicas and the daemon — a shared warm cache is the whole
  point, and in-process can't provide it.
- **No Redis:** the cache is disposable state over immutable objects; a
  networked/replicated store isn't worth it. Each binary instance keeps its own
  local SSD, **stateless, never synced**.
- **No S3 protocol:** transparency (any S3 client, pure URL swap) was the only
  reason to speak S3, and it drags in SigV4 + the entire write API. Since our
  clients are our own Rust code, a tiny HTTP range protocol does the job with
  none of that cost.
- **Writes bypass the cache:** write sites are few; the client delegates
  `put`/`delete`/`list` to a direct-S3 store, so the binary needs no write path.

### Local SSD vs S3 Express One Zone (why cache at all)

S3 Express One Zone narrows the gap to S3 Standard but a local-SSD cache is still
~1–2 orders of magnitude faster per read and carries no per-request fee, which is
what makes a read-through cache worthwhile even in front of Express. Approximate
characteristics (order-of-magnitude, not benchmarks):

| Characteristic        | Local NVMe SSD                     | S3 Express One Zone                          |
|-----------------------|------------------------------------|----------------------------------------------|
| Read latency (per op) | ~50–150 µs                         | ~1–5 ms first byte (single-digit ms)         |
| Throughput            | multiple GB/s per device           | very high aggregate, scales horizontally; per-connection bounded |
| IOPS / request rate   | 10⁵–10⁶ IOPS, no request charge    | high, but billed per request (GET/PUT)       |
| Cost model            | included in instance (no per-op)   | per-request + per-GB data + single-AZ storage |
| Durability            | ephemeral — lost on stop/terminate | durable within one AZ                         |
| Sharing               | per-pod, not shared                | shared, networked                            |

Implication: the SSD tier absorbs hot/repeat reads at µs latency with zero
per-request cost; S3 (Standard or Express) remains the durable, shared backing
store hit only on miss. The cache complements Express rather than competing with
it — Express lowers cold-miss latency, the SSD removes it on hits.

## Current State

All lake access funnels through one object store via `blob_storage.inner()`:

- `rust/telemetry/src/blob_storage.rs` — `BlobStorage::connect(url)` calls
  `object_store::parse_url_opts` and wraps the result in `PrefixStore`. The
  reader path retrieves the store via `BlobStorage::inner()`.
- `connect_to_data_lake` / `connect_to_remote_data_lake`
  (`rust/ingestion/src/data_lake_connection.rs:24`,
  `rust/ingestion/src/remote_data_lake.rs:43`) build `BlobStorage`; used by
  analytics (`LakehouseContext`), monolith, ingestion-srv.

Existing **in-process** caches are kept and become an L1 in front of the shared
cache: `FileCache` (whole files ≤10 MB), `CachingReader`, `MetadataCache`, and
DataFusion's `FileMetadataCache`. For files >10 MB, `CachingReader` already
bypasses the in-process cache and issues `get_range`/`get_ranges`
(`caching_reader.rs:100-141`) — those now land on the shared cache, closing the
current large-file gap.

Partitions/payloads are **write-once** (created or deleted, never mutated, paths
never reused), so cached ranges never go stale — no ETag/revalidation needed.

Workspace already has `axum 0.8`, `reqwest 0.12` (rustls), `bytes`, `moka`,
`async-trait`, `tokio`. New dep: `foyer` (hybrid RAM+SSD cache).

## Design

### Three crates

Package names take the `micromegas-` prefix (lib convention; binary matches
`micromegas-monolith`); directories are unprefixed:
`micromegas-range-cache` (`rust/range-cache/`),
`micromegas-object-cache-srv` (`rust/object-cache-srv/`),
`micromegas-range-cache-client` (`rust/range-cache-client/`).

```
rust/range-cache/          (core lib — reusable, no HTTP)
  RangeCache { origin: Arc<dyn ObjectStore>, backend, block_size, ns }
    get_range / get_ranges / size   (block model, single-flight, fallback)
  RangeCacheBackend (trait) + FoyerBackend (RAM+SSD) + MemoryBackend (tests)

rust/object-cache-srv/     (binary — the shared cache process, built now)
  axum HTTP server over RangeCache:
    GET  /obj/{key}  (Range: bytes=a-b)  -> 206 + bytes
    HEAD /obj/{key}                      -> Content-Length = size
    POST /obj/{key}/ranges  (JSON ranges)-> length-framed concatenated bytes
  origin = object_store AmazonS3 -> real S3;  backend = Foyer on local SSD

rust/range-cache-client/   (client lib — used in flight-sql + daemon)
  CacheClientStore { http: reqwest client -> object-cache-srv,
                     direct: Arc<dyn ObjectStore> (real S3) } : impl ObjectStore
    reads  (get/get_range/get_ranges/head) -> HTTP; fall back to `direct` on error
    writes (put/delete/list/...)           -> delegate to `direct`
```

The cache logic (`RangeCache` + Foyer) lives only in `range-cache` and is used
by the binary now. An eventual in-process decorator reuses the *same*
`RangeCache` directly (backend = Foyer or RAM), no protocol — that is the reuse
contract.

### RangeCache core (block model)

Reads align to fixed-size blocks so overlapping reads share entries and the key
space is bounded. Entries:

- size: `meta:{ns}:{key}` → `u64`. Immutable; loaded once via `origin.head`.
- block: `blk:{ns}:{key}:{i}` → bytes of `[i*B, min((i+1)*B, size))`; last block
  may be short.

`{ns}` namespace (default derived from origin bucket/prefix) lets one SSD hold
multiple origins without collisions. `{key}` is always a **full bucket-relative
key** (including the lake root prefix): the client injects the cache layer
*inside* `PrefixStore` (see Wiring), so keys reaching the cache are not
prefix-relative and match the binary's origin and prefix validation.

`get_range(key, start..end)`:
1. Resolve `size` (cache → `origin.head`); store on miss.
2. Clamp `end = min(end, size)`; empty → empty `Bytes`.
3. Block indices `first..=last`; look up in backend.
4. Missing blocks: coalesce contiguous runs → one
   `origin.get_range(run_first*B .. min((run_last+1)*B, size))` each; split into
   blocks; insert.
5. Concatenate, slice to `[start, end)`, return.

`get_ranges` unions block indices across ranges, fills once, assembles each.

**Fill policy (patchwork reads).** A single large read over a partially-cached
range yields *several* missing contiguous runs interleaved with cached blocks.
The runs are independent, so **fetch them concurrently** — wall-clock latency is
then the slowest single GET, not the sum, and only the genuinely missing bytes
are fetched (cached blocks are never refetched). Two bounds keep this sane: a
**max coalesced GET size** so one wide run doesn't become a giant fetch (split
oversized runs, also fetched concurrently), and a **per-request concurrency cap**
(semaphore) so a heavily fragmented patchwork doesn't open hundreds of
simultaneous connections. Gap-tolerant merging (refetch a small cached gap to
join two runs into one GET) is *not* the primary lever — concurrency already
removes the latency cost of separate runs; it's a measure-and-tune guard only for
pathological fragmentation where the simultaneous-connection / S3 per-prefix
request rate, not latency, is the limit.

**Single-flight:** concurrent fetches of the same block coalesce via a
`moka::future::Cache` (the pattern `FileCache` already uses), so a cold row group
hits S3 once even under concurrent scans. A run-GET resolves the per-block
single-flight entries for *every* block it covers (not the run as an opaque
unit), so two concurrent large reads over overlapping patchworks still dedupe at
the block level rather than each issuing its own run fetch.

**Graceful degradation (required):** any backend error is a miss + a metric, and
the read falls back to origin. A read never fails because the cache is down.

### FoyerBackend (local SSD, disposable)

`foyer::HybridCache<String, Bytes>` provides RAM+SSD tiering, admission, and
byte-weighted eviction in one component. The SSD is a **scratch volume** — never
backed up, never synced; on restart with an empty volume the cache re-warms from
S3. `MemoryBackend` (`Mutex<HashMap>`) is the deterministic test backend behind
the same trait.

### Cache binary (object-cache-srv)

axum server, plaintext HTTP in-cluster. Two read routes only (`GET` with `Range`,
`HEAD`); plain HTTP status codes for errors (404 missing, 5xx), no S3 XML. Origin
bucket is fixed per deployment (`s3://bucket`, no prefix), so the client sends
only the key (URL-encoded), which is the **full bucket-relative key** (the client
injects its layer inside `PrefixStore`, so the lake root prefix is already part
of the key — see Wiring). The handler validates the key against the allowed lake
prefix (`MICROMEGAS_OBJECT_CACHE_PREFIX`) and rejects
empty / `..` / leading-`/` / out-of-prefix keys, so the binary can't be turned
into a general bucket proxy. Caller authentication reuses the existing
`micromegas-auth` `ApiKeyAuthProvider` and the axum `auth_middleware` already
used by telemetry-ingestion-srv (bearer key, named keyring, constant-time
compare) — a drop-in layer, not "trusted network only";
see Security. `#[micromegas_main]`
tracing, health/readiness probe, graceful shutdown to match existing services.

The `ranges` endpoint takes a JSON body `{"ranges": [[start,end], ...]}` and
returns a **length-framed** response: for each requested range, in order, an
8-byte little-endian length followed by that many bytes (actual length reflects
EOF clamping). This handles many ranges and arbitrary sizes without HTTP header
limits or `multipart/byteranges` parsing. The handler is a direct call to
`RangeCache.get_ranges`, which already coalesces at the block level — so a
footer + N row-group fetch becomes one HTTP round trip and the minimum set of S3
fetches.

### Client store (CacheClientStore)

An `ObjectStore` impl wired in at `blob_storage.inner()` via a layer closure
(below). Holds the direct real-S3 store it wraps plus a reqwest client to the
cache binary.

- `get_opts` (the required read method; `get`/`get_range`/`head` are default
  impls that delegate to it) is **the** read interception point. It honors the
  incoming `GetOptions` (including the byte `range`) and routes to the binary
  over HTTP; on any transport/5xx error, fall back to `direct` (so a cache
  outage degrades to direct-S3 reads). Overriding `get_opts` ensures every read
  path goes through the cache rather than just the convenience methods.
- `get_ranges` (overridden) → one `POST /obj/{key}/ranges` call; parse the
  length-framed response back into `Vec<Bytes>`. Single round trip for the
  Parquet footer + row-group batch.
- Required write/metadata methods — `put_opts`, `put_multipart_opts`,
  `delete` / `delete_stream`, `list`, `list_with_delimiter`, `copy`,
  `copy_if_not_exists` — delegate to `direct`. Writes never touch the cache;
  no write surface in the binary.

### Wiring (keeps `reqwest`/cache deps out of `telemetry`)

1. `BlobStorage` gains, in `telemetry`, a closure-injection constructor (no new
   dependency):
   ```rust
   pub fn connect_with_layer(
       url: &str,
       layer: impl FnOnce(Arc<dyn ObjectStore>) -> Arc<dyn ObjectStore>,
   ) -> Result<Self>;
   ```
   `connect(url)` becomes `connect_with_layer(url, |s| s)`. **Layer ordering:**
   the layer wraps the **raw** parsed store *inside* `PrefixStore` —
   `PrefixStore::new(layer(blob_store), blob_store_root)` — so the cache client
   sees **full bucket-relative keys** (including the lake root prefix), matching
   the binary's origin (`s3://bucket`, no prefix) and its prefix validation.
2. `connect_to_data_lake` / `connect_to_remote_data_lake` (ingestion crate,
   depends on `range-cache-client`) build the layer from env: if
   `MICROMEGAS_OBJECT_CACHE_URL` is set, wrap the store in `CacheClientStore`;
   else identity. One change covers flight-sql, daemon, monolith, ingestion;
   per-process env decides who uses the cache.

The static-tables store built directly via `parse_url_opts` in
`static_tables_configurator.rs:72` is out of scope (tiny static files).

### Deployment (stateless, no sync)

- Each cache pod: binary + an SSD volume (local NVMe / SSD-backed PV). N
  replicas behind a Service; no coordination, no replication.
- Pod carries the IAM role and origin S3 config. Clients set
  `MICROMEGAS_OBJECT_CACHE_URL` at the Service.

### Configuration (env)

Cache binary:
- `MICROMEGAS_OBJECT_CACHE_LISTEN` — e.g. `0.0.0.0:8080`.
- `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI` — real S3, e.g. `s3://bucket` (origin
  client uses standard AWS creds/role from env).
- `MICROMEGAS_OBJECT_CACHE_RAM_MB` — Foyer in-memory budget.
- `MICROMEGAS_OBJECT_CACHE_DISK_PATH`, `MICROMEGAS_OBJECT_CACHE_DISK_GB` — SSD.
- `MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE` — default `1048576` (1 MiB).
- `MICROMEGAS_OBJECT_CACHE_NAMESPACE` — default derived from origin.
- `MICROMEGAS_OBJECT_CACHE_PREFIX` — allowed key prefix; reject out-of-prefix
  keys (default: whole bucket).
- `MICROMEGAS_API_KEYS` — JSON keyring (same convention as flight-sql and
  telemetry-ingestion-srv) for the `ApiKeyAuthProvider` behind the axum
  `auth_middleware`; unset ⇒ auth disabled (trusted-network/test only).

Clients: `MICROMEGAS_OBJECT_CACHE_URL` — unset ⇒ direct S3 (current behavior);
`MICROMEGAS_OBJECT_CACHE_API_KEY` — bearer key attached on outbound requests to
the cache.

## Implementation Steps

### Phase 1 — `range-cache` core lib
1. Create `rust/range-cache/`; add `foyer` to workspace `Cargo.toml`
   (alphabetical); crate deps (`object_store`, `bytes`, `async-trait`, `moka`,
   `micromegas-tracing`, `anyhow`).
2. `blocks.rs`: range↔block math + assembly (pure, heavily unit-tested).
3. `backend.rs` trait; `memory_backend.rs`; `foyer_backend.rs`.
4. `range_cache.rs`: `RangeCache` (algorithm, single-flight, fallback, metrics).

### Phase 2 — `object-cache-srv` binary
5. Create `rust/object-cache-srv/` (`axum`, `tokio`, `object_store`,
   `range-cache`, `micromegas-auth`, tracing/telemetry).
6. Build origin `AmazonS3` from `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI`.
7. `GET`/`HEAD`/`POST …/ranges` handlers → `RangeCache`; parse `Range` /
   JSON ranges, emit `206`/`Content-Length`/length-framed bytes. Validate the
   key against the allowed lake prefix (reject empty / `..` / leading-`/` /
   out-of-prefix) before serving.
8. Add the existing `micromegas-auth` `ApiKeyAuthProvider` + axum `auth_middleware`
   (the same pairing telemetry-ingestion-srv uses) as an axum layer (keyring from
   `MICROMEGAS_API_KEYS`) on the `/obj` routes; exempt health/readiness. Add the
   `micromegas-auth` dep.
9. Health/readiness, `#[micromegas_main]`, graceful shutdown.

### Phase 3 — `range-cache-client` + wiring
10. Create `rust/range-cache-client/`; `CacheClientStore: ObjectStore` (reqwest
    reads + fallback; write delegation). Attaches `MICROMEGAS_OBJECT_CACHE_API_KEY`
    as a bearer token on outbound requests.
11. Add `BlobStorage::connect_with_layer`; refactor `connect` to use it.
12. Build the layer from env in `connect_to_data_lake` /
    `connect_to_remote_data_lake`; add ingestion dep on `range-cache-client`.

### Phase 4 — Deploy, verify, document
13. Container + deployment manifest (SSD volume); point `flight-sql-srv` and the
    daemon at the cache Service.
14. Tests (below); docs.

### Phase 5 — In-process consolidation (later, optional)
15. Replace `FileCache`/`CachingReader` with an in-process `RangeCache` (RAM
    backend) reusing the same core, for DRY. Deferred — not required to ship the
    service.

## Files to Modify / Create
- **New** `rust/range-cache/`, `rust/object-cache-srv/`,
  `rust/range-cache-client/`.
- `rust/Cargo.toml` — add `foyer` (alphabetical) + the three path deps.
- `rust/telemetry/src/blob_storage.rs` — `connect_with_layer`.
- `rust/ingestion/Cargo.toml` — dep on `range-cache-client`.
- `rust/ingestion/src/{data_lake_connection,remote_data_lake}.rs` — wire layer.
- Deployment manifests / container build for the cache binary.
- `CLAUDE.md`, `AI_GUIDELINES.md`, `mkdocs/docs/` — service + env vars +
  SSD/stateless deployment notes.

## Trade-offs
- **Separate binary vs in-process.** A shared process gives multiple flight-sql
  replicas + the daemon one warm cache and decouples cache capacity from service
  pods; cost is a process to operate and a network hop on hits (in-cluster,
  cheap). In-process can't share across replicas — rejected for this reason.
- **Local SSD vs Redis.** Disposable state over immutable objects doesn't justify
  a replicated store; per-pod SSD is simpler, cheaper, statelessly scalable. Cost:
  each pod warms its own SSD (bounded, and mitigated by hash routing below).
- **HTTP range protocol vs gRPC vs S3.** Plain HTTP range GET is the minimum that
  does the job: no proto codegen/ops weight (gRPC), no SigV4/write API/XML (S3).
  Our clients are our own code, so transparency buys nothing.
- **Writes bypass the cache.** Few write sites + client-side delegation to direct
  S3 keeps the binary read-only — removing the hardest part (transparent write
  re-signing) entirely.
- **Reusable core.** `RangeCache` over `Arc<dyn ObjectStore>` is protocol- and
  deployment-agnostic, so the in-process decorator later is a thin wrapper.
- **Block size 1 MiB.** Footer granularity vs key/round-trip count; tunable.

## Security

The cache is a **confused-deputy risk**: it holds S3 read credentials and will
return the bytes of any key it's asked for. Anyone who can reach it and name a
key reads that slice of the lake, bypassing every check FlightSQL makes. The
asset to protect is the *credentialed read path*, not the SSD. Defense in depth
means *independent* layers, so no single failure — a leaked token, an SSRF in a
neighboring pod, a compromised replica — exposes the lake.

### Why a security group isn't enough
A security group authenticates *network position*, not the *request*: "this
packet came from an allowed SG," not "this is flight-sql making a legitimate
read." It therefore falls to anything that can act from an allowed host — a
compromised flight-sql/daemon pod, an SSRF or request-forgery bug in any service
on the allowed SG, a malicious sidecar, a pod that later reuses the SG. Keep it
(it's necessary), but it's one positional layer, not the whole story.

### What the cache can and can't enforce
Per-user data authorization can't live here: the daemon has no end user, and by
the time a request is "key X, bytes a–b," FlightSQL's table/row decision is
already made — the cache has no basis to re-decide it. So **user-level authz
stays in FlightSQL, where it already lives.** Trying to "follow FlightSQL auth"
into the cache buys nothing for the daemon and duplicates an upstream decision.
The cache's narrower job: authenticate that the *caller is one of our services*,
and *contain the blast radius* of the credentials it holds.

### Layers (each independent)
1. **Network position.** Restrict ingress at the orchestrator's security group
   so only the flight-sql and daemon services can reach the cache; keep it in
   private subnets and never publish it through a public load-balancer listener.
   (Note: a platform may still assign an instance a public IP for egress — the
   ingress security group, not the IP, is the control that matters.) Necessary,
   not sufficient (see above).
2. **Request-level API key (the key addition).** On top of the SG, require a
   bearer API key per request, so the control survives a pivot or a header-less
   SSRF from an allowed host and distinguishes cache clients from the rest of the
   coarse shared SG. **Reuse the existing `micromegas-auth` `ApiKeyAuthProvider`
   + axum `auth_middleware`** already used by telemetry-ingestion-srv
   (`rust/public/src/servers/ingestion.rs`) — a named **keyring**
   (list of keys) compared in constant time, bearer token, `401` on failure, the
   key *name* (not the key) logged for audit. Same `MICROMEGAS_API_KEYS` JSON
   keyring convention as the other services; flight-sql and the daemon each
   present a key. Because the checker holds a *list*, rotation is zero-downtime —
   add the new key, roll the clients, drop the old — so the key needs no expiry.
   This does **not** defend against full compromise of an allowed caller (the
   attacker reads the key and already holds that task's stronger read/write IAM
   role); that exposure is bounded by layer 4, not here.
3. **Blast-radius containment at the cache.** The binary is structurally
   read-only — no put/delete/list code path exists — so even a trusted-then-
   compromised caller can only *read ranges*, never mutate or enumerate.
   Additionally the cache serves only keys under the configured lake prefix
   (reject empty keys, `..`, leading `/`, out-of-prefix keys), so it can't be
   turned into a general proxy for the rest of the bucket.
4. **Blast-radius containment at the object store (highest-leverage lever).** The
   cache's role is read-only — `GetObject` only, no Put/Delete/List — and scoped
   to the single lake bucket/prefix. This is *strictly narrower* than the
   read/write/delete role the lake query services run with today, so a fully
   compromised cache yields less reach than compromising those services: no
   writes, no deletes, no enumeration, read-only on exactly the lake data.
5. **Audit (detection).** Log (authenticated caller identity, key, range, bytes
   served) and emit metrics; optionally forward the FlightSQL end-user identity
   as an *advisory* header for the audit trail only — never for an authz
   decision.

### v1 posture
Layers 1, 3, and 4 are mandatory for v1 — all cheap and structural, and layer 4
in particular is the single biggest risk reduction (read-only, scoped role).
Request-level API key (layer 2) via the existing `micromegas-auth` middleware is
included in v1 — a drop-in layer, not new machinery. Identity-level schemes
(mTLS, VPC Lattice/IAM) are deferred as over-engineered for this threat model:
given the read-only scoped role (layer 4) and that compromising a caller already
yields stronger credentials than the cache holds, a rotatable shared API key
behind the SG is the right altitude.

### Data at rest
SSD holds cached object bytes — same sensitivity as the lake. It's a scratch
volume (not backed up, not synced); don't share it across tenants, and enable
volume encryption-at-rest where the platform offers it cheaply.

## Testing Strategy
- **`blocks.rs` units:** single/multi-block, boundary-spanning, partial last
  block, empty range, range past EOF (clamping).
- **`range_cache` tests** (`MemoryBackend` over a counting wrapper around
  `object_store::memory::InMemory`): bytes equal direct origin reads across many
  random ranges; cold read = one origin fetch per missing block, warm = zero;
  partial coverage doesn't re-fetch seeded blocks; erroring backend still yields
  correct bytes (graceful degradation); N concurrent identical reads → one origin
  fetch (single-flight).
- **Foyer integration:** SSD cache in a tempdir serves hits; gated behind a
  feature/env flag if it needs real disk.
- **Binary protocol tests:** drive `object-cache-srv` (origin = `InMemory`) with
  reqwest; assert `GET`+`Range` → `206` correctness, `HEAD` size, and that
  `POST …/ranges` round-trips a multi-range request (framing decodes to the same
  `Vec<Bytes>` as per-range reads, including an EOF-clamped final range).
- **Auth tests:** with a keyring configured, a request with no / wrong bearer key
  → `401`; a valid key → served; an out-of-prefix key request → rejected; the
  health/readiness probe needs no key.
- **Client store tests:** reads hit a fake/real cache server; **writes go to the
  inner store, not HTTP** (assert via a counting inner store); cache-unreachable
  → reads fall back to inner.
- **End-to-end:** run the binary + `flight-sql-srv` pointed at it against the
  local stack; `poetry run pytest` (results identical); cache-hit metrics rise on
  repeated queries.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.

## Open Questions
- None blocking. Crate/binary names resolved:
  `micromegas-range-cache`, `micromegas-object-cache-srv`,
  `micromegas-range-cache-client`.

## Deferred / Future
- **In-process `FileCache` consolidation — follow-up.** `FileCache` /
  `CachingReader` stay exactly as they are for this work and become an in-process
  L1 in front of the shared cache. Replacing them with an in-process `RangeCache`
  (RAM backend) on the reusable core is a separate follow-up, out of scope here.
- **Multiple cache instances + LB routing.** v1 runs a **single cache instance**
  (sufficient for the current use case), so there is no routing/hit-rate concern
  yet. When scaling to multiple instances later, add consistent-hash-by-key
  routing so each node owns a key slice (better aggregate hit rate and SSD use)
  rather than round-robin. No code change in the cache or client — purely a
  deployment concern.
- **Multi-object footer prefetch (ties into #1121) — deferred, not now.** Not a
  transparent `ObjectStore` optimization: DataFusion's read path is strictly
  per-file (`ParquetFileReaderFactory::create_reader` is per `PartitionedFile`,
  `AsyncFileReader`/`ObjectStore::get_ranges` are per-object), so it would never
  call a cross-object endpoint. The opening is explicit prefetch from the query
  layer, which already resolves the full partition set before the scan: footers
  are read for every file regardless of pruning, so one batched call warms all
  partition footers (the tail range of N keys) in a single round trip — letting
  the per-file readers hit a warm cache and helping retire the Postgres
  `partition_metadata` footer cache. Requires a multi-key extension of the
  `/ranges` endpoint plus a query-layer prefetch hook. (Row-group data prefetch
  is not worth it — ranges are runtime-pruning-dependent, already covered by
  within-file block coalescing + DataFusion's cross-file concurrency.)
- **Identity-level caller auth (mTLS / VPC Lattice + IAM).** Deferred — see the
  Security v1 posture; only if the shared API key proves insufficient.
