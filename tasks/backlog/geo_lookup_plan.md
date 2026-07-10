# Per-Process Geographic Data Plan

## Overview

Resolve the process IP to geographic data at ingestion time and store configurable fields (IP, country, city, lat/long) as process properties. Each field is independently toggled, allowing operators to match their privacy requirements — from storing nothing to full detail.

## Current State

- **Process model** (`rust/ingestion/src/sql_telemetry_db.rs:24-48`): The `processes` table has a `properties micromegas_property[]` column — a key-value store that flows through all views and is queryable via `property_get(properties, 'key')`.
- **Ingestion handler** (`rust/public/src/servers/ingestion.rs:49-54`): `insert_process_request` takes only `Extension(service)` and `body` — no access to HTTP request parts.
- **Remote IP extraction** (`rust/public/src/servers/http_utils.rs:11-40`): `get_client_ip(headers, extensions)` (to be renamed `get_remote_ip`) already handles X-Forwarded-For, X-Real-IP, and ConnectInfo fallback.
- **Insert logic** (`rust/ingestion/src/web_ingestion_service.rs:152-187`): `insert_process(body)` deserializes CBOR into `ProcessInfo`, calls `make_properties(&process_info.properties)` to persist the properties HashMap.
- **Object store** (`rust/telemetry/src/blob_storage.rs`): `BlobStorage` wraps `object_store` crate, configured via `MICROMEGAS_OBJECT_STORE_URI`. Uses `parse_url_opts` pattern.
- **Env var config patterns** (`rust/auth/src/oidc.rs:150-154`): JSON config from env var via `serde_json::from_str`, e.g. `MICROMEGAS_OIDC_CONFIG`.

## Design

### Data flow

```
Client process ──HTTP POST──> ingestion-srv ──┬──> extract process IP
                                              ├──> resolve via MMDB (if configured)
                                              ├──> store enabled fields as properties
                                              ▼
                                    PostgreSQL processes.properties
                                      'process_ip'      (if store_ip enabled)
                                      'geo_country'    (if store_country enabled)
                                      'geo_city'       (if store_city enabled)
                                      'geo_latitude'   (if store_latitude enabled)
                                      'geo_longitude'  (if store_longitude enabled)
```

### Configuration

Two independent settings:

**`MICROMEGAS_STORE_PROCESS_IP`** (boolean env var, default `false`)

Controls whether the raw process IP is stored as `process_ip` in process properties. Independent of geo resolution — no MMDB needed.

**`MICROMEGAS_GEO_CONFIG`** (JSON env var, optional)

Controls MMDB-based geo resolution:

```json
{
  "mmdb_uri": "s3://bucket/path/GeoLite2-City.mmdb",
  "store_country": true,
  "store_city": true,
  "store_latitude": false,
  "store_longitude": false
}
```

- If `MICROMEGAS_GEO_CONFIG` is not set, geo resolution is disabled entirely.
- `mmdb_uri` is required.
- Each `store_*` flag defaults to `false`.
- Both settings can be combined: store IP + geo, geo only (IP discarded after resolution), IP only (no MMDB needed), or neither.

This follows the existing JSON-from-env pattern used by `MICROMEGAS_OIDC_CONFIG` and `MICROMEGAS_API_KEYS`.

### MMDB loading and refresh

Load at ingestion service startup via `object_store::parse_url_opts()` (same pattern as `BlobStorage::connect()`). The reader wraps `maxminddb::Reader<Vec<u8>>` — no mmap, since the file comes from object storage. Only loaded if at least one geo field is enabled.

The reader is held behind `Arc<ArcSwap<maxminddb::Reader<Vec<u8>>>>`. `ArcSwap` allows the reader to be atomically replaced without blocking lookups — readers on the hot path call `arc_swap.load()` which is wait-free.

