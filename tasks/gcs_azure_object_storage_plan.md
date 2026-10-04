# Enable GCS and Azure Object Storage Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1639

## Overview

The docs advertise Google Cloud Storage (`gs://…`) as a lake backend, but the workspace builds
`object_store` with only the `aws` feature, so `object_store::parse_url_opts` rejects every
`gs://` and `az://` URI at startup. Enable the `gcp` and `azure` features so both backends work
through the existing generic `ObjectStore` code path. Then give object-storage configuration one
documentation home that names S3, GCS, Azure and S3-compatible endpoints, and make every other
page point to it.

## Current State

- `rust/Cargo.toml:69` — `object_store = { version = "0.13", features = ["aws"] }` (resolved
  to 0.13.2). All crates (`telemetry`, `analytics`, `ingestion`, `object-cache`,
  `object-cache-srv`, `analytics-web-srv`, `public`) use `object_store.workspace = true`, so
  this one line sets the features for every binary.
- `rust/telemetry/src/blob_storage.rs:20-30` — `parse_object_store_url[_parsed]` is the only
  place that builds a store from a URI. It calls `object_store::parse_url_opts(url, env vars
  lowercased)`. The lake (`BlobStorage::connect*`), the ingestion data-lake connection
  (`rust/ingestion/src/data_lake_connection.rs:341`), the object-cache origin
  (`rust/object-cache-srv/src/object_cache_srv.rs:51`) and the maps store all go through it. No
  code branches on the URL scheme.
- `BlobStorage::put_if_absent` (`blob_storage.rs:94-115`) uses `PutMode::Create` and maps
  `AlreadyExists` to `PutIfAbsent::AlreadyExists` and `NotImplemented` to an error that only
  mentions S3. GCS (`ifGenerationMatch=0`) and Azure (`If-None-Match: *`) both implement
  `PutMode::Create` and return `AlreadyExists` on a collision.
- Deleting a missing key returns `NotFound` on GCS and Azure. S3 returns success. Behavior at
  each delete site:
  - `BlobStorage::delete_batch` (`blob_storage.rs:139`) already treats `NotFound` as success.
  - `maps_delete` (`rust/analytics-web-srv/src/maps.rs:396`) already matches `NotFound`.
  - `rust/analytics/src/lakehouse/write_partition.rs` 835/870/892/998 are best-effort cleanups.
    They log a warning or ignore the error. `delete_if_orphan` (423) runs only for a file that
    was just written, and its caller logs any error as a warning (453). The worst case on
    GCS/Azure is an extra warning in an edge case that is already rare.
  - `BlobStorage::delete` (single key) has no production callers.
- Feature cost: in object_store 0.13.2, `gcp = ["cloud", "rustls-pki-types"]` and
  `azure = ["cloud", "httparse"]`. `aws` already enables `cloud`, and `rustls-pki-types` and
  `httparse` are already in the tree (rustls and hyper). So the dependency graph should gain no
  new crates, and the `cargo deny` license and duplicate checks should be unaffected.
- `rust/datafusion-wasm` is a separate tree that does not use `object_store`, so it is
  unaffected.
- Docs: object-store URIs are described piecemeal. `admin/ingestion.md:28` lists `gs://`.
  `admin/monolith.md:43`, `admin/flight-sql.md:27` and `admin/maintenance.md:17` list only
  `file`/`s3` or nothing. `admin/web-app.md:153-159` has a URI table with S3/GCS.
  `admin/object-cache.md:18,39,60` says "bucket-style origin (`s3://`/`gs://`)" and lists only
  AWS env vars. Overview pages say "S3/GCS": `index.md:36`, `getting-started.md:131`,
  `query-guide/index.md:7`, `query-guide/advanced-features.md:5`, `architecture/index.md:31,113`,
  `architecture/caching.md:4,24`, `when-to-use/saas-vendors.md:55,106`, `README.md:53`,
  `rust/object-cache-srv/README.md:4`.

## Design

### Build change

```toml
# rust/Cargo.toml
object_store = { version = "0.13", features = ["aws", "gcp", "azure"] }
```

No other code changes are needed for the backends to work:

