# Map Management UI Plan

Follow-up to `tasks/completed/glb_maps_to_s3_plan.md`, addressing
[issue #1050](https://github.com/madesroches/micromegas/issues/1050).

## Overview

Add an admin-only UI in the analytics web app to upload and delete map
GLBs in the configured object store. The server-side handler gzips the
body on upload and writes the object with
`Content-Type: model/gltf-binary` + `Content-Encoding: gzip` so the
existing read path serves it verbatim and the browser transparently
decodes.

Goal: remove the operational footgun of remembering
`aws s3 cp … --content-encoding gzip --content-type model/gltf-binary`,
let non-ops users add maps, and keep the gzip-at-upload convention in
one code path that is exercised every time.

## Current State

The read path lives in
[`rust/analytics-web-srv/src/maps.rs`](../rust/analytics-web-srv/src/maps.rs):

- `MapsState { store: Option<Arc<dyn ObjectStore>> }` carried via
  `Extension<MapsState>`.
- `GET /api/maps/catalog` — lists direct-child objects under the
  configured prefix; returns `[{ file, size }]`.
- `GET /api/maps/blob/{filename}` — streams the object, passing through
  `Content-Type` and `Content-Encoding` attributes verbatim.
- Both endpoints sit under cookie auth; catalog is on the compressed
  router branch, blob is on its own branch so the global
  `CompressionLayer` doesn't re-encode GLBs on egress.

Admin page lives at
[`analytics-web-app/src/routes/AdminPage.tsx`](../analytics-web-app/src/routes/AdminPage.tsx)
with cards for Data Sources / Export / Import Screens. The closest CRUD
shape to copy is
[`DataSourcesPage.tsx`](../analytics-web-app/src/routes/DataSourcesPage.tsx)
(list, create dialog, edit, delete confirm, `AuthGuard requireAdmin`).
Frontend catalog caching:
[`maps-catalog.ts`](../analytics-web-app/src/lib/maps-catalog.ts) keeps
a module-level shared promise.

No mutation endpoints exist yet — the read path lands first; this plan
adds the write path on top of the same `MapsState` and the same prefix.

## Design

### Endpoints

All map mutations require `is_admin` and live under the same
`MapsState` extension. They follow the same shape as the data-sources
CRUD: JSON errors with `{ code, message }`, 403 on non-admin, 503 when
storage unconfigured.

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `GET` | `/api/maps/catalog` | user | (existing) list maps |
| `GET` | `/api/maps/blob/{filename}` | user | (existing) stream a map |
| `PUT` | `/api/maps/blob/{filename}` | admin | upload / replace |
| `DELETE` | `/api/maps/blob/{filename}` | admin | delete |

Upload via `PUT /api/maps/blob/{filename}` (raw body, not multipart):

- `Content-Type: model/gltf-binary` expected on the request; rejected
  with 415 otherwise. This is the only safety check beyond size — the
  bucket is reserved for map assets, the prefix is admin-gated, and
  GLB structural validation is out of scope (the renderer logs visible
  console errors for non-conforming GLBs per the authoring contract).
- Body size limit: 256 MiB per route via
  `DefaultBodyLimit::max(256 * 1024 * 1024)`. Default axum limit is
  2 MiB, which would reject every realistic GLB.
- Body is buffered into memory, gzipped with `flate2::write::GzEncoder`
  at default compression, then written via
  `store.put_opts(path, gzipped.into(), PutOptions { attributes, ..})`
  with `Content-Type: model/gltf-binary` and
  `Content-Encoding: gzip` attributes set so the GET handler passes
  them through.
- If `Content-Encoding: gzip` is already on the request (client
  pre-gzipped), skip the server-side compression step and store the
  body verbatim with the same two attributes.
- Returns `200 { file, size }` where `size` is the wire (gzipped) size,
  matching what the catalog reports.

Delete via `DELETE /api/maps/blob/{filename}`:

- 204 on success, regardless of whether the object existed. DELETE is
  idempotent — this matches S3's native semantics (a DELETE on a
  missing key returns 204) and avoids a useless `head()` round-trip
  just to manufacture a 404 that some backends would never surface.
- `is_direct_child` filename validation as in the GET handler.

Rename is intentionally out of scope. The `object_store` crate's
`rename_if_not_exists` falls through to `copy_if_not_exists` + `delete`,
and `AmazonS3::copy_if_not_exists` returns `Error::NotSupported` unless
`aws_copy_if_not_exists` is configured (`multipart`, `header:…`, etc.).
Re-upload under the new name + delete the old one is the documented
workflow until there's a clear need for an atomic move.