A background task periodically checks the object store for updates (using the object's `last_modified` / `e_tag` metadata via `ObjectStore::head()`). If the file has changed, it downloads the new MMDB, constructs a new reader, and swaps it in. In-flight lookups using the old reader complete normally; the old data is dropped when the last reference is released.

Config:
```json
{
  "mmdb_uri": "s3://bucket/path/GeoLite2-City.mmdb",
  "refresh_interval_seconds": 3600,
  ...
}
```

`refresh_interval_seconds` defaults to `3600` (hourly). Set to `0` to disable refresh (load once at startup). When enabled, a `tokio::spawn` background task runs at that interval. The background task logs on successful refresh and on errors (but does not stop the service on failure — the previous reader remains active).

### SQL usage after implementation

```sql
-- Processes by country
SELECT property_get(properties, 'geo_country') as country, count(*) as cnt
FROM processes
GROUP BY country
ORDER BY cnt DESC;

-- Full geo detail (when all fields enabled)
SELECT process_id, exe, computer,
       property_get(properties, 'process_ip') as ip,
       property_get(properties, 'geo_country') as country,
       property_get(properties, 'geo_city') as city,
       property_get(properties, 'geo_latitude') as lat,
       property_get(properties, 'geo_longitude') as lng
FROM processes;
```

## Implementation Steps

### Phase 0: Rename `get_client_ip` to `get_remote_ip`

Rename across the codebase to use domain-neutral terminology:
- `rust/public/src/servers/http_utils.rs` — function definition and doc comments
- `rust/public/src/servers/axum_utils.rs` — call site
- `rust/public/src/servers/http_gateway.rs` — call site
- `rust/public/src/servers/log_uri_service.rs` — call site

### Phase 1: MMDB reader and geo config module

1. Add `maxminddb = "0.24"` to workspace deps in `rust/Cargo.toml`.
   Add `maxminddb.workspace = true` to `rust/ingestion/Cargo.toml`.

2. Create `rust/ingestion/src/geo.rs`:
   - `GeoConfig` struct (deserialized from JSON):
     ```
     mmdb_uri: String
     refresh_interval_seconds: u64  // default 3600 (hourly)
     store_country: bool
     store_city: bool
     store_latitude: bool
     store_longitude: bool
     ```
     All `store_*` fields default to `false` via serde defaults.
   - `GeoConfig::from_env() -> Result<Option<GeoConfig>>` — reads `MICROMEGAS_GEO_CONFIG`, returns `None` if unset.
   - `GeoResolver` struct holding `GeoConfig` + `Arc<ArcSwap<maxminddb::Reader<Vec<u8>>>>` + object e_tag for change detection
   - `GeoResolver::new(config) -> Result<Self>` — loads MMDB from `mmdb_uri`
   - `GeoResolver::enrich(&self, properties: &mut HashMap<String, String>, process_ip: &str)` — loads current reader via `arc_swap.load()` (wait-free), resolves IP, inserts enabled fields. Lookup failures silently skipped.
   - `GeoResolver::start_refresh_task(self: &Arc<Self>)` — if `refresh_interval_seconds > 0`, spawns a tokio task that periodically checks object metadata and swaps in a new reader when the file changes
   - Separate `store_process_ip_enabled() -> bool` function reads `MICROMEGAS_STORE_PROCESS_IP` env var.

3. Add `pub mod geo;` to `rust/ingestion/src/lib.rs`.

### Phase 2: Integrate into ingestion service

4. In `rust/ingestion/src/web_ingestion_service.rs`:
   - Add `geo: Option<Arc<GeoResolver>>` and `store_process_ip: bool` fields to `WebIngestionService`
   - Update `new()` to accept both
   - In `insert_process()`, add `process_ip: Option<String>` parameter
   - If `store_process_ip` is true, insert `process_ip` into `process_info.properties`
   - If `geo` is available, call `geo.enrich(&mut process_info.properties, &process_ip)` before `make_properties()`

5. In `rust/public/src/servers/ingestion.rs`:
   - Add `ConnectInfo<SocketAddr>` and `HeaderMap` extractors to `insert_process_request`
   - Use `get_process_ip()` from `http_utils.rs` to extract the IP
   - Pass to `service.insert_process(body, Some(process_ip))`

### Phase 3: Wire up in ingestion service binary

6. In `rust/telemetry-ingestion-srv/src/main.rs`:
   - Read `MICROMEGAS_STORE_PROCESS_IP` (bool, default false)
   - Call `GeoConfig::from_env()`
   - If geo config present, create `GeoResolver::new(config)`, wrap in `Arc`
   - Pass both to `WebIngestionService::new(lake, store_process_ip, geo_resolver)`

## Files to Modify

| File | Change |
|------|--------|
| `rust/Cargo.toml` | Add `maxminddb`, `arc-swap` workspace deps |
| `rust/ingestion/Cargo.toml` | Add `maxminddb.workspace = true`, `arc-swap.workspace = true` |
| `rust/ingestion/src/lib.rs` | Add `pub mod geo` |
| `rust/ingestion/src/web_ingestion_service.rs` | Add `geo` field, call `enrich()` |
| `rust/public/src/servers/ingestion.rs` | Extract process IP from request |
| `rust/telemetry-ingestion-srv/src/main.rs` | Load geo config and resolver |

## New Files

| File | Purpose |
|------|---------|
| `rust/ingestion/src/geo.rs` | `GeoConfig`, `GeoResolver`, MMDB loading |

## Dependencies

- `maxminddb = "0.24"` — ISC license (permissive, compatible with Apache-2.0). Provides `Reader::from_source(Vec<u8>)` for in-memory MMDB access. Sub-microsecond lookups.
- `arc-swap = "1"` — Apache-2.0/MIT. Wait-free atomic pointer swap for hot-path reader access during MMDB refresh.

## Trade-offs

**Resolve at ingestion vs. query time**: Resolving at ingestion means you can't re-resolve against an updated MMDB for historical data. But it avoids storing the IP when not needed, and keeps the analytics layer unchanged. GeoIP databases change slowly, so this trade-off is acceptable.

**No analytics-side changes**: Since the resolved geo data is stored as plain properties, no UDFs, no session configurator changes needed. The analytics side just works — `property_get(properties, 'geo_country')` in any existing query.

**JSON config for geo, simple env var for IP**: `store_ip` is a simple on/off unrelated to MMDB, so a boolean env var fits. The geo fields are interrelated (all need the MMDB) so grouping them in one JSON config makes sense.

**All fields default to false**: Secure by default. Operators must explicitly opt in to each piece of data they want stored. This aligns with data minimization principles.

**ArcSwap for hot reload**: `ArcSwap` adds a dependency but avoids `RwLock` contention on every lookup. The read path (`load()`) is wait-free — no lock, no CAS, just an atomic pointer read. The write path (swap) only happens on refresh. This is the right trade-off for a high-throughput ingestion service where every request hits the reader.

## Privacy Notes

Each field has different privacy implications — the independent toggles let operators match their legal requirements:

| Field | Privacy risk | Notes |
|-------|-------------|-------|
| `store_ip` | High | IP is personal data under GDPR. Only enable if needed and retention policy is in place. |
| `store_country` | Low | Not personally identifiable. Safe under all frameworks. |
| `store_city` | Moderate | Can localize individuals in small towns. Flag for legal review if minors may be users. |
| `store_latitude/longitude` | Elevated | City-centroid accuracy (~5-25km) but coordinates feel more surveillance-adjacent. Review under GDPR/CPRA/CAADCA if children may be users. |

## Documentation

- `mkdocs/docs/admin/` — Document `MICROMEGAS_GEO_CONFIG` env var with JSON schema and privacy guidance per field

## Testing Strategy

1. **cargo build** — verify compilation with new dependency
2. **cargo test** — unit tests for `geo.rs`:
   - Config parsing with various field combinations
   - `needs_mmdb()` logic
   - Resolution with a test MMDB file
   - Verify only enabled fields are injected into properties
   - Verify graceful handling of lookup failures
   - Verify refresh: swap a new MMDB into the ArcSwap, confirm subsequent lookups use the new data
3. **cargo clippy --workspace -- -D warnings**
4. **cargo fmt**
5. **Integration**: Start services with `MICROMEGAS_GEO_CONFIG` set (country+city enabled), send telemetry, verify `property_get(properties, 'geo_country')` returns values. Verify disabled fields are absent from properties.
