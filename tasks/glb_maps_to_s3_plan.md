# Move GLB Maps to Object Store

## Overview

The Map cell's GLB binaries and catalog used to live under
`analytics-web-app/public/maps/`, served by the analytics web tier's
static `ServeDir`. This change moves them off the web tier and into an
**object store** accessed through `analytics-web-srv` via the
`object_store` crate — same URL grammar (`s3://`, `gs://`, `file://`,
`memory://`) that ingestion and analytics use under the hood, without
depending on the telemetry-shaped `BlobStorage` wrapper.

`maps.json` is gone. The catalog is derived at request time by listing
the configured object-store prefix — what you see is exactly what's in
the bucket, no second source of truth to keep in sync.

Why this shape (and not browser-direct-to-S3):

- Matches the URL grammar the rest of the platform uses (one env var,
  `object_store::parse_url_opts` with lowercased env-var pass-through for
  credentials).
- `file://` URLs trivially support local dev — no special fallback code path.
- The backend already gates the rest of the app behind cookie auth, so map
  access inherits the same access control without per-tenant signed-URL
  plumbing.
- No CORS configuration on the bucket: requests are same-origin to
  `analytics-web-srv`.

Trade-off (no CDN edge caching) is acceptable for the current scale — maps
load once per browser session and `useGLTF` caches them in-memory. A CDN
in front of `analytics-web-srv` is a deployment-layer change later if
needed.

## Divergences from the original plan

These came up mid-implementation and shaped the final result:

1. **Filename validation simplified.** The plan called for a strict
   `^[A-Za-z0-9._-]+\.glb$` regex on both catalog entries and blob
   lookups. Replaced with `is_direct_child` (reject anything containing
   `/`). Rationale: the prefix is reserved for map assets, so the
   `.glb` extension and character set aren't worth enforcing
   server-side; axum's `{filename}` path capture is already a single
   segment, and `object_store::path::Path` keys are opaque (no `..`
   traversal). One `/`-check is the only defense-in-depth needed.

2. **CompressionLayer exclusion via router structure, not predicate.**
   The plan offered both options (content-type predicate vs separate
   branch). We mounted the blob route on its own router branch
   (`build_protected_maps_blob_route`) so the global `CompressionLayer`
   never sits in front of it. No content-type strings in the predicate
   — the structure makes the exclusion obvious.

3. **Local-dev default moved to the lake.** Initially defaulted to
   `file://.../analytics-web-app/public/maps/`. Reworked so the start
   script derives `MICROMEGAS_MAPS_OBJECT_STORE_URI` as
   `<MICROMEGAS_OBJECT_STORE_URI>/maps/` — sibling of the existing
   telemetry `blobs/` directory. The `public/maps/` directory was
   deleted entirely; no `.gitkeep`, no gitignore entry for `*.glb`.

## Asset layout (new)

Storage layout is flat under one URI per environment:

```
<MICROMEGAS_MAPS_OBJECT_STORE_URI>/
├── main.glb
├── topdown.glb
└── Arena_North_01.glb
```

| Environment | Example URI |
|---|---|
| Local dev | `file:///home/you/lake/maps/` (sibling of `lake/blobs/`) |
| AWS prod  | `s3://my-bucket/maps/` |
| Tests     | `memory:///` (in-process) |

The workspace `object_store` dep enables `["aws"]` only; `s3://`,
`file://`, and `memory://` are the supported schemes. Adding GCS
(`gs://`) or Azure later is a matter of enabling the corresponding
`object_store` feature in `rust/Cargo.toml`.

## Env var

| Var | Purpose | Default |
|---|---|---|
| `MICROMEGAS_MAPS_OBJECT_STORE_URI` | URI of the maps store (any `object_store::parse_url_opts` form) | Unset by default. `start_analytics_web.py` derives it from `MICROMEGAS_OBJECT_STORE_URI` (appending `/maps/`) when that var is set. Unset = catalog endpoint returns 503; frontend dropdown shows an empty list with an explanatory hint. |

Deliberately **separate** from `MICROMEGAS_OBJECT_STORE_URI` (the telemetry
data lake), because:

- They have different lifecycles (telemetry is hot/append-only; maps are
  cold/replace-in-place).
- They may live in different buckets / accounts with different access
  policies.
- A deployment may want telemetry but no map cells, or vice versa.

A deployment that wants to share one lake just lets the start script's
default do its thing (or sets both vars to the same prefix).

## Backend

### `rust/analytics-web-srv/src/maps.rs` (new)

- `MapsState { store: Option<Arc<dyn ObjectStore>> }` — wrapped in
  `Extension<MapsState>` to match the local convention (every existing
  protected handler takes its dependencies via `Extension<T>`).
