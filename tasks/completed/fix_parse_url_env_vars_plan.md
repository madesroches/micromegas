# Fix object_store::parse_url Ignoring Environment Variables

GitHub issue: [#948](https://github.com/madesroches/micromegas/issues/948)

## Overview

Replace all calls to `object_store::parse_url` with `object_store::parse_url_opts(&url, std::env::vars())` so that standard AWS credential chains (EC2 instance profiles, ECS container credentials, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` env vars) are honored. Currently, `parse_url` uses `AmazonS3Builder::new()` internally, which ignores all environment variables.

## Current State

Two call sites use `parse_url`:

1. **`rust/telemetry/src/blob_storage.rs:28`** — `BlobStorage::connect()`:
   ```rust
   let (blob_store, blob_store_root) =
       object_store::parse_url(&url::Url::parse(object_store_url)?)?;
   ```
   Called from `rust/ingestion/src/remote_data_lake.rs:49` and `rust/ingestion/src/data_lake_connection.rs:30`.

2. **`rust/analytics/src/lakehouse/static_tables_configurator.rs:49`** — `StaticTablesConfigurator::new()`:
   ```rust
   let (object_store, prefix) = object_store::parse_url(&parsed_url)?;
   ```

Both sites use `object_store` 0.12 (workspace dep in `rust/Cargo.toml:57`). The crate exposes `parse_url_opts` which accepts an `IntoIterator<Item = (impl Into<String>, impl Into<String>)>` — passing `std::env::vars()` makes each builder read its recognized keys (e.g., `AWS_*` for S3, `GOOGLE_*` for GCS, `AZURE_*` for Azure) and silently ignore the rest.

## Design

Replace `parse_url(&url)` with `parse_url_opts(&url, std::env::vars())` at both call sites. No new types, no API changes, no new dependencies.

The fix is intentionally minimal — `std::env::vars()` is the correct iterator because:
- Each cloud provider builder only reads the keys it recognizes
- Unknown keys are silently ignored
- This matches the behavior users expect from standard cloud SDKs

## Implementation Steps

1. In `rust/telemetry/src/blob_storage.rs:28`, change:
   ```rust
   object_store::parse_url(&url::Url::parse(object_store_url)?)
   ```
   to:
   ```rust
   object_store::parse_url_opts(&url::Url::parse(object_store_url)?, std::env::vars())
   ```

2. In `rust/analytics/src/lakehouse/static_tables_configurator.rs:49`, change:
   ```rust
   object_store::parse_url(&parsed_url)
   ```
   to:
   ```rust
   object_store::parse_url_opts(&parsed_url, std::env::vars())
   ```

3. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`.

## Files to Modify

- `rust/telemetry/src/blob_storage.rs`
- `rust/analytics/src/lakehouse/static_tables_configurator.rs`

## Trade-offs

**Chosen: `parse_url_opts` with `std::env::vars()`** — One-line change per call site. Cloud-provider-agnostic. No new configuration knobs or abstractions needed.

**Alternative: Construct cloud-specific builders manually** — Would require detecting the URL scheme (s3://, gs://, az://) and calling the appropriate builder's `from_env()`. More code, cloud-specific branching, and fragile to new providers. Rejected.

**Alternative: Accept explicit options map** — Would require plumbing a config parameter through `BlobStorage::connect()` and `StaticTablesConfigurator::new()`. Over-engineered for this fix; the environment is the standard config source for cloud credentials.

## Testing Strategy

1. `cargo build` — verify `parse_url_opts` compiles with the `std::env::vars()` iterator
2. `cargo test` — all existing tests pass (local/file:// URLs are unaffected by env vars)
3. `cargo clippy --workspace -- -D warnings` — no new warnings
4. Manual verification in an AWS environment (eCS/EC2) where `AWS_*` env vars are the only credential source

## Open Questions

None — the fix is straightforward and the issue describes the exact solution.