- **Configuration** — `parse_url_opts` receives the lowercased process environment, so the
  standard variables are honored:
  - GCS: `GOOGLE_SERVICE_ACCOUNT` / `GOOGLE_SERVICE_ACCOUNT_KEY` /
    `GOOGLE_APPLICATION_CREDENTIALS`, falling back to the GCE/GKE metadata server.
  - Azure: `AZURE_STORAGE_ACCOUNT_NAME`, `AZURE_STORAGE_ACCOUNT_KEY`, `AZURE_STORAGE_SAS_KEY`,
    `AZURE_CLIENT_ID`/`AZURE_CLIENT_SECRET`/`AZURE_TENANT_ID`, workload identity, or managed
    identity (IMDS).
- **URI schemes** — GCS: `gs://bucket/root`. Azure: `az://container/root` (account from
  `AZURE_STORAGE_ACCOUNT_NAME`), `abfss://container@account.dfs.core.windows.net/root`, or
  `https://account.blob.core.windows.net/container/root`.
- **Object-cache origin** — the "bucket-only, empty prefix" check in `object_cache_srv.rs`
  works for any scheme, since `parse_url_opts` returns the path after the bucket or container
  as the prefix. The default namespace already strips any `scheme://`.

### Error message tweak

The `NotImplemented` message in `put_if_absent` currently assumes S3. Generalize the first
clause ("object store does not support conditional put (PutMode::Create)…") and keep the
`aws_conditional_put=disabled` hint scoped to S3-compatible stores. This is a wording change
only.

### Documentation home

Add `mkdocs/docs/admin/object-storage.md` ("Object Storage"). It becomes the single reference
for `MICROMEGAS_OBJECT_STORE_URI` and the other URI settings
(`MICROMEGAS_OBJECT_CACHE_ORIGIN_URI`, `MICROMEGAS_MAPS_OBJECT_STORE_URI`):

- A backend table (local `file://`, AWS S3 `s3://`, GCS `gs://`, Azure `az://` / `abfss://` /
  `https://…blob.core.windows.net`) with an example URI for each.
- Credentials: point to the standard `AWS_*`, `GOOGLE_*`, `AZURE_STORAGE_*` / `AZURE_*`
  variables, including instance/managed identity. Say that options are read from the
  environment (lowercased) as `object_store` keys, and link to the `object_store` builder docs
  instead of copying every key.
- **S3-compatible stores** (MinIO, Cloudflare R2, Ceph, …): use `s3://` plus `AWS_ENDPOINT`
  (and `AWS_ALLOW_HTTP=true` for plain HTTP). The store must support conditional put
  (`If-None-Match: *`).
- **Requirement**: the lake needs create-only writes (`PutMode::Create`). Say this is why plain
  `http`/WebDAV stores are not supported.
- Required permissions: read, write, delete and list on the configured prefix (generalize the
  IAM note now in `web-app.md:134`).

Every per-service env-var table then points to this page instead of listing schemes inline.
Overview pages change "S3/GCS" to name S3, GCS and Azure.

## Implementation Steps

1. **Cargo** — add `gcp` and `azure` to the `object_store` features in `rust/Cargo.toml`. Run
   `cargo update -p object_store` only if the lockfile needs it (building normally refreshes the
   feature set). Confirm `cargo tree -e features -i object_store` shows `gcp` and `azure`.
2. **Dependency hygiene** — run `cargo deny check` and `cargo machete` (as CI does) and confirm
   no new license or duplicate-version failures.
3. **Error message** — generalize the `NotImplemented` message in
   `rust/telemetry/src/blob_storage.rs` (see Design).
4. **Unit tests** — add parse tests to `rust/telemetry/tests/blob_storage_tests.rs` (see
   Testing Strategy).
5. **Docs** — create `mkdocs/docs/admin/object-storage.md` and add it to the `nav` in
   `mkdocs/mkdocs.yml`, next to the other admin deployment pages, both where `admin/*` pages
   appear (around lines 132 and 163).
6. **Docs, per-service tables** — in `admin/ingestion.md`, `admin/flight-sql.md`,
   `admin/maintenance.md` and `admin/monolith.md`, make the `MICROMEGAS_OBJECT_STORE_URI`
   description link to the new page. In `admin/object-cache.md`, list `s3://`, `gs://` and
   `az://` origins (lines 18 and 39) and replace the AWS-only env-var sentence (line 60) with a
   link. In `admin/web-app.md`, add an Azure row to the URI table (153-159) and link the IAM
   paragraph (134) to the permissions section of the new page.