- `connect_maps_store(uri: Option<&str>)` — the 5-line `parse_url_opts`
  + `PrefixStore::new` idiom from `telemetry/src/blob_storage.rs`,
  duplicated here rather than reaching into the telemetry wrapper
  (different shape: we need `list` + streaming `get`, not eager `put`
  / `read_blob`).
- `is_direct_child(name: &str)` — non-empty, contains no `/`. Used by
  both the catalog filter and the blob handler.

**`GET /api/maps/catalog`**

- Storage unconfigured (`store.is_none()`) → `503 Service Unavailable`.
- Lists the prefixed store (`store.list(None)`), filters to direct
  children only (no `/` in the relative location returned by
  `PrefixStore`), alphabetizes, returns JSON:
  ```json
  [
    { "file": "Arena_North_01.glb", "size": 47185920 },
    { "file": "main.glb", "size": 2516582 }
  ]
  ```
- `size` is the upstream `ObjectMeta.size`. `Cache-Control: no-cache`.

Display name is derived client-side (strip `.glb`; underscores are
preserved) — the server doesn't store or serve names.

**`GET /api/maps/blob/{filename}`**

- Validate `is_direct_child(&filename)` — rejects anything containing
  `/` (e.g. percent-encoded forms decoded into the segment). Anything
  else → `400 Bad Request`.
- `store.get(&Path::from(filename))` returns a `GetResult` whose
  `.into_stream()` is `Stream<Item = object_store::Result<Bytes>>`.
- Errors are mapped to `std::io::Error` so the body works with
  `axum::body::Body::from_stream`.
- Response headers:
  - `Content-Type`: pass through from `GetResult.attributes`, default
    `model/gltf-binary`.
  - `Content-Length` from `GetResult.meta.size`. The stored object's
    size *is* the wire size — we never re-encode on the fly.
  - `Content-Encoding`: pass through from `GetResult.attributes` when
    present (`Attribute::ContentEncoding`). Pre-gzipped objects in the
    bucket flow through verbatim; the browser transparently decodes.
  - `Cache-Control: public, max-age=3600` — modest cache.
- `NotFound` → `404`. Other object-store errors → `500` with the error
  string logged.

### Storage compression convention

GLBs in the bucket may be stored gzipped, with the object's
`Content-Encoding` metadata set to `gzip` and `Content-Type` to
`model/gltf-binary`. The endpoint passes both through verbatim and
never re-encodes — the server burns no CPU compressing on the fly,
`Content-Length` reflects the wire size, and the browser auto-decodes
before handing bytes to three.js.

Bare uploads (no metadata) are served uncompressed. The convention is
"gzipped when the upload tooling writes it"; degraded uploads still
work.

### CompressionLayer exclusion

The global `CompressionLayer` MUST NOT sit in front of the maps blob
route:

- Pre-gzipped objects served verbatim would be safe (tower-http skips
  compression when `Content-Encoding` is already set), but…
- Plain GLBs would be re-encoded on every request: 30 MB of CPU work
  per fetch, and `Content-Length` is dropped because compression
  switches to chunked transfer.

Implementation: the blob route lives on its own router branch
(`build_protected_maps_blob_route`) that is merged into the app
**after** `CompressionLayer` has been applied to the JSON/HTML branch.
Same auth + Extension + observability layers as the rest of the
protected routes — only the compression layer is missing. The catalog
route (small JSON) stays under the global layer.

### Authentication

**Both endpoints are registered inside `build_protected_routes`** /
`build_protected_maps_blob_route`, both wrapped by
`auth::cookie_auth_middleware`. Anonymous requests get 401 with no
body — no map binaries are served, and the catalog isn't enumerable.

Caveat: `--disable-auth` dev mode bypasses the middleware (a synthetic
`ValidatedUser` is injected instead). This is the same posture as
every other `/api` endpoint; not a new attack surface.

The auth-regression guard test (`maps_tests.rs`) wraps the routes with
the real middleware and asserts unauthenticated requests get 401 and
that the body doesn't leak the catalog or any GLB bytes.

### Wiring in `main.rs`

1. Read `MICROMEGAS_MAPS_OBJECT_STORE_URI` once at startup.
2. Call `maps::connect_maps_store(...)` — fail-fast on error. Wrap the
   `Option<Arc<dyn ObjectStore>>` in `MapsState`.
3. `build_protected_routes` registers `/api/maps/catalog` (compressed)
   and `.layer(Extension(MapsState))`.
4. `build_protected_maps_blob_route` registers `/api/maps/blob/{filename}`
   on a separate branch (no compression) with its own auth + Extension
   layers; merged into the app after `CompressionLayer`.
