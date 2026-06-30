# Feasibility: range-aware S3 caching proxy (#1122)

## Goal

A transparent, in-cluster, **range-aware** cache for object-store reads, shared
across `flight-sql-srv` (and any other service that reads partitions). The
stated UX requirement: **consumers stay on the S3 protocol and only change a
URL** — they should not know the cache exists.

Two driving forces:

1. **#1121 / retiring the PostgreSQL footer cache.** Once `partition_metadata`
   is removed, every cold miss falls back to a Parquet footer range-read from
   S3. A shared range cache makes those misses cheap and shared across
   processes (today's in-process caches don't survive restarts and aren't
   shared between `flight-sql-srv` replicas). The user has confirmed this
   direction: *"eventually, we'd retire the footer cache in psql."*
2. **Row-group reuse.** Parquet reads are all range requests. A range cache
   accelerates repeatedly-scanned row groups, not just footers — for hot
   dashboards this can eliminate most S3 traffic.

## Current architecture (what "change the URL" touches)

Every reader path funnels through **one** construction point:

- `BlobStorage::connect(object_store_url)` (`rust/telemetry/src/blob_storage.rs`)
  calls `object_store::parse_url_opts(url, env)` and wraps the result in a
  `PrefixStore`. The URL comes from `MICROMEGAS_OBJECT_STORE_URI`.
- Consumers (`LakehouseContext`, `WebIngestionService`, monolith) all go
  through `connect_to_data_lake` → `BlobStorage::connect`.
- `object_store = "0.13"` with the `aws` feature; DataFusion `54.0`.

Existing cache layers (all **in-process**, lost on restart, not shared between
replicas):

- `FileCache` (`file_cache.rs`): moka LRU, **whole-object** caching of files
  ≤ 10 MB, 200 MB budget, thundering-herd coalescing via `try_get_with`.
- `CachingReader` (`caching_reader.rs`): for files ≤ 10 MB it fetches the whole
  object and slices ranges locally; for larger files it bypasses the cache and
  does `get_range`/`get_ranges` straight to S3. **So today there is no caching
  of individual ranges for large files** — exactly the gap #1122 targets.
- `MetadataCache`: cached Parquet metadata keyed by partition.
- DataFusion's built-in `FileMetadataCache` (DF 50+): last ~512 KB footer +
  statistics/page index, in-memory.

Key takeaway: because all reads pass through a single `Arc<dyn ObjectStore>`
built from one URL, micromegas has **two** clean insertion points for a shared
cache — at the S3 wire (a proxy) or at the `object_store` layer (a decorator).

## The crux of feasibility: SigV4 and "transparent"

The hard part of an S3-protocol proxy is **not** the cache — it's authentication.
SigV4 signs the `Host` header, the canonical path, the query string, and a set
of headers. The signature is bound to the host the client signed for.

This makes "fully transparent + proxy holds no credentials" essentially
contradictory. The realistic options:

| Model | Proxy holds creds? | Truly transparent? | Notes |
|---|---|---|---|
| **A. Re-sign at proxy** | Yes (its own IAM role) | Client only changes the **endpoint URL**; keeps `s3://bucket/...` | Proxy terminates the client request, validates/ignores client auth, and re-signs to the backing store with its own pod IAM role. Simple, robust. **Relaxes** the "no creds in proxy" want. |
| **B. Signature passthrough** | No | Only if client signs for the *real* S3 host but connects to the proxy | Requires decoupling "host to sign" from "host to connect to" (DNS redirection / VPC endpoint), plus a TLS cert the client trusts for the AWS hostname. Fragile; the AWS sample's "passthrough" leans on this kind of routing. |
| **C. MITM with private CA** | No (forwards bytes) | Yes, but needs trust config | Proxy presents a cert from a private CA the clients trust; terminates TLS, forwards the verbatim signed request to S3. Operationally heavy. |

**Conclusion on transparency:** in our own cluster, where we control the
`object_store` config, **Model A is the pragmatic "just change the URL" answer.**
`object_store`'s S3 client already supports a custom endpoint
(`AWS_ENDPOINT_URL` / `with_endpoint`, plus `allow_http` for in-cluster plaintext).
Consumers keep `s3://<bucket>/...` and we only point the endpoint at the proxy.
The "no credentials in the proxy" goal from the issue is not achievable
*together with* full S3 transparency without network/TLS gymnastics — and it
isn't worth much here, since an in-cluster proxy can carry a scoped IAM role
just like the services do today.

## Two viable architectures

### Option A — S3-protocol caching proxy (the literal ask)

A standalone service speaking enough of the S3 GET API (`GET` with `Range`,
`HEAD`) to serve `object_store`'s read path. Deploy like MinIO/LocalStack:
in-cluster, services set `AWS_ENDPOINT_URL=http://s3-cache:9000`.

