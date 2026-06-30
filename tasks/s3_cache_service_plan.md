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
multiple origins without collisions.

`get_range(key, start..end)`:
1. Resolve `size` (cache → `origin.head`); store on miss.
2. Clamp `end = min(end, size)`; empty → empty `Bytes`.
3. Block indices `first..=last`; look up in backend.
4. Missing blocks: coalesce contiguous runs → one
   `origin.get_range(run_first*B .. min((run_last+1)*B, size))` each; split into
   blocks; insert.
5. Concatenate, slice to `[start, end)`, return.

`get_ranges` unions block indices across ranges, fills once, assembles each.

**Single-flight:** concurrent fetches of the same block coalesce via a
`moka::future::Cache` (the pattern `FileCache` already uses), so a cold row group
hits S3 once even under concurrent scans.

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
bucket is fixed per deployment, so the client sends only the key (URL-encoded).
No client auth in v1 (trusted network); a shared bearer token is an easy later
add. `#[micromegas_main]` tracing, health/readiness probe, graceful shutdown to
match existing services.

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

- `get` / `get_range` / `head` → HTTP to the binary; on any transport/5xx error,
  fall back to `direct` (so a cache outage degrades to direct-S3 reads).
- `get_ranges` → one `POST /obj/{key}/ranges` call; parse the length-framed
  response back into `Vec<Bytes>`. Single round trip for the Parquet
  footer + row-group batch.
- `put` / `delete` / `list` / multipart / `copy` → delegate to `direct`. Writes
  never touch the cache; no write surface in the binary.

### Wiring (keeps `reqwest`/cache deps out of `telemetry`)

1. `BlobStorage` gains, in `telemetry`, a closure-injection constructor (no new
   dependency):
   ```rust
   pub fn connect_with_layer(
       url: &str,
       layer: impl FnOnce(Arc<dyn ObjectStore>) -> Arc<dyn ObjectStore>,
   ) -> Result<Self>;
   ```
   `connect(url)` becomes `connect_with_layer(url, |s| s)`.
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

Clients (no code change beyond wiring): `MICROMEGAS_OBJECT_CACHE_URL` — unset ⇒
direct S3 (current behavior).

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
   `range-cache`, tracing/telemetry).
6. Build origin `AmazonS3` from `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI`.
7. `GET`/`HEAD`/`POST …/ranges` handlers → `RangeCache`; parse `Range` /
   JSON ranges, emit `206`/`Content-Length`/length-framed bytes.
8. Health/readiness, `#[micromegas_main]`, graceful shutdown.

### Phase 3 — `range-cache-client` + wiring
9. Create `rust/range-cache-client/`; `CacheClientStore: ObjectStore` (reqwest
   reads + fallback; write delegation).
10. Add `BlobStorage::connect_with_layer`; refactor `connect` to use it.
11. Build the layer from env in `connect_to_data_lake` /
    `connect_to_remote_data_lake`; add ingestion dep on `range-cache-client`.

### Phase 4 — Deploy, verify, document
12. Container + deployment manifest (SSD volume); point `flight-sql-srv` and the
    daemon at the cache Service.
13. Tests (below); docs.

### Phase 5 — In-process consolidation (later, optional)
14. Replace `FileCache`/`CachingReader` with an in-process `RangeCache` (RAM
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
- Cache pod holds a read-scoped IAM role; treat as a credentialed service.
- In-cluster plaintext HTTP is acceptable behind the Service; client→cache auth
  is optional in v1 (trusted network). The binary must not be exposed outside the
  cluster without adding a shared token. SSD holds cached object bytes — same
  sensitivity as S3; don't share the volume across tenants.

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
- **Client store tests:** reads hit a fake/real cache server; **writes go to the
  inner store, not HTTP** (assert via a counting inner store); cache-unreachable
  → reads fall back to inner.
- **End-to-end:** run the binary + `flight-sql-srv` pointed at it against the
  local stack; `poetry run pytest` (results identical); cache-hit metrics rise on
  repeated queries.
- `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.

## Open Questions
1. **Crate / binary names:** `range-cache`, `object-cache-srv`,
   `range-cache-client` — confirm (`micromegas-` prefixes).

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
- **Batch `ranges` optimizations** and a client→cache auth token.