5. No `ServeDir` for the maps prefix — bypassing the handlers would
   bypass auth.

### Dropped grid fallback

`MapViewer.tsx` used to render a drei `<Grid>` when `mapUrl` was empty,
paired with a `<option value="">None (grid only)</option>` in the
editor. Removed both. A user who wants a grid background uploads a
`grid.glb` like any other map — no special case in code.

Unset `mapUrl` is now a configuration error: `MapCell` early-returns
"No map selected. Open the editor and pick a map from the dropdown."
with the same `text-theme-text-muted text-sm` styling as the existing
"No spatial data available" message.

## Frontend

The endpoints sit at fixed paths relative to `basePath`, which the
frontend reads from `getConfig()`.

### `analytics-web-app/src/lib/maps-catalog.ts` (new)

- `fetchMapCatalog(basePath)` — module-level shared promise so multiple
  Map cells don't fetch the catalog twice. `credentials: 'include'`
  for cookie auth.
- `normalizeMapFilename(raw)` — strips a leading `/maps/` if present.
  Used in two places:
  - The renderer's URL composition (legacy `mapUrl="/maps/main.glb"`
    saved notebooks → still load).
  - The editor's `<select value={...}>` binding, so the dropdown
    shows the correct selected entry for legacy notebooks too.
- `resolveMapBlobUrl(file, basePath)` — calls `normalizeMapFilename`
  then composes `${basePath}/api/maps/blob/${file}`. Used by the
  renderer; saved notebooks store the bare filename, not the URL, so
  base-path changes don't invalidate them.
- `formatMapName(file)` — strips `.glb`. Underscores are preserved.

### `MapCell.tsx`

- Replaced inline `fetch('/maps/maps.json')` with the helper.
- `options.mapUrl` is now the bare filename; the renderer composes the
  blob URL at render time.
- Early-returns "No map selected" when `mapUrl` is unset.
- Editor dropdown drops the "None" option, normalizes the `value`
  binding, and shows new hint text:
  > "Maps are loaded from the server's object store
  > (`MICROMEGAS_MAPS_OBJECT_STORE_URI`). Drop `.glb` files at that
  > prefix to make them appear here."

### `MapViewer.tsx`

- `mapUrl` prop is now required (was optional).
- Dropped the `Grid` import from `@react-three/drei` and the
  `PlaceholderGrid` function.
- The `mapUrl ? <MapModel/> : <PlaceholderGrid/>` branch is just
  `<MapModel url={mapUrl}/>` now.

## Local dev

`start_analytics_web.py` derives the maps default from the telemetry
lake when present:

```python
if (
    "MICROMEGAS_MAPS_OBJECT_STORE_URI" not in os.environ
    and "MICROMEGAS_OBJECT_STORE_URI" in os.environ
):
    lake_uri = os.environ["MICROMEGAS_OBJECT_STORE_URI"].rstrip("/")
    env_vars["MICROMEGAS_MAPS_OBJECT_STORE_URI"] = f"{lake_uri}/maps/"
```

For a typical local setup with `MICROMEGAS_OBJECT_STORE_URI=file:///home/me/lake`,
this resolves to `file:///home/me/lake/maps/` — sibling of `lake/blobs/`.

`analytics-web-app/public/maps/` is gone entirely. The `.gitignore`
entries for `public/maps/maps.json` and `public/maps/*.glb` were
removed too. Developers drop GLBs into their lake's `maps/` directory
(or wherever `MICROMEGAS_MAPS_OBJECT_STORE_URI` points).

## Tests

### `rust/analytics-web-srv/tests/maps_tests.rs`

10 tests across two flavors:

**Auth-regression guard** (real middleware, no token):
- `unauthenticated_catalog_returns_401` — wraps the catalog route
  with `cookie_auth_middleware` (same wrapping `main.rs` applies).
  Sends a request with no cookie and no `Authorization` header → 401.
  The body is checked to not contain any catalog entries.
- `unauthenticated_blob_returns_401` — same shape for the blob route.
  Seeds a GLB with sentinel bytes; asserts the response body does not
  contain them.

Both work without a live OIDC server because `cookie_auth_middleware`
returns `Unauthorized` at the cookie-jar lookup before initializing
the OIDC provider.

**Handler behavior** (auth bypassed via injected `ValidatedUser`):
- `catalog_returns_503_when_storage_unconfigured`
- `blob_returns_503_when_storage_unconfigured`
- `catalog_lists_flat_entries_alphabetized_with_size` — seeds two GLBs
  and a nested object; asserts the nested one is excluded.