- **Pros:** language/runtime agnostic; benefits *any* S3 client (future tools,
  Grafana, ad-hoc queries), not just micromegas Rust code. Matches the issue
  exactly.
- **Cons:** must correctly implement S3 GET/Range semantics (partial content
  `206`, multi-range, conditional headers, error XML), handle SigV4 (Model A),
  and own the maintenance. A second network hop and a new always-on service to
  operate, scale, and monitor.
- **Build-on-the-sample path:** `aws-samples/sample-s3-hybrid-cache` (Rust,
  v2.0.0 June 2026) is the closest starting point — range-aware, SigV4-aware,
  real test suite — but explicitly a *sample*, not supported, minimal adoption.
  Adopting it means owning a fork.

### Option B — `object_store`-layer caching decorator (recommended first step)

Wrap the `Arc<dyn ObjectStore>` returned by `parse_url_opts` in a
`CachingObjectStore` decorator that intercepts `get_range`/`get_ranges`/`get`
and consults a **shared** cache backend before hitting S3. Selected by URI
scheme, e.g. `MICROMEGAS_OBJECT_STORE_URI=cached+s3://bucket/...` parsed in
`BlobStorage::connect`.

- **Pros:** ~all the value (shared, range-aware, survives restarts) with a
  fraction of the risk. No S3 protocol surface to reimplement, no SigV4
  handling, no second hop for cache *hits*. Reuses the existing thundering-herd
  pattern. Naturally replaces the `CachingReader` large-file bypass gap. Ships
  incrementally.
- **Cons:** only benefits **micromegas's own Rust consumers** — not transparent
  to external S3 clients. Still needs a shared cache backend service.
- **"Change the URL" still holds** for our services (scheme prefix), which is
  what actually matters for #1121/#1122.

Both options need the same backend decision below.

## Cache design (common to A and B)

- **Key:** `(object_path, aligned_byte_range)`. Align ranges to fixed blocks
  (e.g. 1 MB) so overlapping reads share entries and the key space is bounded;
  serve sub-ranges by slicing. Footers are tiny and pin well; row groups map to
  a handful of blocks.
- **Immutability is our superpower.** Partitions are write-once Parquet objects
  that are only ever *deleted*, never mutated. So cached ranges never go stale
  while the object exists — **no revalidation / ETag dance needed** on hits.
  Invalidation = the object was retired; a TTL plus accepting rare misses on
  deleted objects is sufficient.
- **Thundering herd:** reuse the moka `try_get_with` coalescing already proven
  in `FileCache`.
- **Backend choices:**
  - *S3 Express One Zone* — ~2–5 ms, durable-enough, no service to run; per-GB
    cost + AWS dependency. Good fit for the footer cold-miss layer in #1121.
  - *Valkey/Redis* — sub-ms, shared across replicas, automatic eviction;
    memory-bound, another stateful service.
  - *Foyer (hybrid mem+disk)* — what the AWS sample and OpenDAL FoyerLayer use;
    local NVMe gives large capacity cheaply but is per-node unless fronted by
    the proxy.

## Recommendation

1. **Do Option B first.** It delivers the shared, range-aware, restart-surviving
   cache that #1121 needs (footer cold-miss layer) and that #1122 wants (row-group
   reuse), at low risk, entirely within our Rust stack, behind a URI-scheme
   change. It directly closes the current `CachingReader` large-file gap.
2. **Keep Option A on the table** only if/when a non-Rust or external S3 consumer
   needs the same cache, or if we want a single shared on-disk cache fronting all
   replicas. If we go there, **build on `aws-samples/sample-s3-hybrid-cache`**
   rather than from scratch, and accept the maintenance ownership. Use **Model A
   (proxy re-signs with its own IAM role)** — full transparency without proxy
   credentials is not realistically achievable.
3. **Sequence with #1121:** land the decorator + shared backend, point the
   cold-miss footer read at it, *then* drop `partition_metadata`. That keeps a
   persistent cheap-miss layer in place before the psql cache is retired.

## Feasibility verdict

- **Option B: clearly feasible, low risk.** Insertion point already exists
  (`BlobStorage::connect`), patterns exist (`FileCache`/`try_get_with`),
  immutable objects remove the cache-coherence hard part. Estimated small-to-
  medium effort: decorator + range-block keying + backend client + config
  plumbing + tests.
- **Option A (true S3 proxy): feasible but medium-to-high risk**, dominated by
  S3 API fidelity and SigV4, not the cache. The "no credentials in the proxy"
  requirement is the part that does *not* hold up; everything else does. Best
  approached as a fork of the AWS sample, not a clean-room build.

## Open questions

- Backend: S3 Express vs Valkey vs Foyer-on-NVMe? (cost vs latency vs ops)
- Block alignment size — tune to typical row-group sizes in our partitions.
- Is any **non-Rust** S3 consumer in scope? If no, Option A's main advantage
  disappears and B is sufficient indefinitely.
