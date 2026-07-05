# Write-Time Partition Warming (notify-by-key) Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1201

## Overview

After a freshly-materialized parquet partition is written durably to S3 and registered in
PostgreSQL, POST its object key to the object-cache `/prefetch` endpoint so the cache re-reads it
from origin into its SSD tier *before* the follow-up query asks for it. This turns the first read
of a new partition from a cold S3 read into a warm cache hit. The mechanism is **notify-by-key**:
the write path never pushes bytes into the cache (the cache stays strictly read-only); it only
tells the cache *which key* became available, and the cache pulls it from origin at prefetch
priority. The cost is one extra origin GET per new partition, paid by the cache, off the write
path.

The trigger must be **fire-and-forget at prefetch priority**: it must not add latency to, or
propagate errors into, the write/materialization path. It builds on the already-merged `/prefetch`
endpoint and client (#1198 / #1220 / #1218) and the read-path rework (#1203 / #1216).

## Current State

### Where partitions are written

Every partition — daemon-scheduled materialization, JIT materialization, and merges — funnels
through one function:

- `write_partition_from_rows` (`rust/analytics/src/lakehouse/write_partition.rs:565`). It writes
  the parquet file to object storage via `AsyncParquetWriter` over `BufWriter`, then calls
  `finalize_partition_write` (producing a `PartitionWriteResult`), then `insert_partition`
  (`write_partition.rs:251`) which, inside one transaction, retires overlapping partitions, inserts
  the parquet metadata, and inserts the `lakehouse_partitions` row. When `insert_partition` returns
  `Ok`, the file is durable in S3 **and** registered — the exact post-condition #1201 wants to warm
  on.
- Callers that reach it (all via `spawn_with_context`): `block_partition_spec.rs:80`,
  `sql_partition_spec.rs:92`, `metadata_partition_spec.rs:85`, `merge.rs:184`,
  `net_spans_view.rs:135`, `thread_spans_view.rs:125`. Because they all converge here, a single
  hook inside `write_partition_from_rows` covers daemon, JIT, and merge writes (DRY).

`PartitionWriteResult` (`write_partition.rs:403`) already carries exactly what a warm needs:
- `file_path: Option<String>` — `None` for an empty partition (nothing written to S3); `Some` is a
  lake-root-relative key like `views/{view_set}/{view_instance}/{YYYY-MM-DD}/{HH-MM-SS}_{uuid}.parquet`.
- `file_size: i64` — the exact byte count written (`byte_counter`, `write_partition.rs:514`), which
  is the S3 object size and satisfies the `PrefetchItem::size` exact-size contract.

### How the cache client is wired (and the key-namespace subtlety)

- `DataLakeConnection` (`rust/ingestion/src/data_lake_connection.rs:11`) holds
  `blob_storage: Arc<BlobStorage>`. It is built by `connect_to_data_lake`
  (`data_lake_connection.rs:44`) and `connect_to_remote_data_lake`
  (`rust/ingestion/src/remote_data_lake.rs:44`), both via
  `BlobStorage::connect_with_layer(url, make_cache_layer())`.
- `make_cache_layer` (`data_lake_connection.rs:25`) reads `MICROMEGAS_OBJECT_CACHE_URL` /
  `MICROMEGAS_OBJECT_CACHE_API_KEY`; when both are set it wraps the raw store in
  `CacheClientStore::new(url, api_key, direct)` (`rust/object-cache/src/client.rs:36`), else returns
  the store unwrapped.
- `BlobStorage::connect_with_layer` (`rust/telemetry/src/blob_storage.rs:33`) parses the URL into
  `(raw_bucket_store, root_path)` via `object_store::parse_url_opts`, applies the layer to the
  **full-bucket** store, then wraps the result in `PrefixStore::new(layered, root)`. So the final
  layering is `PrefixStore(root) → CacheClientStore → raw_bucket_store`.

**Key-namespace consequence.** A demand read of partition `views/foo.parquet` goes
`blob_storage.inner().get("views/foo.parquet")` → `PrefixStore` prepends `root` →
`CacheClientStore.get("root/views/foo.parquet")` → the cache server is keyed by
`root/views/foo.parquet`. Analytics registers `lake.blob_storage.inner()` as the DataFusion object
store for `obj://lakehouse/` (`rust/analytics/src/lakehouse/query.rs:204`), so *all* partition
reads use this same `PrefixStore→CacheClientStore` stack and therefore this same key.

Therefore a warm POST must use the **root-prefixed** key `root/views/foo.parquet`, not the
lake-root-relative `views/foo.parquet` that the write path holds. Getting this wrong warms the
wrong key and every demand read stays a cold miss — silently. The prefix is applied today only
inside `PrefixStore`, which is buried behind `Arc<dyn ObjectStore>`; the warm path needs its own
prefix application that matches `PrefixStore`'s composition exactly.

### The prefetch surface that already exists

- `CacheClientStore::prefetch(items: Vec<PrefetchItem>)` (`client.rs:188`) POSTs NDJSON to
  `{base}/prefetch`, best-effort (an `Err` means "the warm didn't happen"; no retry). It already
  emits `range_cache_client_prefetch_error` on failure.
- The dyn-compatible seam `ObjectPrefetch` (`rust/object-cache/src/prefetch.rs`) is implemented by
  `CacheClientStore` (`client.rs:224`) — this is the "downstream consumers hold `dyn`, not the
  concrete store" seam that #1198's plan added specifically for #1200/#1201. It returns
  `PrefetchResponse { accepted, rejected, dropped }`.
- `PrefetchItem { key, size, ranges }` — `ranges: None` means "warm the whole object `[0, size)`".
  That is exactly a whole-partition warm.

### Negative-caching interaction (#1196) — currently moot

The issue notes a hazard: a key missed *before it existed* must not stay negatively cached once
warming makes it available. In the **current tree there is no persistent negative cache**:
`RangeCache::size` (`rust/object-cache/src/range_cache.rs:506`) resolves a miss by having the
in-flight owner `head` the origin and, on `NotFound`, `fulfill(Err(..))` and `remove_entry`
(`range_cache.rs:546-551`) — nothing is stored for an absent key. #1196 ("single-flight and
negative caching for size() lookups", CLOSED) was realized as single-flight coalescing without a
TTL'd negative entry; the module docs (`range_cache.rs:441-447`) confirm size/block caches carry no
TTL and are never invalidated. So there is nothing to invalidate here today, and warming a
previously-missed key simply populates a fresh size/block entry on the cache's own origin GET. This
plan therefore adds **no** negative-cache coupling; if a TTL'd negative cache is added later, its
own TTL note (per #1196) is the mitigation, not this issue.

## Design

Three pieces: (1) a prefix-applying `ObjectPrefetch` adapter so warm keys match read keys; (2)
wiring the cache client's prefetch face onto `DataLakeConnection`; (3) a fire-and-forget warm call
at the write choke point.

### 1. `PrefixPrefetch` adapter (`object-cache` crate)

The read path applies the lake root via `object_store::PrefixStore`. The warm path needs the
identical transformation on `PrefetchItem::key`. Add a small adapter in
`rust/object-cache/src/prefetch.rs` (the crate already depends on `object_store` for `path::Path`):

```rust
use object_store::path::Path;

/// Prepends a root prefix to each `PrefetchItem`'s key before delegating, so a
/// warm keyed by a lake-root-relative path (`views/…`) targets the same cache
/// key a demand read produces through `object_store::PrefixStore` (`root/views/…`).
/// This mirrors, for the prefetch path, what `PrefixStore` does for reads.
pub struct PrefixPrefetch {
    inner: Arc<dyn ObjectPrefetch>,
    prefix: Path,
}

impl PrefixPrefetch {
    pub fn new(inner: Arc<dyn ObjectPrefetch>, prefix: Path) -> Self { Self { inner, prefix } }
}

#[async_trait]
impl ObjectPrefetch for PrefixPrefetch {
    async fn prefetch(&self, mut items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse> {
        for item in &mut items {
            // Compose exactly as PrefixStore does: chain the prefix's path parts
            // with the key's parts, so the resulting string equals the key
            // CacheClientStore.get() sees for a demand read of the same object.
            let full = Path::from_iter(
                self.prefix.parts().chain(Path::from(item.key.as_str()).parts()),
            );
            item.key = full.as_ref().to_string();
        }
        self.inner.prefetch(items).await
    }
}
```

Using `Path`-parts composition (not `format!("{prefix}/{key}")`) guarantees byte-identical keys to
`PrefixStore` for every prefix shape, including an **empty root** (`Path::default()`, common in the
local MinIO test env where the URL has no path component) — then the key is unchanged. Export
`PrefixPrefetch` from `rust/object-cache/src/lib.rs`.

### 2. Surface the prefetch face + root on `DataLakeConnection`

`CacheClientStore` is currently type-erased to `Arc<dyn ObjectStore>` inside `make_cache_layer`, and
its `ObjectPrefetch` face is thrown away. Also the root prefix is consumed inside
`connect_with_layer` and never surfaced. Both are needed. Two coupled changes, both in crates that
already depend on `object-cache` / `object_store` (no new heavy dep on the base `telemetry` crate —
`object-cache` pulls `foyer`/`reqwest`, which must **not** be dragged into `telemetry`):

**(a) `telemetry`: expose URL parsing so the caller gets the root.** Refactor
`BlobStorage::connect_with_layer` to delegate its parse to a new public associated fn (keeps the
env-var lowercasing in one place):

```rust
// rust/telemetry/src/blob_storage.rs
pub fn parse_url_opts(object_store_url: &str) -> Result<(Arc<dyn ObjectStore>, Path)> {
    let (blob_store, root) = object_store::parse_url_opts(
        &url::Url::parse(object_store_url)?,
        std::env::vars().map(|(k, v)| (k.to_lowercase(), v)),
    )?;
    Ok((Arc::new(blob_store), root))
}
```

`connect_with_layer` now calls `parse_url_opts` then applies the layer + `PrefixStore` as before.
`BlobStorage::new(store, root)` (`blob_storage.rs:19`) already wraps in `PrefixStore` and is reused
below. No `object-cache` dependency is added to `telemetry`.

**(b) `ingestion`: build the cache client once, use it twice, compose the prefix.** Replace
`make_cache_layer` (which returns only a store-transform) with a helper returning both faces:

```rust
// rust/ingestion/src/data_lake_connection.rs
/// Wrap `direct` with the object cache when configured, returning the store
/// layer and — when enabled — the same client's `ObjectPrefetch` face for
/// write-time warming.
fn make_cache(direct: Arc<dyn ObjectStore>)
    -> (Arc<dyn ObjectStore>, Option<Arc<dyn ObjectPrefetch>>) {
    let cache_url = std::env::var("MICROMEGAS_OBJECT_CACHE_URL").ok();
    let api_key = std::env::var("MICROMEGAS_OBJECT_CACHE_API_KEY").ok();
    match cache_url {
        Some(url) if api_key.is_some() => {
            let client = Arc::new(CacheClientStore::new(url, api_key, direct));
            (client.clone() as Arc<dyn ObjectStore>, Some(client as Arc<dyn ObjectPrefetch>))
        }
        Some(url) => { // URL without key: disabled, warn (preserve current behavior)
            warn!("MICROMEGAS_OBJECT_CACHE_URL is set ({url}) but MICROMEGAS_OBJECT_CACHE_API_KEY is missing: the object cache is disabled");
            (direct, None)
        }
        None => (direct, None),
    }
}
```

Both `connect_to_data_lake` and `connect_to_remote_data_lake` change to:

```rust
let (raw_store, root) = BlobStorage::parse_url_opts(object_store_url)?;
let (layered, prefetch_client) = make_cache(raw_store);
let blob_storage = Arc::new(BlobStorage::new(layered, root.clone()));
let prefetch = prefetch_client
    .map(|p| Arc::new(PrefixPrefetch::new(p, root)) as Arc<dyn ObjectPrefetch>);
// … db pool (and migrate for remote) …
Ok(DataLakeConnection::new_with_prefetch(pool, blob_storage, prefetch))
```

This preserves the exact layering (`layer` applied to the full-bucket store, then `PrefixStore(root)`)
and additionally hands the root to `PrefixPrefetch` so warm keys line up with read keys.

**`DataLakeConnection` gains an optional prefetch field** (`data_lake_connection.rs:11`):

```rust
pub struct DataLakeConnection {
    pub db_pool: PgPool,
    pub blob_storage: Arc<BlobStorage>,
    prefetch: Option<Arc<dyn ObjectPrefetch>>, // None when the cache is not configured
}
```

Keep the existing `new(db_pool, blob_storage)` constructor working (it sets `prefetch: None`) so
the `readiness.rs` test and any other direct callers are untouched (open/closed). Add
`new_with_prefetch(db_pool, blob_storage, prefetch)` for the connect functions.

### 3. Fire-and-forget warm at the write choke point

Add a best-effort method on `DataLakeConnection`:

```rust
/// Warm a freshly-written partition in the object cache. Fire-and-forget at
/// prefetch priority: spawns a detached task and returns immediately, so the
/// write/materialization path is never delayed or failed by a warm. No-op when
/// the cache is not configured or the partition is empty. Returns the spawned
/// task handle (or None) purely so tests can await completion deterministically;
/// production callers ignore it.
pub fn warm_partition(&self, file_path: &str, file_size: i64) -> Option<JoinHandle<()>> {
    let prefetch = self.prefetch.as_ref()?.clone();
    if file_size <= 0 { return None; } // empty partitions have no object to warm
    let item = PrefetchItem { key: file_path.to_string(), size: file_size as u64, ranges: None };
    imetric!("write_time_partition_warm_requested", "count", 1_u64);
    Some(spawn_with_context(async move {
        match prefetch.prefetch(vec![item]).await {
            Ok(resp) => debug!(
                "write-time warm enqueued accepted={} rejected={} dropped={}",
                resp.accepted, resp.rejected, resp.dropped
            ),
            // CacheClientStore::prefetch already bumps range_cache_client_prefetch_error;
            // keep this at debug — a failed warm just means the first read is a cold miss.
            Err(e) => debug!("write-time warm failed for {file_path}: {e}"),
        }
    }))
}
```

`spawn_with_context` (`rust/tracing/src/spans/instrumented_future.rs:96`, reachable from
`ingestion` via `micromegas_tracing`) keeps the warm inside the tracing span context, matching how
the write itself is spawned.

**Hook** in `write_partition_from_rows` (`write_partition.rs:649`), after `insert_partition`
succeeds — so a warm is only ever requested for a partition that is durable in S3 *and* committed
to PostgreSQL:

```rust
insert_partition(&lake, &Partition { /* … */ file_path: result.file_path.clone(), /* … */ }, result.file_metadata.as_ref(), logger)
    .await
    .with_context(|| "insert_partition")?;

if let Some(file_path) = &result.file_path {
    // fire-and-forget; ignore the handle in production
    lake.warm_partition(file_path, result.file_size);
}
Ok(())
```

`result.file_path` is currently moved into the `Partition` at `write_partition.rs:657`; clone it
(or reorder so the warm reads it before the move) — a single `String` clone per partition write,
negligible.

### Why this shape

- **One hook, all writers.** Placing the warm in `write_partition_from_rows` covers daemon, JIT,
  and merge with no per-caller change and no chance of a new writer forgetting to warm.
- **Read-only cache preserved.** The write path only *names* the key; the cache pulls bytes from
  origin itself, so no write-ingest surface is added (the issue's central constraint).
- **Never on the critical path.** The detached spawn means the write returns without waiting on the
  cache; a slow/unreachable cache cannot delay or fail materialization. The client's own 2s
  connect / 15s request timeouts (`client.rs:22-25`) bound the detached task.
- **Prefix correctness by construction.** `PrefixPrefetch` reuses `Path`-parts composition, the
  same primitive `PrefixStore` uses, so warm and read keys can't drift.

## Implementation Steps

### Phase 1 — prefix adapter (`object-cache`)
1. Add `PrefixPrefetch` to `rust/object-cache/src/prefetch.rs` and export it from
   `rust/object-cache/src/lib.rs`.

### Phase 2 — wiring (`telemetry` + `ingestion`)
2. `rust/telemetry/src/blob_storage.rs`: add `pub fn parse_url_opts(url) -> Result<(Arc<dyn ObjectStore>, Path)>`;
   refactor `connect_with_layer` to use it.
3. `rust/ingestion/src/data_lake_connection.rs`: replace `make_cache_layer` with `make_cache`
   (returns store + optional `ObjectPrefetch`); add `prefetch` field to `DataLakeConnection`; keep
   `new`, add `new_with_prefetch`; add `warm_partition`; rewrite `connect_to_data_lake` per §2.
4. `rust/ingestion/src/remote_data_lake.rs`: rewrite `connect_to_remote_data_lake` the same way
   (keeps `migrate_db`).

### Phase 3 — hook the write path (`analytics`)
5. `rust/analytics/src/lakehouse/write_partition.rs`: after `insert_partition` succeeds in
   `write_partition_from_rows`, call `lake.warm_partition(file_path, result.file_size)` for a
   non-empty partition (clone `file_path` before it moves into `Partition`).

### Phase 4 — metrics, docs, tests
6. Metrics (below). 7. Docs (below). 8. Tests (below).

## Files to Modify
- `rust/object-cache/src/prefetch.rs` — `PrefixPrefetch` adapter.
- `rust/object-cache/src/lib.rs` — export `PrefixPrefetch`.
- `rust/telemetry/src/blob_storage.rs` — `parse_url_opts`; `connect_with_layer` uses it.
- `rust/ingestion/src/data_lake_connection.rs` — `make_cache`, `prefetch` field,
  `new_with_prefetch`, `warm_partition`, `connect_to_data_lake` rewrite.
- `rust/ingestion/src/remote_data_lake.rs` — `connect_to_remote_data_lake` rewrite.
- `rust/analytics/src/lakehouse/write_partition.rs` — warm hook after `insert_partition`.
- `rust/object-cache/tests/prefetch_tests.rs` — `PrefixPrefetch` unit test (append if the file
  exists; the crate's tests live under `tests/` per project convention).
- `rust/ingestion/tests/` — `warm_partition` behavior test (new file, e.g. `warm_partition_tests.rs`).
- `mkdocs/docs/admin/object-cache.md` — document write-time warming.
- `rust/object-cache-srv/README.md` (or `rust/ingestion/README.md` if present) — mention the
  producer side of `/prefetch` warming, if a natural spot exists.
- Changelog entry.

## Metrics
- `write_time_partition_warm_requested` — a warm was scheduled (per non-empty partition write).
- Reuse `range_cache_client_prefetch_error` (already emitted by `CacheClientStore::prefetch`) for
  failures — no new error metric needed. Server-side `object_cache_prefetch_keys_warmed` /
  `object_cache_prefetch_dropped` (already exist) show the warm actually landing / being shed.

## Trade-offs
- **Hook in `write_partition_from_rows` vs per-caller.** The shared choke point is DRY and
  future-proof (new writers inherit warming). Alternative — warm in each spec/merge caller — is
  more code and error-prone. Chosen: single hook.
- **`DataLakeConnection` holds the prefetch face vs `BlobStorage` holds it.** `BlobStorage` lives in
  the base `telemetry` crate, which must stay free of the heavy `object-cache` deps (`foyer`,
  `reqwest`). `DataLakeConnection` lives in `ingestion`, which already depends on `object-cache`, so
  the face lives there. `telemetry` only gains a dependency-free `parse_url_opts`.
- **`PrefixPrefetch` adapter vs prepending inline at the call site.** The adapter localizes the
  prefix rule to one tested place and reuses `PrefixStore`'s exact composition primitive; an inline
  `format!` at the write site would risk drifting from `PrefixStore` (double slashes, empty-root
  edge). Chosen: adapter.
- **Fire-and-forget detached spawn vs inline await.** Awaiting the POST (even though it returns
  `202` quickly) adds an HTTP round-trip to every materialization and couples write latency to cache
  availability. Detached spawn keeps the write path clean; the cost is that a warm failure is only
  observable via metrics/logs, which is acceptable for a best-effort optimization. Returning the
  `JoinHandle` keeps it awaitable in tests without changing production behavior.
- **Automatic when cache is configured vs an explicit on/off knob.** Warming is on whenever the
  cache is configured (URL + key present). A separate enable flag is unnecessary complexity for now;
  see Open Questions.
- **Whole-object warm vs ranged.** A new partition will be scanned broadly by the follow-up query,
  so warm the whole object (`ranges: None`). Ranged warming is a future refinement (would need the
  query's projected byte ranges, which the write path doesn't have) and is #1200's territory.

## Documentation
- `mkdocs/docs/admin/object-cache.md`: add a "Write-time warming" subsection — after a partition is
  written, the producer POSTs its key to `/prefetch` (fire-and-forget, prefetch priority); enabled
  automatically when `MICROMEGAS_OBJECT_CACHE_URL` + `MICROMEGAS_OBJECT_CACHE_API_KEY` are set on
  the writing service; one extra origin GET per new partition; the cache stays read-only. Mention
  the `write_time_partition_warm_requested` metric and that failures surface via
  `range_cache_client_prefetch_error`.
- Changelog entry.

## Testing Strategy
- **Unit — `PrefixPrefetch` (`object-cache`)**: a mock `ObjectPrefetch` capturing received items;
  assert keys are prefixed to match `PrefixStore` for (a) a non-empty root, (b) an empty root
  (`Path::default()`, keys unchanged), and — to lock the "matches read key" contract — compare the
  produced key against `PrefixStore::new(dummy, root)`'s effective path for the same input.
- **Unit/integration — `warm_partition` (`ingestion`)**: build a `DataLakeConnection` via
  `new_with_prefetch` with a mock `ObjectPrefetch` that records calls;
  - non-empty partition → exactly one `PrefetchItem` with the given key and `size`, `ranges: None`
    (await the returned `JoinHandle` for determinism);
  - `file_size == 0` or `file_path == None` → no call, `None` returned;
  - `prefetch == None` (cache off) → no call, `None` returned, no panic;
  - mock returning `Err` → `warm_partition`'s task completes without propagating (write path
    unaffected).
- **End-to-end smoke**: `start_minio.py` + object-cache-srv + `start_services.py` with
  `MICROMEGAS_OBJECT_CACHE_URL`/`_API_KEY` set; ingest telemetry, force materialization of a
  partition, then confirm the cache warmed it — `object_cache_prefetch_keys_warmed` incremented and
  a subsequent query's demand read is a cache hit (no new origin GET). Verify the key the cache
  received equals `root/views/…` (prefix applied).
- **CI**: `cd rust && python3 ../build/rust_ci.py` (fmt, clippy `-D warnings`, tests).

## Open Questions
1. **Explicit enable/disable knob?** Warming is currently coupled to "cache configured". If an
   operator wants the cache for reads but not write-time warming (e.g. to avoid the extra origin GET
   per partition during a bulk backfill), a `MICROMEGAS_OBJECT_CACHE_WRITE_WARM` off-switch would be
   needed. Recommendation: ship without it (simpler); add later if a use case appears.
2. **Warm on JIT single-block partitions?** JIT can create tiny single-block partitions
   (`begin_insert == end_insert`). Warming them is cheap and harmless (one small origin GET), so the
   plan warms uniformly rather than special-casing. Flag only if the extra GET volume from
   high-churn JIT views proves noticeable — observable via `write_time_partition_warm_requested`.
</content>
</invoke>