### Server-side gzip

`flate2::write::GzEncoder` over a `Vec<u8>` writer at
`Compression::default()` (level 6). GLBs are roughly 30 MiB and
gzip compresses them to ~50–70%. At ~30 MiB the encode is fast (sub-second
on a single core). No streaming compression — the body is already fully
buffered to enforce the size cap and to compute `Content-Length` for the
stored object.

Rejected alternative: re-engineer the GET handler to compress on the
fly so uploads can stay raw. The existing GET specifically bypasses
`CompressionLayer` because every fetch of an uncompressed object would
burn 30 MiB of CPU and drop `Content-Length`; moving that cost to
upload-once is the entire point.

### Body size limit

There is no enforced max on the download side today: the existing
`maps_blob` handler in
[`rust/analytics-web-srv/src/maps.rs`](../rust/analytics-web-srv/src/maps.rs)
streams `store.get(...).into_stream()` straight through `Body::from_stream`
with no size check. Whatever's in the bucket gets served. So whatever
cap is chosen for upload is, in practice, also the practical ceiling
on what shows up on the read side from the UI-uploaded population —
ops drop-ins remain unbounded but that's the existing posture.

For the upload route, a per-route
`DefaultBodyLimit::max(MICROMEGAS_MAPS_MAX_UPLOAD_BYTES)` rejects
oversize bodies before the handler runs. axum's default is 2 MiB,
which rejects every realistic GLB, so an explicit value is required
regardless.

Recommendation: **256 MiB default, configurable via env var**
`MICROMEGAS_MAPS_MAX_UPLOAD_BYTES` (parsed as bytes; default
`268435456`). Reasons:

- Current GLBs are ~30 MiB; 256 MiB leaves ~8× headroom for growth
  without needing a config change.
- Bounds the in-memory buffer the gzip step holds.
- Bounds the worst-case admin foot-gun (single request filling the
  bucket).
- Tunable per deployment without a code change — fleet ops can lower
  it where storage is tight or raise it for richer GLBs.

If the read side ever needs a matching cap (e.g. to bound bandwidth
served to anonymous-ish viewers in a future signed-URL world),
that's a separate change on the GET handler — out of scope here.

### Admin gating

The read endpoints stay open to any authenticated user; mutations
check `is_admin` and return 403 with
`{ code: "FORBIDDEN", message: "Admin access required" }`.

The current `require_admin` helper lives privately in `data_sources.rs`
and returns `Result<(), DataSourceError>`. To share the rule across
modules, lift it into `auth.rs` (the home of `ValidatedUser`) as:

```rust
/// Returned by `require_admin` when the user is not an admin.
/// Implements `IntoResponse` as 403 with a JSON `{ code, message }`
/// body matching the data-sources error shape.
pub struct AdminRequired;

impl IntoResponse for AdminRequired { /* 403 + JSON */ }

pub fn require_admin(user: &ValidatedUser) -> Result<(), AdminRequired> {
    if user.is_admin { Ok(()) } else { Err(AdminRequired) }
}
```

Two call patterns, both supported by this signature:

- Maps handlers return `Result<impl IntoResponse, AdminRequired>` (or
  a wider error enum that has `From<AdminRequired>`) so `?` propagates
  the 403:
  ```rust
  pub async fn maps_upload(...) -> Result<impl IntoResponse, AdminRequired> {
      require_admin(&user)?;
      // …
  }
  ```
  Returning `Response` directly is not compatible with `?`, which
  requires a `Result` return type.
- Handlers with a domain error enum map at the boundary:
  ```rust
  require_admin(&user).map_err(|_| DataSourceError::Forbidden)?;
  ```

`data_sources.rs` is refactored to call the shared helper (its local
`fn require_admin` and the inline `Forbidden` branch are removed; the
remaining map step is `.map_err(|_| DataSourceError::Forbidden)?`).
Maps handlers use the same import.

This is a small, scoped refactor — the auth module gains one
function + one error type; `data_sources.rs` loses one function and
gains one import. No behavior change for existing endpoints.

### Multipart vs raw PUT