- `blob_streams_bytes_and_sets_content_type_and_length` — round-trips
  raw bytes; asserts default `Content-Type` and that no `Content-Encoding`
  is set when the upstream has none.
- `blob_passes_through_content_encoding_from_object_metadata` — seeds
  an object with `Content-Encoding: gzip` attribute; asserts header
  passes through.
- `blob_rejects_filenames_containing_slashes` — `..%2Fmain.glb` and
  `foo%2Fbar.glb` both → 400.
- `blob_serves_non_glb_extensions_from_prefix` — seeds `readme.txt`,
  fetches it successfully (no extension enforcement).
- `blob_returns_404_for_missing_object`.

Inline unit tests (`mod tests`) cover `is_direct_child`.

### `analytics-web-app/src/lib/__tests__/maps-catalog.test.ts`

12 tests covering `normalizeMapFilename`, `resolveMapBlobUrl` (including
legacy `/maps/` prefix), `formatMapName`, and `fetchMapCatalog` (caching,
fetch error, non-OK response).

## Files changed

- `rust/analytics-web-srv/Cargo.toml` — added `object_store`, `url`
- `rust/analytics-web-srv/src/maps.rs` (new) — handlers + `MapsState`
- `rust/analytics-web-srv/src/lib.rs` — export `maps`
- `rust/analytics-web-srv/src/main.rs` — env var, `MapsState`, route
  registration, separate-branch wiring for the blob route
- `rust/analytics-web-srv/tests/maps_tests.rs` (new)
- `analytics-web-app/src/lib/maps-catalog.ts` (new)
- `analytics-web-app/src/lib/__tests__/maps-catalog.test.ts` (new)
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/components/map/MapViewer.tsx` — dropped grid
- `analytics-web-app/start_analytics_web.py` — derive maps URI from
  lake URI
- `analytics-web-app/.gitignore` — removed `public/maps/` entries
- `analytics-web-app/public/maps/` — deleted (directory + `maps.json`)
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `analytics-web-app/README.md`

## Trade-offs

**Backend proxy vs browser-direct-to-S3.** Direct-to-S3 would win on
CDN edge caching and bandwidth offload, but lose on (a) consistency
with the rest of the codebase, (b) auth (would need presigning), (c)
CORS configuration. A CDN in front of `analytics-web-srv` would
recover most of the caching benefit without changing service code.

**Prefix-listing vs DB-backed catalog.** A `maps_catalog` table would
let us audit / tag / per-tenant the catalog, but doubles the moving
parts. Listing the prefix means "drop a `.glb` in the bucket and
you're done" — no second source of truth.

**Streaming `get` vs buffered read.** GLBs are 30 MB; buffered reads
would pin one copy in server memory per concurrent request.
`Body::from_stream` over `GetResult.into_stream()` keeps memory flat.
Also one concrete reason `BlobStorage::read_blob` doesn't fit — it's
eager-buffered by design.

**Reserve-the-prefix filename policy vs strict regex.** Original plan
called for `^[A-Za-z0-9._-]+\.glb$`. Simplified to "no `/`" because
(a) the prefix is committed to map assets, (b) axum routing already
constrains to a single segment, (c) `object_store::path::Path` is
opaque. If the policy ever needs tightening, the check is one
function.

**Separate maps URI vs reusing `MICROMEGAS_OBJECT_STORE_URI`.** A
single URI would be simpler but couples deployment lifecycles. The
two-var form costs one line of deployment config and keeps the two
stores independent — and the local-dev start script auto-derives one
from the other anyway.

**`object_store` directly vs `BlobStorage`.** The existing wrapper is
shaped for telemetry payload R/W — eager `read_blob`, `put`,
`delete_batch`. Using it would mean reaching past the abstraction for
both the streaming GET and the `list` call. Cost of not using it: ~5
lines of `parse_url_opts` + `PrefixStore::new` duplicated; same idiom
already duplicated across several call sites; one more is fair for
keeping the web tier off the telemetry crate.

## Open follow-ups

1. **Quotas / size limits.** Should the blob endpoint reject objects
   above some size (e.g. 100 MB)? Probably yes as a safety rail; pick
   a number with ops.
2. **Upload tooling / management UI.** Planned follow-up — a UI inside
   the web app for listing, uploading (with the gzip + `Content-Encoding`
   convention handled transparently), renaming, and deleting maps.
   Bootstrap for now is `aws s3 cp ./main.glb s3://bucket/maps/` or
   plain `cp` for `file://` dev.
3. **CDN.** Out of scope. The streaming endpoint is a fine shape to
   put CloudFront / Cloudflare in front of later if traffic grows.
