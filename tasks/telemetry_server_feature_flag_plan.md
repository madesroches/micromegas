# Gate Server-Only Deps Behind Feature Flags

GitHub issue: #855

## Overview

`micromegas-telemetry` unconditionally depends on `sqlx` and `object_store`, which get pulled into lightweight consumers like `micromegas-telemetry-sink`. The umbrella `micromegas` crate unconditionally depends on the entire server stack (analytics, ingestion, datafusion, axum, tonic, etc.). This plan adds a `server` feature flag at both levels so that client-side consumers — whether using individual crates or the umbrella — can avoid compiling the server dependency tree.

## Current State

### `micromegas-telemetry` modules (`rust/telemetry/src/lib.rs`)

| Module | Heavy deps | Used by telemetry-sink |
|--------|-----------|----------------------|
| `blob_storage` | `object_store`, `futures`, `url`, `bytes` | No |
| `property` | `sqlx` | No |
| `block_wire_format` | None | Yes |
| `compression` | `lz4` only | Yes |
| `stream_info` | None | Yes |
| `wire_format` | `ciborium` only | Yes |
| `types/block` | None | No (but pure) |

### `micromegas` (public) crate (`rust/public/`)

Re-exports everything unconditionally:
- **Always needed for client use**: `tracing`, `telemetry` (wire format), `telemetry_sink`, proc-macros, `chrono`, `uuid`
- **Server-only modules**: `servers/` (ingestion routes, FlightSQL impl, maintenance daemon, perfetto, http_gateway), `client/` (FlightSQL client — uses analytics + arrow-flight + tonic), `utils/` (uses analytics + datafusion)
- **Server-only re-exports**: `arrow_flight`, `axum`, `datafusion`, `object_store`, `prost`, `sqlx`, `tonic`
- **Server-only crate deps**: `micromegas-analytics`, `micromegas-ingestion`, `micromegas-auth`, `micromegas-perfetto`

## Design

### Layer 1: `micromegas-telemetry` feature flag

Add a `server` feature that gates `blob_storage` and `property` modules along with their dependencies (`sqlx`, `object_store`, `futures`, `url`, `bytes`).

**`rust/telemetry/Cargo.toml`**:
```toml
[features]
default = []
server = ["dep:bytes", "dep:futures", "dep:object_store", "dep:sqlx", "dep:url"]

[dependencies]
# Always required
anyhow.workspace = true
chrono.workspace = true
ciborium.workspace = true
lz4.workspace = true
micromegas-transit.workspace = true
serde.workspace = true
uuid.workspace = true

# Server-only
bytes = { workspace = true, optional = true }
futures = { workspace = true, optional = true }
object_store = { workspace = true, optional = true }
sqlx = { workspace = true, optional = true }
url = { workspace = true, optional = true }
```

**`rust/telemetry/src/lib.rs`**:
```rust
#[cfg(feature = "server")]
pub mod blob_storage;
pub mod block_wire_format;
pub mod compression;
#[cfg(feature = "server")]
pub mod property;
pub mod stream_info;
pub mod types;
pub mod wire_format;
```

### Layer 2: `micromegas` (public) crate feature flag

Add a `server` feature that gates the heavy crate dependencies, server/client/utils modules, and server-specific re-exports.

