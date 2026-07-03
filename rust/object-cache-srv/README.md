# Micromegas Object Cache Server Crate

This crate provides a shared object range cache service for the Micromegas
observability platform. It sits in front of an origin object store (S3, GCS,
local filesystem, ...) and serves byte-range reads from a two-tier RAM + disk
cache, so that many query workers share a single warm cache of frequently read
objects (parquet column chunks, blocks) instead of each re-reading from the
origin.

The cache layer is backed by [`micromegas-object-cache`](../object-cache) with a
foyer RAM/disk backend.

## HTTP API

Keys are passed as the trailing path segment. They are validated to reject empty
keys, leading `/`, and `..` traversal, and (when configured) must fall under the
allowed prefix.

| Method | Path           | Description                                                                 |
|--------|----------------|-----------------------------------------------------------------------------|
| `GET`  | `/health`      | Liveness probe — always `200 OK`.                                           |
| `GET`  | `/ready`       | Readiness probe — always `200 OK`.                                          |
| `HEAD` | `/obj/{key}`   | Returns the object size in `Content-Length`.                                |
| `GET`  | `/obj/{key}`   | Reads a single byte range (standard `Range: bytes=start-end` header). Replies `206 Partial Content` with a `Content-Range` header. |
| `POST` | `/ranges/{key}`| Reads many ranges at once. Body: `{"ranges": [[start, end), ...]}`. Replies with each chunk length-prefixed as a little-endian `u64` followed by its bytes. |
| `POST` | `/prefetch`    | Warms a batch of keys at prefetch priority without returning bytes. Body: `application/x-ndjson`, one `{"key": ..., "size": ..., "ranges": [[start, end), ...]}` object per `\n`-terminated line (`ranges` optional; absent/empty warms the whole object). Replies immediately with `202 Accepted` and `{"accepted": n, "rejected": n, "dropped": n}`. |

Range bounds use half-open `[start, end)` semantics in the `POST` body. Inverted
or zero-length ranges are rejected with `400`. A single request is capped at
4096 ranges and 512 MiB of total requested bytes (`413 Payload Too Large`
beyond that). Out-of-bounds ranges return `416 Range Not Satisfiable`.

`/prefetch` enqueues each key onto a bounded background queue and returns
without waiting for the fetch to complete. `size` should be the object's exact
current size — the server trusts it to avoid an origin HEAD, since prefetch
targets cold objects. An oversized value is safe (the origin GET past EOF fails
and nothing is stored); an undersized value is the only harmful case, and it is
mitigated by a hit-path length guard that detects and refetches a truncated
block on the next correctly-sized read. The NDJSON body is parsed line by line
as it streams in, so there is no whole-batch size cap and no key-count cap; the
only remaining ceiling is 1 MiB on a single line (`400` if exceeded). There is
also no per-item size limit, since the fill worker streams the block-index
space in bounded windows rather than materializing it. A bad key, a malformed
line, or an inverted/out-of-bounds range fails only that item (`rejected`); a
full queue load-sheds the item (`dropped`) rather than blocking the caller.
Neither failure mode affects the response status, which is always `202` once
the request stream itself is consumed successfully.

## Authentication

Requests to the object endpoints are authenticated with API keys via
`MICROMEGAS_API_KEYS`. Pass `--disable-auth` for local development only. The
`/health` and `/ready` probes are always unauthenticated.

## Configuration

All flags can be set via the listed environment variables.

| Flag                            | Env var                              | Default        | Description                                                        |
|---------------------------------|--------------------------------------|----------------|--------------------------------------------------------------------|
| `--listen`                      | `MICROMEGAS_OBJECT_CACHE_LISTEN`     | `0.0.0.0:8080` | Listen address.                                                    |
| `--origin-uri`                  | `MICROMEGAS_OBJECT_CACHE_ORIGIN_URI` | _(required)_   | Origin object store URI. Must be bucket-only with no path component. |
| `--ram-mb`                      | `MICROMEGAS_OBJECT_CACHE_RAM_MB`     | `512`          | RAM cache size, in MB.                                             |
| `--disk-path`                   | `MICROMEGAS_OBJECT_CACHE_DISK_PATH`  | _(required)_   | Directory for the on-disk cache.                                  |
| `--disk-gb`                     | `MICROMEGAS_OBJECT_CACHE_DISK_GB`    | `50`           | Disk cache size, in GB.                                           |
| `--block-size`                  | `MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE` | `1048576`      | Cache block size, in bytes (must be > 0).                         |
| `--namespace`                   | `MICROMEGAS_OBJECT_CACHE_NAMESPACE`  | _(derived)_    | Cache namespace. Defaults to the origin URI with the scheme stripped. |
| `--prefix`                      | `MICROMEGAS_OBJECT_CACHE_PREFIX`     | _(required)_   | Allowed key prefixes. Repeat the flag or comma-separate the env var (e.g. `blobs,views`). Only keys equal to a prefix or under `{prefix}/` are served. The server refuses to start unless at least one is set or `--allow-all-prefixes` is passed. |
| `--allow-all-prefixes`          |                                      | `false`        | Serve the entire bucket, bypassing prefix containment (development only). |
| `--api-keys`                    | `MICROMEGAS_API_KEYS`                | _(none)_       | Key ring for request authentication.                             |
| `--disable-auth`                |                                      | `false`        | Disable authentication (development only).                        |
| `--shutdown-grace-period-seconds` | `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS` | `25`  | Graceful-shutdown grace period on `SIGTERM`.                     |
| `--max-concurrent-fetches`      | `MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES` | `32` | Total concurrent origin GETs.                              |
| `--demand-reserved-fetches`     | `MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES` | `8` | Origin-GET slots always reserved for demand reads; must be less than `--max-concurrent-fetches`. |
| `--max-coalesced-get-bytes`     | `MICROMEGAS_OBJECT_CACHE_MAX_COALESCED_GET_BYTES` | `8388608` | Max byte span of one coalesced run GET.                |
| `--memory-budget-mb`            | `MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB` | `1024`        | Cross-request cap on concurrently-assembled response bytes, in MiB. |
| `--promote-whole-batch`         | `MICROMEGAS_OBJECT_CACHE_PROMOTE_WHOLE_BATCH` | `false`    | On a demand hit into a prefetch batch, promote the whole batch instead of only the covering run. |
| `--prefetch-queue-capacity`     | `MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY` | `4096` | Depth of the bounded `/prefetch` queue; items beyond this are load-shed. |
| `--prefetch-worker-concurrency` | `MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY` | `8` | Concurrent in-flight prefetch fills driven by the queue worker. |

> **Note:** `--origin-uri` must point at the bucket root with no path
> component. The lake-root prefix arrives inside each request key, so a path on
> the origin URI would be applied twice and produce silent 404s.

## Documentation

- 📖 [Complete Documentation](https://micromegas.info/)
- 🏗️ [Architecture Overview](https://micromegas.info/docs/architecture/)
- 💻 [GitHub Repository](https://github.com/madesroches/micromegas)
