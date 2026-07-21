# Object Cache Deployment

`micromegas-object-cache-srv` is a shared HTTP range cache that sits in front of the data lake's object store. Split-mode query services (FlightSQL and the maintenance daemon) read overlapping byte ranges of the same Parquet/block files; routing those reads through one shared cache avoids re-fetching the same bytes from S3/GCS on every process, cutting egress cost and read latency.

It only caches **reads**. Writes, deletes, and listings always go straight to the origin store — see [What gets cached](#what-gets-cached) below.

!!! info "Looking for the *why*, not the *how*?"
    This is an operator/deployment guide. For an architecture-level view of how the cache tiers fit together — the in-process L1 cache, this L2 server, the metadata cache, and why there is no invalidation — see [Caching Architecture](../architecture/caching.md).

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
| `MICROMEGAS_OBJECT_CACHE_DISK_PATH` | Yes | Local disk path for the on-disk cache tier. The disk store carries an internal format version; on startup, a build whose format differs from the persisted store wipes the store directory once and rewarms from origin (no data loss). Same-format restarts reuse the store warm. |
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

!!! warning "Give the cache its own directory — its contents can be wiped on startup"
    The cache manages `MICROMEGAS_OBJECT_CACHE_DISK_PATH` exclusively. The on-disk store carries an internal format version, and when a build's format differs from the persisted store (a format-changing upgrade, or a first boot onto a pre-versioning store), the cache **deletes all contents** of this path on startup, then rewarms from origin. The directory/mount point itself is preserved; only its contents are removed. This is safe for cache data — the cache is a read-through layer over a write-once origin, so nothing but reconstructible cache blocks is lost (this is what "no data loss" means above) — but it means the path must be used **exclusively** by the cache. Never point it at a shared volume or a directory holding anything else, or that data will be erased on the next format bump. The wipe emits the `object_cache_disk_format_wiped` metric (see [Monitoring](#monitoring)).

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

Origin fetches share one global, priority-aware budget rather than a per-request cap. The knobs
that shape it are in the [environment variables](#environment-variables) table above; the ones
worth tuning:

- `--max-concurrent-fetches` / `--demand-reserved-fetches` — total origin-GET concurrency, and the
  slice always reserved for demand reads so they never queue behind a large prefetch batch.
- `--max-coalesced-get-bytes` — how large a run of contiguous missing blocks may be merged into a
  single origin GET.
- `--memory-budget-mb` — cross-request cap on in-flight streaming memory; the server refuses to
  start if it is set below one per-stream window.

See [Caching Architecture](../architecture/caching.md#read-path-mechanics) for how demand/prefetch
prioritization, coalescing, and streaming work.

## Prefetch

`POST /prefetch` warms the cache for a batch of keys at background priority, returning `202 Accepted` immediately without serving bytes back. The body is `Content-Type: application/x-ndjson`, one JSON object per line:

```
{"key": "blobs/abc", "size": 123456}
{"key": "blobs/def", "size": 654321, "ranges": [[0, 65536]]}
```

`size` is the object's exact size (the server trusts it rather than issuing a HEAD); `ranges` is optional and defaults to the whole object. A single NDJSON line is capped at 1 MiB. The response reports counts:

```json
{"accepted": 1, "rejected": 0, "dropped": 1}
```

- `accepted` — enqueued onto the background fill queue.
- `rejected` — failed key/prefix or range validation; the rest of the batch still proceeds.
- `dropped` — load-shed because the queue (`MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY`) was full. Prefetch is best-effort — a full queue never blocks the caller.

Warmed blocks are admitted to the SSD tier only, so they don't evict hot demand data from RAM.

## Write-time warming

When the writing service has the cache configured, a freshly-committed Parquet partition is warmed
automatically: the writer POSTs its key to `/prefetch` so the first query read is a hit instead of
a cold origin GET. It is fire-and-forget — a slow or unreachable cache never delays or fails a
write — and costs one extra origin GET per new partition, paid by the cache.

There is no separate on/off switch: it follows the same [client opt-in](#client-opt-in) as reads
(`MICROMEGAS_OBJECT_CACHE_URL` + `MICROMEGAS_OBJECT_CACHE_API_KEY`). The `object_warm_requested`
metric counts scheduled warms; a failed warm surfaces through `range_cache_client_prefetch_error`
and just means that object's first read stays a cold miss.

See [Caching Architecture](../architecture/caching.md#cache-warming) for the design.

## Client opt-in

The cache is opt-in per client. FlightSQL and the maintenance daemon use it only when both of these are set in *their own* environment:

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

Query processes (FlightSQL, the monolith) also carry a small **in-process L1 cache** in front of
this server: a repeat read of the same partition is served from the query process's own memory and
never reaches this service or the origin. It is a sibling tier of the same object-cache subsystem —
see [Caching Architecture](../architecture/caching.md) for how L1, this L2 server, and the origin
fit together, and what L1 does and doesn't cover.

For operators there is one knob: `MICROMEGAS_OBJECT_CACHE_L1_MB` (default `200`; `0` disables), set
in the *query* process's own environment. It sizes the in-process L1 RAM tier independently of
`MICROMEGAS_OBJECT_CACHE_RAM_MB` above, which sizes this server's RAM tier.

L1 emits the same `range_cache_*` hit/miss metrics as this server (see [Monitoring](#monitoring)),
but without per-prefix labels — every L1 hit/miss reports `prefix="other"`, giving aggregate L1
observability only.

## What gets cached

Only reads are cached, and the client falls back to a direct origin read on any cache error, so an unreachable or misbehaving cache degrades to direct reads rather than failing requests:

| Operation | Path |
|---|---|
| Range/whole-object reads (`get`, `get_ranges`) | Through the cache, with fallback to the origin store |
| Writes (`put`, multipart upload) | Always direct to the origin store |
| Deletes, listing, copy | Always direct to the origin store |

Cached ranges never need invalidation because the lake is write-once; see [Caching Architecture](../architecture/caching.md) for why.

## Authentication

The cache authenticates with API keys only (no OIDC). Configure a key ring with `MICROMEGAS_API_KEYS` and give **each client its own named key**, so keys can be rotated or revoked per service. Issue keys only to the services that actually use the cache — currently **FlightSQL and the maintenance daemon**.

Apply defense in depth: API keys are the application-layer check, but the cache is a purely internal service with no public role, so restrict it at the network layer too. Bind it to a private network and use a security group / firewall / Kubernetes `NetworkPolicy` so that **only the services that use the cache can reach its listen endpoint** — nothing else, the public internet included, should be able to open a connection.

`--disable-auth` drops the API-key check and is for local development only — never on an endpoint reachable by anything but localhost.

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
| `object_cache_ram_tier_hit` / `object_cache_disk_tier_hit` (`+ prefix`) | cache server | Block-key gets served from the foyer RAM tier vs. the disk tier (known by construction from the two-step read, not sniffed from a `Source` enum). `meta:`-prefixed `size()` lookups are excluded from both, so `range_cache_block_request − (ram_tier_hit + disk_tier_hit)` is a valid block-only miss rate — a proper aggregate tiered hit rate, the primary input to RAM-sizing decisions. `{prefix}` currently always resolves to `"other"` for these two (the storage-prefixed `blk:...` key never matches a content `prefix` label), so only the aggregate is meaningful today. |
| `object_cache_promotion_count` (`+ prefix`) | cache server | One per successful disk→RAM block promotion (the length-validated `Load::Entry`/`Load::Piece` promote arms). Equal by construction to `object_cache_disk_tier_hit`; paired with `object_cache_promotion_bytes` as the disk→RAM churn volume, weighed against `object_cache_ram_tier_eviction_*`. Same `{prefix}`-resolves-to-`"other"` caveat as `disk_tier_hit` above. |
| `object_cache_promotion_bytes` (`+ prefix`) | cache server | Bytes promoted disk→RAM (the promoted block's length). With the count, gives mean promoted block size — the churn-volume half of the RAM-sizing signal. Same `{prefix}` caveat as above. |
| `range_cache_size_backend_hit` (`+ prefix`) | cache server | `size()` lookups served from the backend. Fires exactly once per ranged GET (a prior double-counting bug on the handler's own pre-resolved size has been fixed). |
| `range_cache_size_implausible` (`+ prefix`) | cache server | A cached `size()` value decoded above the plausibility ceiling (256 TiB) — a corrupt/misdecoded cache entry, rejected and re-resolved from origin rather than trusted. Should be ~0. |
| `range_cache_origin_head` (`+ prefix`) | cache server | `head` calls to the origin to resolve object sizes (once per object, then cached). |
| `range_cache_backend_error` | cache server | SSD/IO faults in the disk backend. Should be ~0; a sustained non-zero rate means a degraded volume silently inflating origin traffic. |
| `range_cache_load_coalesced` | cache server | A concurrent foyer disk-tier load for a key already in flight joined the owner's single-flight load instead of issuing its own disk read. Cheap to observe with this change; a healthy signal of read coalescing under concurrent access to the same cold key. |
| `object_cache_disk_format_wiped` | cache server | The on-disk format-version marker didn't match this build's format, so the disk store directory was wiped and rewarms from origin. Fires once on a format-changing startup; should otherwise be ~0. |
| `object_cache_get_requests` (`status, prefix`) | cache server | Every `GET /obj/{key}` outcome — success and failure alike (a prior success-only bias has been fixed). Slice by `status` for the error-rate breakdown. |
| `object_cache_ranges_requests` (`status, prefix`) | cache server | Every `POST /ranges/{key}` outcome, same fix/semantics as `object_cache_get_requests`. |
| `object_cache_head_requests` (`status, prefix`) | cache server | Every `HEAD /obj/{key}` outcome — success and failure alike. Slice by `status` for the error-rate breakdown. |
| `object_cache_ranges_count` | cache server | Number of ranges in a `POST /ranges` request (success path only). |
| `object_cache_get_bytes_served` / `object_cache_ranges_bytes_served` | cache server | Bytes served to clients over the wire. Fires once per fully-produced response; a response cut short by a mid-stream origin error or an early consumer disconnect is excluded, so this can slightly under-count relative to raw egress. |
| `range_cache_client_fallback` | each client | Reads that fell back to the direct store (cache unreachable, non-2xx, or bad response). **A rising rate is the primary "cache unhealthy" alert** — routine fallback logs at `debug` precisely so it doesn't flood, leaving this metric as the signal. |
| `range_cache_block_len_mismatch` | cache server | A cached block's length didn't match its expected byte span (e.g. a poisoned entry from an undersized prefetch `size`, or the origin object changed size); the block is refetched and overwritten. Should be ~0. |
| `range_cache_promotion_len_mismatch` | cache server | A foyer disk-tier hit's length didn't match the caller's expected length (the same poisoned-short-prefetch scenario as `range_cache_block_len_mismatch`, observed at the backend's disk->RAM promotion gate instead of at the block-cache layer): the backend refuses to promote it and reports a miss instead. Should be ~0. |
| `range_cache_origin_run_len_mismatch` | cache server | An origin `get_range` fetch returned fewer bytes than the requested run span (the origin object shrank mid-flight). Surfaced as a fetch error rather than silently under-yielded. Should be ~0. |
| `object_cache_prefetch_requests` / `object_cache_prefetch_keys_enqueued` | cache server | `POST /prefetch` request and accepted-key counts. |
| `object_cache_prefetch_dropped` | cache server | Prefetch items load-shed because the queue was full. A sustained non-zero rate means prefetch volume exceeds `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` / worker throughput. |
| `object_cache_prefetch_keys_warmed` / `object_cache_prefetch_fill_error` | cache server | Prefetch fills that completed successfully vs. failed (e.g. key not found at the origin). |
| `range_cache_client_prefetch_error` | each client | Client-side prefetch calls that failed (transport error or non-2xx). Best-effort — callers do not retry. |
| `object_warm_requested` | maintenance daemon / JIT | A cache warm was scheduled for a freshly-written object (e.g. a new partition; see [Write-time warming](#write-time-warming)). |
| `range_cache_prefetch_admission_unexpected_none` | cache server | Defensive counter in the SSD-only prefetch admission path: bumped if a force-insert unexpectedly reports no admission. Should never fire; a sustained non-zero rate points to an admission-path regression. |

### Latency

Spans (queryable in the spans table, correlatable with a trace) cover every fetch stage: `range_cache_origin_get`, `range_cache_origin_head_latency`, `range_cache_backend_read`, and `range_cache_fetch_permit_wait`. The highest-value stages additionally get a duration metric so they're trivially aggregatable/alertable and dimensionable by `class`:

| Metric | Where | Meaning |
|---|---|---|
| `range_cache_fetch_permit_wait_ms` (`+ class`) | cache server | Time a coalesced origin fetch spent waiting for a fetch-budget permit before its `get_range` started. The highest-value scheduler signal — a rising `class="demand"` wait means demand is contending with prefetch (or with itself) for the shared budget. |
| `range_cache_origin_get_ms` (`+ class`) | cache server | Duration of the origin `get_range` call itself, once a permit was held. |
| `object_cache_mem_permit_wait_ms` | cache server | Time a request spent waiting to acquire its cross-request memory-budget permits (`--memory-budget-mb`) before it could start streaming. A rising value means the memory budget, not the fetch budget, is the bottleneck. |
| `object_cache_ttfb_ms` (`+ prefix`) | cache server | Time to first byte: handler entry to the first chunk being ready to send. |
| `range_cache_client_roundtrip_ms` | each client | Time for a streaming cache-path read (`get_range`/`get_full_stream`) to get a usable stream back from the cache server, measured before any body bytes are read. |
| `range_cache_client_ranges_ms` | each client | Time for a `get_ranges` cache-path read to fully read and reassemble the framed multi-range response body. Not directly comparable to `range_cache_client_roundtrip_ms`, which is measured at time-to-headers rather than time-to-full-body. |
| `range_cache_client_direct_ms` | each client | Time for the direct-store fallback path taken on a cache miss/error, on any of the above paths. Compare against the corresponding cache-path metric to confirm the cache path is actually winning end-to-end. |

### Saturation

A background sampler emits these gauges on a fixed interval (5s by default), independent of request volume — the signals needed to tell *which* resource is the bottleneck, not just that requests are slow:

| Metric | Meaning |
|---|---|
| `object_cache_fetch_shared_occupancy` / `object_cache_fetch_shared_available` | Occupied/available slots in the total origin-GET concurrency budget (`--max-concurrent-fetches`). |
| `object_cache_fetch_prefetch_occupancy` / `object_cache_fetch_prefetch_available` | Occupied/available slots in the prefetch-only sub-budget (`--max-concurrent-fetches` minus `--demand-reserved-fetches`). |
| `object_cache_inflight_entries` | Number of block/`size()` keys currently in flight to origin. A key scheduler signal alongside the permit-wait latency above. |
| `object_cache_ram_tier_usage_bytes` | Accounted RAM-tier byte usage (foyer's own weigher total). Compare against the host's `used_memory` system metric: this gauge staying at/below the configured `--ram-mb` size *while* `used_memory` climbs is the signature of a cached block over-retaining a larger allocation than its accounted weight. |
| `object_cache_mem_budget_occupancy_mb` / `object_cache_mem_budget_available_mb` | Occupied/available MiB of the cross-request streaming memory budget (`--memory-budget-mb`). |
| `object_cache_prefetch_queue_depth` | Items currently queued in the bounded `/prefetch` queue, waiting for a worker slot. |
| `object_cache_nic_rx_bytes_per_sec` / `object_cache_nic_tx_bytes_per_sec` | Host-level network throughput — the expected ceiling on the deployment's instance type. |
| `object_cache_foyer_disk_write_bytes_per_sec` / `object_cache_foyer_disk_read_bytes_per_sec` | The foyer disk engine's own write/read throughput, sourced from the cache engine rather than host disk enumeration. The drain-throughput signal for whether the flushers are keeping up with write-in pressure (see "Tuning the write path" below). |
| `object_cache_foyer_disk_write_ios_per_sec` / `object_cache_foyer_disk_read_ios_per_sec` | The foyer disk engine's own write/read IO rate. |

Routine fallback-to-direct is by-design graceful degradation and is logged at `debug` (not `warn`). Genuinely unexpected conditions — a truncated cache response, a backend IO fault, an internal server error — log at `warn`/`error`.

### Tuning the write path

Prefetch fills are force-admitted to the SSD tier (bypassing the admission picker), so a large prefetch burst can outrun foyer's disk-engine submit queue; when that happens foyer logs a recurring `submit queue overflow, new entry ignored` WARN and silently drops the entry (no crash, but a lower hit rate and more origin traffic). Two knobs control the ceiling:

- `MICROMEGAS_OBJECT_CACHE_FLUSHERS` — how many blocks can be written to disk concurrently.
- `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` — the flush buffer pool size; the submit-queue overflow threshold is set to 2x this value.

If the overflow WARN is firing, check `object_cache_foyer_disk_write_bytes_per_sec` / `object_cache_foyer_disk_write_ios_per_sec` alongside it: a write rate that's flat while the WARN fires means the flushers are saturated and would benefit from a higher `MICROMEGAS_OBJECT_CACHE_FLUSHERS`, while a healthy rate with occasional bursts past the buffer suggests raising `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` instead. foyer does not expose submit-queue occupancy itself, so the WARN log remains the direct drop signal.