**`rust/public/Cargo.toml`**:
```toml
[features]
default = []
server = [
    "dep:anyhow",
    "dep:arrow-flight",
    "dep:async-stream",
    "dep:async-trait",
    "dep:axum",
    "dep:bytes",
    "dep:datafusion",
    "dep:futures",
    "dep:http",
    "dep:micromegas-analytics",
    "dep:micromegas-auth",
    "dep:micromegas-ingestion",
    "dep:micromegas-perfetto",
    "dep:object_store",
    "dep:once_cell",
    "dep:prost",
    "dep:serde_json",
    "dep:sqlx",
    "dep:thiserror",
    "dep:tokio",
    "dep:tonic",
    "dep:tower",
    "micromegas-telemetry/server",
]

[dependencies]
# Always available
chrono.workspace = true
micromegas-proc-macros.workspace = true
micromegas-telemetry.workspace = true
micromegas-telemetry-sink.workspace = true
micromegas-tracing = { workspace = true, features = ["tokio"] }
serde.workspace = true
uuid.workspace = true

# Server-only (optional)
anyhow = { workspace = true, optional = true }
arrow-flight = { workspace = true, optional = true }
async-stream = { workspace = true, optional = true }
async-trait = { workspace = true, optional = true }
axum = { workspace = true, optional = true }
bytes = { workspace = true, optional = true }
datafusion = { workspace = true, optional = true }
futures = { workspace = true, optional = true }
http = { workspace = true, optional = true }
micromegas-analytics = { workspace = true, optional = true }
micromegas-auth = { workspace = true, optional = true }
micromegas-ingestion = { workspace = true, optional = true }
micromegas-perfetto = { workspace = true, optional = true }
object_store = { workspace = true, optional = true }
once_cell = { workspace = true, optional = true }
prost = { workspace = true, optional = true }
serde_json = { workspace = true, optional = true }
sqlx = { workspace = true, optional = true }
thiserror = { workspace = true, optional = true }
tokio = { workspace = true, optional = true }
tonic = { workspace = true, optional = true }
tower = { workspace = true, optional = true }
```

**`rust/public/src/lib.rs`** — gate server-only modules and re-exports:
```rust
/// re-exports (always available)
pub use chrono;
pub use uuid;

/// re-exports (server-only)
#[cfg(feature = "server")]
pub use arrow_flight;
#[cfg(feature = "server")]
pub use axum;
#[cfg(feature = "server")]
pub use datafusion;
#[cfg(feature = "server")]
pub use object_store;
#[cfg(feature = "server")]
pub use prost;
#[cfg(feature = "server")]
pub use sqlx;
#[cfg(feature = "server")]
pub use tonic;

/// telemetry protocol
pub mod telemetry {
    pub use micromegas_telemetry::*;
}

/// publication of the recorded events using http
pub mod telemetry_sink {
    pub use micromegas_telemetry_sink::*;
}

/// low level tracing - has minimal dependencies
pub mod tracing {
    pub use micromegas_tracing::*;
}

// Re-export proc macros at the top level for easy access
pub use micromegas_proc_macros::*;

/// authentication providers
#[cfg(feature = "server")]
pub mod auth {
    pub use micromegas_auth::*;
}

/// records telemetry in data lake
#[cfg(feature = "server")]
pub mod ingestion {
    pub use micromegas_ingestion::*;
}

/// makes the telemetry data lake accessible and useful
#[cfg(feature = "server")]
pub mod analytics {
    pub use micromegas_analytics::*;
}

/// perfetto protobufs
#[cfg(feature = "server")]
pub mod perfetto {
    pub use micromegas_perfetto::*;
}

#[cfg(feature = "server")]
pub mod servers;

#[cfg(feature = "server")]
pub mod client;

#[cfg(feature = "server")]
pub mod utils;
```

### Consumer updates

Crates that need the server modules enable the feature:

**`rust/analytics/Cargo.toml`**:
```toml
micromegas-telemetry = { workspace = true, features = ["server"] }
```

**`rust/ingestion/Cargo.toml`**:
```toml
micromegas-telemetry = { workspace = true, features = ["server"] }
```

**Server binaries** (`telemetry-ingestion-srv`, `flight-sql-srv`, `telemetry-admin-cli`, `analytics-web-srv`, `http-gateway`, `uri-handler`) that depend on the `micromegas` umbrella crate:
```toml
micromegas = { workspace = true, features = ["server"] }
```

**`rust/examples/write-perfetto/Cargo.toml`** — also needs the server feature (uses `client` module, `datafusion`, `tonic`):
```toml
micromegas = { workspace = true, features = ["server"] }
```