7. **Docs, overview wording** — replace "S3/GCS" with "S3, GCS, Azure" (or "S3/GCS/Azure"
   inside diagram labels) in `index.md`, `getting-started.md`, `query-guide/index.md`,
   `query-guide/advanced-features.md`, `architecture/index.md`, `architecture/caching.md`,
   `when-to-use/saas-vendors.md`, `README.md` and `rust/object-cache-srv/README.md`.

## Files to Modify

- `rust/Cargo.toml` (+ `rust/Cargo.lock` if it changes)
- `rust/telemetry/src/blob_storage.rs`
- `rust/telemetry/tests/blob_storage_tests.rs`
- `mkdocs/docs/admin/object-storage.md` (new)
- `mkdocs/mkdocs.yml`
- `mkdocs/docs/admin/{ingestion,flight-sql,maintenance,monolith,object-cache,web-app}.md`
- `mkdocs/docs/{index,getting-started}.md`, `mkdocs/docs/query-guide/{index,advanced-features}.md`,
  `mkdocs/docs/architecture/{index,caching}.md`, `mkdocs/docs/when-to-use/saas-vendors.md`
- `README.md`, `rust/object-cache-srv/README.md`

## Trade-offs

- **Unconditional features vs. Cargo feature flags on our crates** (e.g. `micromegas/gcp`).
  Unconditional is simpler, adds no new crates and no measurable binary size (the `cloud` stack
  is already linked for `aws`), and one Docker image then serves every cloud. Opt-in flags would
  thread through `public`, `telemetry` and every server crate and push the backend choice to
  build time.
- **Changing single-key delete sites to tolerate `NotFound`** — not done. Every remaining site
  is best-effort and already logs or ignores errors. Wrapping them would add code for a cosmetic
  warning.
- **One docs page vs. editing each service page in place** — one page avoids repeating the
  scheme list and credential guidance in seven places (DRY), and gives S3-compatible stores and
  the conditional-put requirement a natural home.

## Documentation

See Implementation Steps 5–7. `CHANGELOG.md` is updated by the PR step. This is not a SQL or
Rust API change, only a build-feature addition.

## Testing Strategy

Unit tests in `rust/telemetry/tests/blob_storage_tests.rs`, next to
`parse_object_store_url_file_scheme`. They exercise our `parse_object_store_url` and guard
against the features being dropped again (the original bug: the docs promised `gs://`, the
build rejected it):

- `parse_object_store_url_gcs_scheme` — `gs://bucket/lake/root` parses and returns the prefix
  `lake/root`.
- `parse_object_store_url_azure_scheme` —
  `https://account.blob.core.windows.net/container/lake/root` parses and returns the prefix
  `lake/root`. Use the form that embeds the account so the test does not depend on
  `AZURE_STORAGE_ACCOUNT_NAME`. Neither builder makes network calls at build time (credential
  providers resolve lazily). GCS reads application default credentials from the well-known path
  only if that file exists.

The conditional-put and `NotFound` behavior belongs to the backends themselves and needs real
GCS/Azure endpoints, so it is covered by manual verification. A failure there is loud, not
silent: `put_if_absent` returns an error and ingestion rejects the block.

## Manual Verification

Not automated because it needs cloud accounts and credentials that CI does not have. Emulators
(fake-gcs-server, Azurite) are an acceptable substitute if a real bucket is unavailable, as long
as they implement conditional create.

1. **GCS** — create a bucket and export `GOOGLE_APPLICATION_CREDENTIALS` (or
   `GOOGLE_SERVICE_ACCOUNT`). Start services with
   `MICROMEGAS_OBJECT_STORE_URI=gs://<bucket>/mm-test`, run an instrumented sample (or the
   Python ingestion test sender), and then run
   `micromegas-query "SELECT count(*) FROM log_entries" --begin 1h`. Expected: rows returned,
   and objects present under `mm-test/blobs/` and `mm-test/views/`.
2. **GCS duplicate block** — resend a block with the same id. Expected: the ingestion log shows
   the `AlreadyExists` path (the block is accepted as a duplicate) and no error.
3. **Azure** — repeat steps 1–2 with `AZURE_STORAGE_ACCOUNT_NAME` / `AZURE_STORAGE_ACCOUNT_KEY`
   and `MICROMEGAS_OBJECT_STORE_URI=az://<container>/mm-test`.
4. **Retention** — run the maintenance daemon's expiry or `delete_batch` against each store.
   Expected: no `NotFound` errors surface.
