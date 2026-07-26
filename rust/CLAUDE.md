# Rust (rust/)

## Code Style
- Dependencies in alphabetical order in Cargo.toml files
- Use `expect()` with descriptive messages instead of `unwrap()`
- Run `cargo fmt` before any commit
- Use inline format arguments: `format!("value: {variable}")`
- Import proc macros through parent crate: `micromegas_tracing::prelude::*`
- Always use `prelude::*` when importing from prelude modules
- Unit tests should not be with the lib implementation, unit tests should be under the tests folder of the crate
- Workspace dependencies should be added to the root Cargo.toml

### anyhow vs thiserror
`anyhow` is the default for error propagation/reporting — use it unless the caller needs to branch on the
error kind. Reach for `thiserror` (a typed error enum) only when a caller must match on which variant
occurred to change behavior; the branching need justifies the typed error, not the location in the stack.
The canonical example is retryability: where a path retries, model the retryable/terminal distinction as
an explicit type (see `object-cache/src/range_cache/error.rs`, `otel-ingestion/src/error.rs`,
`telemetry-sink/src/http_event_sink.rs`) rather than downcasting an `anyhow::Error` or matching on its
message string. Don't convert the `anyhow` majority to `thiserror` — only the specific spots where callers
branch on error kind.

## Essential Commands (from `rust/` directory)
- **Build**: `cargo build`
- **Test**: `cargo test` (use `-- --nocapture` to see println! output)
- **Format**: `cargo fmt` (REQUIRED before commit)
- **Lint**: `cargo clippy --workspace -- -D warnings`
- **CI**: `python3 ../build/rust_ci.py`

## Environment Variables (for services)
- `MICROMEGAS_SQL_CONNECTION_STRING`: PostgreSQL connection
- `MICROMEGAS_OBJECT_STORE_URI`: S3/GCS bucket URI for payload storage