**`rust/telemetry-sink/Cargo.toml`** — no change needed.

### Workspace root (`rust/Cargo.toml`)

No changes to workspace dependency declarations. Features are specified at the consumer site.

## Implementation Steps

### Phase 1: `micromegas-telemetry` feature flag

1. Modify `rust/telemetry/Cargo.toml` — add `[features]` section, mark 5 deps as optional
2. Modify `rust/telemetry/src/lib.rs` — add `#[cfg(feature = "server")]` to `blob_storage` and `property`
3. Update `rust/analytics/Cargo.toml` — add `features = ["server"]`
4. Update `rust/ingestion/Cargo.toml` — add `features = ["server"]`

### Phase 2: `micromegas` (public) crate feature flag

5. Modify `rust/public/Cargo.toml` — add `[features]` section, mark server deps as optional
6. Modify `rust/public/src/lib.rs` — gate server modules and re-exports with `#[cfg(feature = "server")]`
7. Update all server binary Cargo.toml files that depend on `micromegas` — add `features = ["server"]`

### Phase 3: Verify

8. `cargo build` (full workspace)
9. `cargo build -p micromegas-telemetry` (without server feature)
10. `cargo build -p micromegas --no-default-features` (lightweight umbrella)
11. `cargo build -p micromegas-telemetry-sink` (confirm no sqlx/object_store)
12. `cargo test`
13. `cargo clippy --workspace -- -D warnings`
14. `cargo tree -p micromegas-telemetry-sink` to verify clean dep tree

## Files to Modify

- `rust/telemetry/Cargo.toml`
- `rust/telemetry/src/lib.rs`
- `rust/analytics/Cargo.toml`
- `rust/ingestion/Cargo.toml`
- `rust/public/Cargo.toml`
- `rust/public/src/lib.rs`
- `rust/telemetry-ingestion-srv/Cargo.toml`
- `rust/flight-sql-srv/Cargo.toml`
- `rust/telemetry-admin-cli/Cargo.toml`
- `rust/analytics-web-srv/Cargo.toml`
- `rust/http-gateway/Cargo.toml`
- `rust/uri-handler/Cargo.toml`
- `rust/examples/write-perfetto/Cargo.toml`

## Trade-offs

**Chosen approach: Feature flags on existing crates at two levels**
- No new crates, no file moves, no broken import paths
- Both individual-crate consumers (`micromegas-telemetry-sink`) and umbrella-crate consumers (`micromegas`) benefit
- Backward compatible for published crate users who can add `features = ["server"]`

**Alternative: Feature flag on `micromegas-telemetry` only**
- Would fix the telemetry-sink case but leave the umbrella crate pulling in everything
- Someone using `micromegas` for just tracing + sink would still compile the entire server stack

**Alternative: Split into separate crates**
- More crates to maintain, breaks existing import paths
- Overkill when feature flags achieve the same result

## Testing Strategy

1. Build `micromegas-telemetry` without features — confirm it compiles without sqlx/object_store
2. Build `micromegas` without features — confirm it compiles with only tracing/telemetry/sink
3. Build `micromegas-telemetry-sink` — confirm clean dep tree via `cargo tree`
4. Build full workspace — confirm all server crates still compile with `server` feature
5. `cargo test` — all existing tests pass
6. `cargo clippy --workspace -- -D warnings` — no new warnings

## Resolved Questions

1. **`bytes` in public crate**: Only used in server-only modules (`servers/ingestion.rs`, `servers/axum_utils.rs`, `servers/perfetto/perfetto_server.rs`). Made optional and gated behind `server` feature.
2. **`anyhow` in public crate**: Only used in server-only modules (`client/`, `servers/`, `utils/`). Made optional and gated behind `server` feature.
3. **Server binary crates**: All crates depending on the `micromegas` umbrella: `telemetry-ingestion-srv`, `flight-sql-srv`, `telemetry-admin-cli`, `analytics-web-srv`, `http-gateway`, `uri-handler`, `examples/write-perfetto`.
