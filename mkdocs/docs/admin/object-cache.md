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
| `MICROMEGAS_OBJECT_CACHE_FLUSHERS` | No | foyer disk-engine flusher count -- how many blocks can be written to disk concurrently (default `2`); must be > 0 |
| `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` | No | foyer disk-engine flush buffer pool size, in MiB (default `128`); the submit-queue overflow threshold is set to 2x this value; must be > 0 |

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
| `--flushers` | `2` | foyer disk-engine flusher count |
| `--write-buffer-mb` | `128` | foyer disk-engine flush buffer pool size, in MiB (submit-queue threshold is 2x this) |

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

## Write-time warming

When a writer (the maintenance daemon, JIT materialization, or a merge) finishes writing a new
Parquet partition durably to the origin store *and* commits its row to
`lakehouse_partitions` in PostgreSQL, it POSTs the partition's key to `/prefetch` so the cache
pulls it from origin at prefetch priority *before* the follow-up query asks for it. This turns the
first read of a new partition from a cold origin GET into a warm cache hit.

This is **notify-by-key**, not a write-through cache: the writer never pushes bytes into the
cache — it only names the key that just became available, and the cache fetches it from origin
itself, exactly like a prefetch triggered any other way. The write/materialization path is never
delayed or failed by a warm: the POST is fired from a detached, fire-and-forget task, so a slow or
unreachable cache has no effect on write latency or success.

