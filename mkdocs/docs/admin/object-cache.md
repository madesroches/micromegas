# Object Cache Deployment

`micromegas-object-cache-srv` is a shared HTTP range cache that sits in front of the data lake's object store. Split-mode services (FlightSQL, the maintenance daemon, ingestion) read overlapping byte ranges of the same Parquet/block files; routing those reads through one shared cache avoids re-fetching the same bytes from S3/GCS on every process, cutting egress cost and read latency.

It only caches **reads**. Writes, deletes, and listings always go straight to the origin store — see [What gets cached](#what-gets-cached) below.

## Quick start with the local helper script

```bash
python3 local_test_env/ai_scripts/start_minio.py
```

Starts a local MinIO container as an S3-compatible origin, creates a test bucket, and launches the rest of the services with the cache wired in front of it. `--no-launch` sets up MinIO only; `--monolith` forwards through to monolith mode. See `local_test_env/ai_scripts/stop_minio.py` for teardown.

This is the only way to exercise the cache locally: it requires a bucket-style origin (`s3://`/`gs://`), and `start_services.py`'s default `file://` lake can't provide one.

## Quick start with Docker

```bash
docker run -d -p 8080:8080 \
  -e MICROMEGAS_OBJECT_CACHE_ORIGIN_URI=s3://my-bucket \
  -e MICROMEGAS_OBJECT_CACHE_DISK_PATH=/data \
  -e MICROMEGAS_API_KEYS='[{"name":"flight-sql","key":"<random>"}]' \
  -v object-cache-data:/data \
  marcantoinedesroches/micromegas-object-cache:latest
```

## Origin URI must be bucket-only

`MICROMEGAS_OBJECT_CACHE_ORIGIN_URI` must have **no path component** — `s3://my-bucket`, not `s3://my-bucket/lake-root`. The lake-root prefix already arrives inside each request key (clients wrap the cache client *inside* their own `PrefixStore`), so a path on the origin would be applied twice and produce silent 404s. The server refuses to start if it parses a non-empty prefix out of the origin URI.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI` | Yes | Bucket-only origin (`s3://bucket`, `gs://bucket`) |
| `MICROMEGAS_OBJECT_CACHE_DISK_PATH` | Yes | Local disk path for the on-disk cache tier |
| `MICROMEGAS_API_KEYS` | Yes, unless `--disable-auth` | JSON array of `{"name":"...","key":"..."}` |
| `MICROMEGAS_OBJECT_CACHE_LISTEN` | No | Bind address (default `0.0.0.0:8080`) |
| `MICROMEGAS_OBJECT_CACHE_RAM_MB` | No | In-memory cache tier size (default `512`) |
| `MICROMEGAS_OBJECT_CACHE_DISK_GB` | No | On-disk cache tier size (default `50`) |
| `MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE` | No | Cache block size in bytes (default `1048576`); must be > 0 |
| `MICROMEGAS_OBJECT_CACHE_NAMESPACE` | No | Cache namespace (default: derived from the origin URI) |
| `MICROMEGAS_OBJECT_CACHE_PREFIX` | Yes | Allowed key prefixes, comma-separated (e.g. `blobs,views`); only keys equal to or under a prefix are served. Required unless `--allow-all-prefixes` is set (development only) |
| `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | No | Drain timeout on `SIGTERM` (default `25`) |
| `MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES` | No | Total concurrent origin GETs (default `32`; NIC-sized starting point, tune against measurement) |
| `MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES` | No | Origin-GET slots always available to demand reads; prefetch is capped at `total - reserved` (default `8`); must be less than `MAX_CONCURRENT_FETCHES` |
| `MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES` | No | Max span of one coalesced run GET, in bytes (default `8388608`, 8 MiB); larger contiguous runs are split at block boundaries |
| `MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB` | No | Cross-request cap on concurrent in-flight streaming windows, in MiB (default `1024`); must be at least the fixed per-stream window's size, or the server refuses to start |
| `MICROMEGAS_OBJECT_CACHE_PROMOTE_WHOLE_BATCH` | No | On a demand hit into a prefetch batch, promote the whole batch (anticipatory) instead of only the covering run (default `false`, precise) |
| `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` | No | Depth of the bounded `POST /prefetch` queue; items beyond this are load-shed (default `4096`); must be > 0 |
| `MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY` | No | Concurrent in-flight prefetch fills driven by the queue worker (default `8`); must be > 0 |

Authenticating *against the origin* (e.g. AWS credentials) uses the same environment variables as every other Micromegas service's `MICROMEGAS_OBJECT_STORE_URI` — standard `object_store` crate variables such as `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ENDPOINT`, `AWS_REGION`, `AWS_ALLOW_HTTP`.

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--listen` | `0.0.0.0:8080` | Bind address |
| `--origin-uri` | — | Bucket-only origin URI (required) |
| `--disk-path` | — | Local disk path for the cache (required) |
| `--ram-mb` | `512` | In-memory cache tier size |
| `--disk-gb` | `50` | On-disk cache tier size |
| `--block-size` | `1048576` | Cache block size in bytes |
| `--namespace` | derived from origin | Cache namespace |
| `--prefix` | none | Restrict served keys to this prefix (repeatable) |
| `--disable-auth` | off | Disable authentication (development only) |
| `--shutdown-grace-period-seconds` | `25` | Seconds to drain before hard exit on `SIGTERM` |
| `--max-concurrent-fetches` | `32` | Total concurrent origin GETs |
| `--demand-reserved-fetches` | `8` | Origin-GET slots reserved for demand reads |
| `--max-coalesced-get-bytes` | `8388608` | Max span of one coalesced run GET, in bytes |
| `--memory-budget-mb` | `1024` | Cross-request memory budget, in MiB |
| `--promote-whole-batch` | `false` | Promote a whole prefetch batch (not just the covering run) on a demand hit |
| `--prefetch-queue-capacity` | `4096` | Depth of the bounded `POST /prefetch` queue |
| `--prefetch-worker-concurrency` | `8` | Concurrent in-flight prefetch fills |

## Fetch scheduling & memory bounds

Concurrent origin fetches share one global, priority-aware budget rather than a
per-request cap:

- **Demand over prefetch.** Every origin GET is either `Demand` (a client's
  `GET`/`POST /ranges` request) or `Prefetch` (background warming, #1198).
  `MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES` of the total budget is
  always available to demand reads; prefetch is capped at the remainder, so a
  demand read is never stuck behind a large prefetch batch.
- **Promotion.** If a demand read arrives for a block a prefetch call already
  queued (but hasn't started fetching), that block is promoted to demand
  priority and competes for reserved capacity immediately, instead of waiting
  behind the rest of the prefetch batch.
- **Coalescing.** Contiguous missing blocks are merged into one origin GET
  (bounded by `MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES`) rather than
  one GET per block.
- **Cross-request memory budget.** `GET /obj/{key}` and `POST /ranges/{key}`
  responses are streamed rather than assembled in memory: bytes are written
  to the socket in bounded windows as they're fetched, so response size is no
  longer capped. `MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB` instead bounds the
  sum of in-flight streaming-window bytes across *all* concurrent requests (1
  MiB per permit). Each request charges `min(response size, a fixed per-stream
  window)`: a small response charges close to its actual size (so plenty of
  small reads can run concurrently), while a large one clamps to the window,
  bounding per-request memory and gating how many large streaming requests can
  run concurrently regardless of how big the response gets. The server floors
  this budget at the window's size at startup and refuses to start below it,
  since a smaller budget would make a large read's charge hang forever instead
  of failing fast.

## Prefetch

`POST /prefetch` warms the cache for a batch of keys at background priority, without serving any bytes back to the caller. The request body is `Content-Type: application/x-ndjson`: one JSON object per `\n`-terminated line, each describing a key to warm:

```
{"key": "blobs/abc", "size": 123456}
{"key": "blobs/def", "size": 654321, "ranges": [[0, 65536]]}
```

`size` must be the object's exact current size, supplied by the caller — the server trusts it rather than issuing an origin HEAD, since prefetch targets objects that are typically cold. `ranges` is optional; when absent or empty the whole object `[0, size)` is warmed, otherwise only the listed `[start, end)` ranges are.

The body is parsed incrementally as it arrives, so there is no whole-batch size cap and no key-count cap. The only remaining ceiling is on a single NDJSON line (1 MiB) — a request with a line longer than that is rejected with `400`. There is also no per-item size limit: the fill worker streams the block-index space in bounded windows rather than materializing it, so warming an arbitrarily large (or even bogus) `size` costs constant per-item memory. An oversized `size` just stops warming at the first origin fetch past the object's real end. A malformed line is counted as `rejected` and does not abort the rest of the batch, since newline framing means one bad line can't desynchronize the ones that follow.

The endpoint returns immediately with `202 Accepted` and a small JSON body:

```json
{"accepted": 1, "rejected": 0, "dropped": 1}
```

- `accepted` — items enqueued onto the background fill queue.
- `rejected` — items that failed key/prefix or range validation and were skipped; the rest of the batch still proceeds.
- `dropped` — items load-shed because the queue (`MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY`) was full. Prefetch is best-effort: a full queue never blocks the caller or the response status.

Fills run at the same `Prefetch` priority described in [Fetch scheduling & memory bounds](#fetch-scheduling--memory-bounds) above, so a large prefetch batch never starves a concurrent demand read. Prefetched blocks are admitted to the SSD tier only (not the RAM tier), so they don't compete with hot demand data for RAM residency; a later demand read against a prefetched block is served from SSD.

## Client opt-in

The cache is opt-in per client. Each split-mode service (ingestion, FlightSQL, the maintenance daemon) reads through it only when both of these are set in *its own* environment:

```bash
export MICROMEGAS_OBJECT_CACHE_URL=http://object-cache:8080
export MICROMEGAS_OBJECT_CACHE_API_KEY=<one of the keys in MICROMEGAS_API_KEYS>
```

If `MICROMEGAS_OBJECT_CACHE_URL` is set but the API key is missing, the client logs a warning and bypasses the cache entirely rather than sending unauthenticated requests. If neither is set, the client reads directly from the origin store, same as without a cache deployed at all.

## What gets cached

Only reads. The client falls back transparently to the direct store on any cache error, non-2xx response, or oversized request, so an unreachable or misbehaving cache degrades to direct reads rather than failing requests:

| Operation | Path |
|---|---|
| Range/whole-object reads (`get`, `get_ranges`) | Through the cache, with fallback to the origin store |
| Writes (`put`, multipart upload) | Always direct to the origin store |
| Deletes, listing, copy | Always direct to the origin store |

This works because the lake is **write-once**: blocks are written to a deterministic path exactly once and never modified in place, so a cached range never goes stale and the cache never needs invalidation.

## Authentication

The cache supports API keys only (no OIDC). Configure a key ring with `MICROMEGAS_API_KEYS` and give each client service its own named key, or pass `--disable-auth` for local development — matching the `--disable-auth` convention used by the other services.

## Health and readiness

`/health` and `/ready` both return an unconditional `200`; unlike the other services (see [Readiness probes](service-lifecycle.md#readiness-probes)), `/ready` does not probe the origin store. A load balancer can use either endpoint as a liveness check, but neither will catch the cache being unable to reach its origin — that surfaces as elevated client-side fallback-to-direct traffic instead.

## Monitoring

The cache emits metrics through the standard micromegas tracing sink (queryable like any other process telemetry). The key signals:

| Metric | Where | Meaning |
|---|---|---|
| `range_cache_block_request` | cache server | Every block lookup. Denominator for hit rate. |
| `range_cache_origin_block_fetch` | cache server | Blocks fetched from the origin (misses). **Hit rate = `1 - origin_block_fetch / block_request`** — this should fall over time as the cache warms. |
| `range_cache_origin_block_bytes` | cache server | Bytes pulled from the origin — the per-request S3 cost the cache exists to avoid. |
| `range_cache_origin_head` | cache server | `head` calls to the origin to resolve object sizes (once per object, then cached). |
| `range_cache_backend_error` | cache server | SSD/IO faults in the disk backend. Should be ~0; a sustained non-zero rate means a degraded volume silently inflating origin traffic. |
| `object_cache_get_bytes_served` / `object_cache_ranges_bytes_served` | cache server | Bytes served to clients over the wire. |
| `range_cache_client_fallback` | each client | Reads that fell back to the direct store (cache unreachable, non-2xx, or bad response). **A rising rate is the primary "cache unhealthy" alert** — routine fallback logs at `debug` precisely so it doesn't flood, leaving this metric as the signal. |
| `range_cache_block_len_mismatch` | cache server | A cached block's length didn't match its expected byte span (e.g. a poisoned entry from an undersized prefetch `size`, or the origin object changed size); the block is refetched and overwritten. Should be ~0. |
| `object_cache_prefetch_requests` / `object_cache_prefetch_keys_enqueued` | cache server | `POST /prefetch` request and accepted-key counts. |
| `object_cache_prefetch_dropped` | cache server | Prefetch items load-shed because the queue was full. A sustained non-zero rate means prefetch volume exceeds `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` / worker throughput. |
| `object_cache_prefetch_keys_warmed` / `object_cache_prefetch_fill_error` | cache server | Prefetch fills that completed successfully vs. failed (e.g. key not found at the origin). |
| `range_cache_client_prefetch_error` | each client | `CacheClientStore::prefetch` calls that failed (transport error or non-2xx). Best-effort — callers do not retry. |
| `range_cache_prefetch_admission_unexpected_none` | cache server | Defensive counter in the SSD-only prefetch admission path (`FoyerBackend::put`): bumped if `.force().insert(value)` unexpectedly returns `None`. Should never fire; a sustained non-zero rate points to an admission-path regression. |

Routine fallback-to-direct is by-design graceful degradation and is logged at `debug` (not `warn`). Genuinely unexpected conditions — a truncated cache response, a backend IO fault, an internal server error — log at `warn`/`error`.