Raw PUT. The frontend `File` object is a `Blob`, and `fetch(url, {
method: 'PUT', body: file, headers: { 'Content-Type':
'model/gltf-binary' } })` sends the bytes directly. Multipart would
require the `axum` `multipart` feature and parsing form fields for no
gain — there's only one field and we know its type.

### Frontend

New admin page at `/admin/maps`. Layout copied from
`DataSourcesPage.tsx`: page header, table of maps with name / size /
last-modified / actions, "Upload Map" button opening a file picker,
confirm dialog for delete. `AuthGuard requireAdmin`.

A drag-and-drop drop zone mirrors `ImportScreensPage.tsx` step 1 —
same `Upload` icon, same dashed-border component — placed above the
table. Dropping a `.glb` opens a confirmation if the name collides;
otherwise it uploads immediately.

`Last modified` in the catalog response: the `ObjectMeta` returned by
`list` already has `last_modified: DateTime<Utc>`. The catalog struct
gains a `last_modified` field serialized as an RFC3339 string. The
existing dropdown consumer (`MapCellEditor`) ignores it.

`maps-catalog.ts` gains:

- `invalidateMapCatalog()` — null out the module-level promise.
  Called after upload / delete so the next dropdown render sees the
  new state.
- `uploadMap(file: File, basePath): Promise<void>`
- `deleteMap(filename: string, basePath): Promise<void>`

Both call `authenticatedFetch` (already used by data-sources) so
401 → refresh → retry works automatically.

The `AdminPage` index gets a fourth card linking to `/admin/maps`,
with the `Map` icon from `lucide-react` (already imported in
`MapCell.tsx`).

### Wiring

`build_protected_routes` in `main.rs` already mounts the maps catalog
under the compressed branch. Adding the mutation routes is one
contiguous block:

```rust
.route(
    &format!("{base_path}/api/maps/blob/{{filename}}"),
    put(maps::maps_upload).delete(maps::maps_delete),
)
```

This sits on the compressed (JSON) branch — these are admin JSON
endpoints. The GET blob route stays on its own branch as before.

The 256 MiB body limit is applied as a per-route layer on the upload
route (not globally — query streaming and screen JSON endpoints
should keep the small default).

### Filename policy

`is_direct_child` (already exported from `maps.rs`) is the only check
on the path segment — same posture as the GET handler. The prefix is
reserved for map assets, and `object_store::path::Path` is opaque, so
no `..` traversal is reachable. We don't enforce the `.glb` extension
on writes either; if an admin uploads `readme.txt`, the catalog
already lists it (per `blob_serves_non_glb_extensions_from_prefix` in
`maps_tests.rs`). The renderer ignores non-`.glb` files only because
`formatMapName` and the dropdown happen to expect them — out of scope
for this plan.

## Implementation Steps

### Phase 1 — backend

1. **`rust/analytics-web-srv/src/auth.rs`**
   - Add `pub struct AdminRequired` and its `IntoResponse` impl
     (403, JSON `{ code: "FORBIDDEN", message: "Admin access required" }`).
   - Add `pub fn require_admin(user: &ValidatedUser) -> Result<(), AdminRequired>`.

2. **`rust/analytics-web-srv/src/data_sources.rs`** — refactor to use
   the shared helper; delete the local `fn require_admin` and replace
   call sites with `auth::require_admin(&user).map_err(|_| DataSourceError::Forbidden)?`.

3. **`rust/analytics-web-srv/src/maps.rs`**
   - Add `last_modified: chrono::DateTime<chrono::Utc>` to
     `CatalogEntry`.
   - Add `maps_upload(filename, headers, body)` handler:
     - 415 unless `Content-Type` is `model/gltf-binary`.
     - Read body into `Vec<u8>`.
     - If request `Content-Encoding: gzip` is present, store verbatim;
       else gzip with `flate2::write::GzEncoder`.
     - `put_opts` with attrs `{ ContentType: model/gltf-binary,
       ContentEncoding: gzip }`.
     - Return `200 { file, size }`.
   - Add `maps_delete(filename)` handler:
     - Idempotent: 204 on success, regardless of whether the object
       existed. Treat `Error::NotFound` from `store.delete` as success
       so backends that surface it (`LocalFileSystem`) match S3's
       silent-success behavior.

4. **`rust/analytics-web-srv/Cargo.toml`** — add `flate2` to
   workspace deps if not present, then to this crate's `[dependencies]`.