Write-time warming is enabled automatically whenever the writing service has the cache configured
(`MICROMEGAS_OBJECT_CACHE_URL` + `MICROMEGAS_OBJECT_CACHE_API_KEY` set, per
[Client opt-in](#client-opt-in) below) — there is no separate on/off switch. The cost is one extra
origin GET per new partition, paid by the cache, off the write path.

A warm is only ever requested for a non-empty partition (empty partitions have no object to warm).
The underlying trigger is a general "warm any object by key" primitive (`DataLakeConnection::warm_object`),
so nothing about it is partition-specific — the write-partition path is simply its first caller.
The `object_warm_requested` metric counts scheduled warms; a failed warm (unreachable
cache, non-2xx, etc.) surfaces through the existing `range_cache_client_prefetch_error` metric and
simply means the first demand read of that object stays a cold miss rather than a hit — it does
not raise an error anywhere.

## Client opt-in

The cache is opt-in per client. Each split-mode service (ingestion, FlightSQL, the maintenance daemon) reads through it only when both of these are set in *its own* environment:

```bash
export MICROMEGAS_OBJECT_CACHE_URL=http://object-cache:8080
export MICROMEGAS_OBJECT_CACHE_API_KEY=<one of the keys in MICROMEGAS_API_KEYS>
```

If `MICROMEGAS_OBJECT_CACHE_URL` is set but the API key is missing, the client logs a warning and bypasses the cache entirely rather than sending unauthenticated requests. If neither is set, the client reads directly from the origin store, same as without a cache deployed at all.

The **monolith** (`micromegas-monolith`) is a client too, not a cache host: it runs no in-process
cache server, so both reads and write-time warming go to an *external* `object-cache-srv` over HTTP.
Set the same two variables in the monolith's environment to enable them; leave them unset (the
default) and the monolith reads directly from origin and every `warm_object` call is a harmless
no-op.

## In-process L1 cache

Query processes (FlightSQL, the monolith) also carry a small **in-process L1 cache**, independent
of the `object-cache-srv` deployment described above. It sits closer to the query than this
service does: L1 caches hot byte ranges directly inside the query process's memory, so a repeat
read of the same partition never leaves the process, let alone reaches this cache server or the
origin store. An L1 miss falls through to whatever store L1 wraps — this cache server if
[client opt-in](#client-opt-in) is configured, otherwise the origin store directly — so the
L1 → L2 (this service) → origin tiering still holds; L1 is just an additional tier in front of it.

L1 is sized by `MICROMEGAS_OBJECT_CACHE_L1_MB` (default `200`; `0` disables it) in the query
process's own environment. It belongs to the same `MICROMEGAS_OBJECT_CACHE_*` family as this
server's knobs — the two are sibling tiers of one object-cache subsystem: `_L1_MB` sizes the
in-process L1 tier, while `MICROMEGAS_OBJECT_CACHE_RAM_MB` above sizes this server's own RAM tier.
Set them independently.

L1 covers exactly two read paths, both read-repeatedly on the query hot path:

- Parquet partition reads (materialized views under `views/...`), through DataFusion's parquet
  reader.
- Static JSON/CSV table reads (`MICROMEGAS_STATIC_TABLES_URL`).

It deliberately excludes raw blob reads (`blobs/{process_id}/{stream_id}/{block_id}`) — ETL
materialization and the `get_payload`/`parse_block` SQL functions always read those directly from
the origin/L2 stack, never through L1, since blobs are read exactly once and caching them would
only add memory pressure for no benefit.

L1 emits the same `RangeCache` hit/miss metrics described in [Monitoring](#monitoring) below, but
without per-prefix labels, so every L1 hit/miss (parquet or static-table alike) reports
`prefix="other"` — this gives aggregate L1 observability only, with no way to split lakehouse
traffic from static-table traffic in the metrics.

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

**Dimensions.** Several metrics below carry small, closed-set tags: `status` (the response's HTTP status, e.g. `"200"`, `"404"`), `prefix` (the object-category label a key falls under, from the server's configured `--prefix`/`MICROMEGAS_OBJECT_CACHE_PREFIX` allowlist, or `"other"` on no match), `class` (`"demand"` vs `"prefetch"`, from the request's priority), and `tier` (`"backend"` vs `"origin"` — implied by which of a metric-name pair fired, e.g. `range_cache_block_backend_hit` is `tier="backend"` and `range_cache_origin_block_fetch` is `tier="origin"`, rather than an explicit tag). This lets hit rate, request volume, and latency be sliced per object category and per demand-vs-prefetch traffic, not just globally.

| Metric | Where | Meaning |
|---|---|---|
| `range_cache_block_request` (`+ prefix`) | cache server | Every block lookup. Denominator for hit rate. |
| `range_cache_origin_block_fetch` (`+ prefix, class`) | cache server | Blocks fetched from the origin (misses). **Hit rate = `1 - origin_block_fetch / block_request`** — this should fall over time as the cache warms, and can now be computed per `prefix`. |
| `range_cache_origin_block_bytes` (`+ prefix, class`) | cache server | Bytes pulled from the origin — the per-request S3 cost the cache exists to avoid. |
| `range_cache_block_backend_hit` (`+ prefix`) | cache server | Block lookups served from the backend (foyer). The `tier="backend"` side of the hit-rate split. |
| `range_cache_size_backend_hit` (`+ prefix`) | cache server | `size()` lookups served from the backend. Fires exactly once per ranged GET (a prior double-counting bug on the handler's own pre-resolved size has been fixed). |
| `range_cache_origin_head` (`+ prefix`) | cache server | `head` calls to the origin to resolve object sizes (once per object, then cached). |
| `range_cache_backend_error` | cache server | SSD/IO faults in the disk backend. Should be ~0; a sustained non-zero rate means a degraded volume silently inflating origin traffic. |
| `object_cache_get_requests` (`status, prefix`) | cache server | Every `GET /obj/{key}` outcome — success and failure alike (a prior success-only bias has been fixed). Slice by `status` for the error-rate breakdown. |
| `object_cache_ranges_requests` (`status, prefix`) | cache server | Every `POST /ranges/{key}` outcome, same fix/semantics as `object_cache_get_requests`. |
| `object_cache_ranges_count` | cache server | Number of ranges in a `POST /ranges` request (success path only). |
| `object_cache_get_bytes_served` / `object_cache_ranges_bytes_served` | cache server | Bytes served to clients over the wire. |
| `range_cache_client_fallback` | each client | Reads that fell back to the direct store (cache unreachable, non-2xx, or bad response). **A rising rate is the primary "cache unhealthy" alert** — routine fallback logs at `debug` precisely so it doesn't flood, leaving this metric as the signal. |
| `range_cache_block_len_mismatch` | cache server | A cached block's length didn't match its expected byte span (e.g. a poisoned entry from an undersized prefetch `size`, or the origin object changed size); the block is refetched and overwritten. Should be ~0. |
| `range_cache_origin_run_len_mismatch` | cache server | An origin `get_range` fetch returned fewer bytes than the requested run span (the origin object shrank mid-flight). Surfaced as a fetch error rather than silently under-yielded. Should be ~0. |
| `object_cache_prefetch_requests` / `object_cache_prefetch_keys_enqueued` | cache server | `POST /prefetch` request and accepted-key counts. |
| `object_cache_prefetch_dropped` | cache server | Prefetch items load-shed because the queue was full. A sustained non-zero rate means prefetch volume exceeds `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` / worker throughput. |
| `object_cache_prefetch_keys_warmed` / `object_cache_prefetch_fill_error` | cache server | Prefetch fills that completed successfully vs. failed (e.g. key not found at the origin). |
| `range_cache_client_prefetch_error` | each client | `CacheClientStore::prefetch` calls that failed (transport error or non-2xx). Best-effort — callers do not retry. |
| `object_warm_requested` | writer (ingestion) | A cache warm was scheduled for a freshly-written object via `DataLakeConnection::warm_object` (e.g. a new partition; see [Write-time warming](#write-time-warming)). |
| `range_cache_prefetch_admission_unexpected_none` | cache server | Defensive counter in the SSD-only prefetch admission path (`FoyerBackend::put`): bumped if `.force().insert(value)` unexpectedly returns `None`. Should never fire; a sustained non-zero rate points to an admission-path regression. |

### Latency

Spans (queryable in the spans table, correlatable with a trace) cover every fetch stage: `range_cache_origin_get`, `range_cache_origin_head_latency`, `range_cache_backend_read`, and `range_cache_fetch_permit_wait`. The highest-value stages additionally get a duration metric so they're trivially aggregatable/alertable and dimensionable by `class`:

| Metric | Where | Meaning |
|---|---|---|
| `range_cache_fetch_permit_wait_ms` (`+ class`) | cache server | Time a coalesced origin fetch spent waiting for a fetch-budget permit before its `get_range` started. The highest-value signal for the #1203 scheduler — a rising `class="demand"` wait means demand is contending with prefetch (or with itself) for the shared budget. |
| `range_cache_origin_get_ms` (`+ class`) | cache server | Duration of the origin `get_range` call itself, once a permit was held. |
| `object_cache_mem_permit_wait_ms` | cache server | Time a request spent waiting to acquire its cross-request memory-budget permits (`--memory-budget-mb`) before it could start streaming. A rising value means the memory budget, not the fetch budget, is the bottleneck. |
| `object_cache_ttfb_ms` (`+ prefix`) | cache server | Time to first byte: handler entry to the first chunk being ready to send, now that streaming (#1189/#1222) has landed. |
| `range_cache_client_roundtrip_ms` | each client | Time for a streaming cache-path read (`get_range`/`get_full_stream`) to get a usable stream back from the cache server, measured before any body bytes are read. |
| `range_cache_client_ranges_ms` | each client | Time for a `get_ranges` cache-path read to fully read and reassemble the framed multi-range response body. Not directly comparable to `range_cache_client_roundtrip_ms`, which is measured at time-to-headers rather than time-to-full-body. |
| `range_cache_client_direct_ms` | each client | Time for the direct-store fallback path taken on a cache miss/error, on any of the above paths. Compare against the corresponding cache-path metric to confirm the cache path is actually winning end-to-end. |

### Saturation

A background sampler (`object-cache-srv/src/saturation_monitor.rs`) emits these gauges on a fixed interval (5s by default), independent of request volume — the signals needed to tell *which* resource is the bottleneck, not just that requests are slow:

| Metric | Meaning |
|---|---|
| `object_cache_fetch_shared_occupancy` / `object_cache_fetch_shared_available` | Occupied/available slots in the total origin-GET concurrency budget (`--max-concurrent-fetches`). |
| `object_cache_fetch_prefetch_occupancy` / `object_cache_fetch_prefetch_available` | Occupied/available slots in the prefetch-only sub-budget (`--max-concurrent-fetches` minus `--demand-reserved-fetches`). |
| `object_cache_inflight_entries` | Number of block/`size()` keys currently in flight to origin. A key signal for the #1203 scheduler alongside the permit-wait latency above. |
| `object_cache_mem_budget_occupancy_mb` / `object_cache_mem_budget_available_mb` | Occupied/available MiB of the cross-request streaming memory budget (`--memory-budget-mb`). |
| `object_cache_prefetch_queue_depth` | Items currently queued in the bounded `/prefetch` queue, waiting for a worker slot. |
| `object_cache_nic_rx_bytes_per_sec` / `object_cache_nic_tx_bytes_per_sec` | Host-level network throughput. The expected ceiling on the target im4gn.large (#1197) instance type, and previously unmeasured. |
| `object_cache_foyer_disk_write_bytes_per_sec` / `object_cache_foyer_disk_read_bytes_per_sec` | The foyer disk engine's own write/read throughput (`Statistics::disk_write_bytes` / `disk_read_bytes`), sourced from the cache engine itself rather than host disk enumeration — supersedes the old `object_cache_ssd_*` gauges, which always read 0 in the deployed container. The drain-throughput signal for whether the flushers are keeping up with write-in pressure (see "Tuning the write path" below). |
| `object_cache_foyer_disk_write_ios_per_sec` / `object_cache_foyer_disk_read_ios_per_sec` | The foyer disk engine's own write/read IO rate (`Statistics::disk_write_ios` / `disk_read_ios`). |

Routine fallback-to-direct is by-design graceful degradation and is logged at `debug` (not `warn`). Genuinely unexpected conditions — a truncated cache response, a backend IO fault, an internal server error — log at `warn`/`error`.

### Tuning the write path

Prefetch fills are force-admitted to the SSD tier (`.force().insert()`, bypassing the admission picker), so a large prefetch burst can outrun foyer's disk-engine submit queue; when that happens foyer logs a recurring `submit queue overflow, new entry ignored` WARN and silently drops the entry (no crash, but a lower hit rate and more origin traffic). Two knobs control the ceiling:

- `MICROMEGAS_OBJECT_CACHE_FLUSHERS` — how many blocks can be written to disk concurrently.
- `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` — the flush buffer pool size; the submit-queue overflow threshold is set to 2x this value.

If the overflow WARN is firing, check `object_cache_foyer_disk_write_bytes_per_sec` / `object_cache_foyer_disk_write_ios_per_sec` alongside it: a write rate that's flat while the WARN fires means the flushers are saturated and would benefit from a higher `MICROMEGAS_OBJECT_CACHE_FLUSHERS`, while a healthy rate with occasional bursts past the buffer suggests raising `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` instead. foyer does not expose submit-queue occupancy itself, so the WARN log remains the direct drop signal.