5. **`rust/analytics-web-srv/src/main.rs`**
   - Register the two new routes.
   - Read `MICROMEGAS_MAPS_MAX_UPLOAD_BYTES` (default
     `256 * 1024 * 1024`) and scope `DefaultBodyLimit::max(N)` to the
     upload handler by chaining it onto the `put(...)` method router
     before `.delete(...)` is added, so the limit applies only to PUT:
     ```rust
     .route(
         &format!("{base_path}/api/maps/blob/{{filename}}"),
         put(maps::maps_upload)
             .layer(DefaultBodyLimit::max(max_upload_bytes))
             .delete(maps::maps_delete),
     )
     ```
     `Router::route_layer` is NOT used here — it would apply the layer
     to every route in `build_protected_routes` (data sources, screens,
     query streaming, …), which contradicts the per-route intent stated
     in the "Wiring" section. Layering on the `MethodRouter` returned by
     `put(...)` before chaining `.delete(...)` scopes the limit to PUT
     only (verified by the canonical pattern in axum 0.8's own tests at
     `axum/src/routing/tests/mod.rs::changing_the_default_limit_differently_on_different_routes`).
     The parsed value is also surfaced on `MapsState` so the handler
     error message can reference the configured cap.
   - Adding a field to `MapsState` is a breaking change for every
     struct-literal construction site. Touch all of them in this step:
     one in `main.rs` itself (the `let maps_state = ...` near
     `connect_maps_store`) and ten in `tests/maps_tests.rs` (every
     `MapsState { store: ... }` literal). The simplest mechanical fix
     is to add a `MapsState::new(store)` constructor that fills in the
     default cap and rewrite the literals; that keeps the test
     surface noise-free and avoids `..Default::default()` boilerplate.

### Phase 2 — backend tests

6. **`rust/analytics-web-srv/tests/maps_tests.rs`** — extend existing
   in-memory tests:
   - `upload_stores_gzipped_with_attributes` — PUT a 1 KiB body,
     read back the stored object's attributes, assert
     `ContentEncoding=gzip` and that the stored bytes decompress to
     the original.
   - `upload_passes_through_client_gzipped_body` — PUT with
     `Content-Encoding: gzip` header; assert stored bytes equal the
     posted bytes (no double-encode).
   - `upload_rejects_wrong_content_type` — `text/plain` → 415.
   - `upload_rejects_oversize_body` — exceed 256 MiB → 413. (May skip
     if synthesizing 256 MiB in a test is too slow; assert the layer is
     applied via a smaller, separate-router test with a 1 KiB limit
     instead.)
   - `upload_requires_admin` — non-admin → 403.
   - `delete_removes_object_and_returns_204`.
   - `delete_is_idempotent_for_missing_object` — DELETE on a key that
     was never put still returns 204 (matches S3 semantics; covers the
     `LocalFileSystem::delete` `NotFound`-suppression branch too).
   - `delete_requires_admin`.
   - All endpoints `_returns_503_when_storage_unconfigured` (parallel
     to the existing 503 tests).
   - Auth-regression guard tests for the new routes (no cookie → 401)
     parallel to the existing two.

### Phase 3 — frontend

7. **`analytics-web-app/src/lib/maps-catalog.ts`**
   - Add `last_modified: string` to `MapCatalogEntry`.
   - Add `invalidateMapCatalog()` (rename `__resetMapCatalogForTest`
     to the public name or keep the test alias as a re-export).
   - Add `uploadMap`, `deleteMap` using `authenticatedFetch`. Each
     calls `invalidateMapCatalog()` on success.

8. **`analytics-web-app/src/routes/MapsPage.tsx` (new)**
   - Pattern: `DataSourcesPage.tsx`. Table columns: Name, Size,
     Last Modified, Actions (delete).
   - "Upload Map" button + dashed drop zone (component extracted from
     `ImportScreensPage.tsx` if reusable, else inlined).
   - Delete `ConfirmDialog`.
   - Empty state mirrors `DataSourcesPage` ("No maps uploaded yet").

9. **`analytics-web-app/src/router.tsx`** — register
   `/admin/maps` → lazy-loaded `MapsPage`.

10. **`analytics-web-app/src/routes/AdminPage.tsx`** — add a fourth
    card linking to `/admin/maps` with the `Map` icon.

### Phase 4 — frontend tests

11. **`analytics-web-app/src/lib/__tests__/maps-catalog.test.ts`** —
    extend the existing 12 tests:
    - `uploadMap` PUTs the file as raw body with the right
      content-type and invalidates the cache on success.
    - `deleteMap` DELETEs the right URL and invalidates on success.
    - Each helper surfaces server errors.

12. **`analytics-web-app/src/routes/__tests__/MapsPage.test.tsx`
    (new)** — mirror the data-sources page tests if any exist; at
    minimum: renders empty state, uploads a file and refreshes the
    list, opens delete confirm.

### Phase 5 — docs

13. **`mkdocs/docs/web-app/notebooks/cell-types.md`** — under the
    Map section, replace the "drop a `.glb` into the configured
    prefix" line with a pointer to the Admin → Maps page (with the
    object-store drop-in still mentioned as the ops-bypass path).

14. **`mkdocs/docs/web-app/admin.md`** (new section if the page
    doesn't already exist) — short Admin page index entry describing
    Maps management.

15. **`analytics-web-app/README.md`** — one-line pointer if the README
    enumerates admin pages.

## Files to Modify

Backend:
- `rust/analytics-web-srv/Cargo.toml` (add `flate2`)
- `rust/Cargo.toml` (workspace dep for `flate2` if missing)
- `rust/analytics-web-srv/src/maps.rs`
- `rust/analytics-web-srv/src/main.rs`
- `rust/analytics-web-srv/tests/maps_tests.rs`

Frontend:
- `analytics-web-app/src/lib/maps-catalog.ts`
- `analytics-web-app/src/lib/__tests__/maps-catalog.test.ts`
- `analytics-web-app/src/routes/MapsPage.tsx` (new)
- `analytics-web-app/src/routes/__tests__/MapsPage.test.tsx` (new)
- `analytics-web-app/src/router.tsx`
- `analytics-web-app/src/routes/AdminPage.tsx`

Docs:
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `analytics-web-app/README.md` (if it enumerates admin pages)

## Trade-offs

**Server-side gzip vs require client-gzip.** The whole point is to
remove the upload-time footgun, so the server must handle plain
uploads. Pass-through for clients that already gzip costs nothing
(check one header) and avoids double-encoding for ops-tooled uploads.

**Buffer-in-memory gzip vs streaming.** 256 MiB cap × in-memory gzip
is bounded and simple; streaming would require `async-compression`
and a streaming `put` API (`object_store::buffered::BufWriter`).
Streaming saves memory for the worst case but complicates the
content-encoding handshake — we'd need to know the gzipped output
size to set `Content-Length` correctly, which a streaming encoder
doesn't give us until it finishes. Buffered is the right shape for
the scale.

**Raw PUT vs multipart.** Raw PUT keeps the dependency surface
unchanged (`axum` multipart feature off, no new parsers). Multipart
would only matter if we needed extra fields (display name override,
tags, etc.) — we don't.

**No rename endpoint.** `object_store`'s `rename_if_not_exists` for
S3 requires `aws_copy_if_not_exists` to be configured (otherwise it
returns `Error::NotSupported`), and a non-atomic copy-then-delete
substitute introduces a TOCTOU window we don't want to design around
for a feature whose user-visible value is small. The re-upload + delete
workflow is one extra click and is honest about what's happening to
the object.

**Dropping previews / thumbnails for now.** The issue marks them
optional. Generating a thumbnail would require running a GLB renderer
server-side (heavy) or in a hidden iframe client-side (complex). Defer
until there's a clear user request — the existing display-name +
size + last-modified gives enough operator context.

**Admin-only mutation vs per-user uploads.** The original storage
plan reserved the prefix for "map assets" without per-tenant ACL. The
issue explicitly puts ACLs out of scope. Admin gating matches the
data-sources page and the rest of the admin shell.

**Validating the GLB authoring contract on upload.** The renderer
already logs visible console errors for non-conforming GLBs (missing
camera, missing `MM_ambient_light`). Server-side validation would
require pulling in a glTF parser and codifying the contract in two
places. Skip.

## Testing Strategy

- Backend in-memory tests cover handler behavior (status codes,
  attributes round-trip, admin gating, auth-regression guard).
- A small handler-level test asserts the gzipped storage contract by
  reading the object back through `store.get` and decompressing in
  the test.
- Frontend unit tests cover the catalog helper changes.
- Manual smoke test against a local `file://` lake: upload a real
  GLB through the page, hit `/api/maps/catalog` directly to confirm
  the entry appears with the right size, then load the map in a Map
  cell.

