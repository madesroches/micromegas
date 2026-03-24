# FlightSQL Server Builder Plan

GitHub issue: #955

## Overview

Consumers of the FlightSQL server must manually wire ~50 lines of boilerplate to assemble the server: data lake connection, lakehouse migration, runtime env, partition provider, view factory, lakehouse context, `FlightSqlServiceImpl`, then a tower layer stack of `GrpcHealthService` + `LogUriService` + `AuthService` with `MultiAuthProvider`. This plan adds a `FlightSqlServer` builder in `micromegas::servers` and a `StaticTablesConfigurator::from_env` convenience constructor to eliminate this plumbing.

## Current State

### FlightSQL server binary (`rust/flight-sql-srv/src/flight_sql_srv.rs`)

Lines 37–115 contain the full setup sequence:

1. Read env vars (`MICROMEGAS_SQL_CONNECTION_STRING`, `MICROMEGAS_OBJECT_STORE_URI`)
2. `connect_to_data_lake()` → `Arc<DataLakeConnection>`
3. `migrate_lakehouse(pool)` — run SQL migrations
4. `make_runtime_env()` → `Arc<RuntimeEnv>`
5. `LakehouseContext::new(data_lake, runtime)` → `Arc<LakehouseContext>`
6. `default_view_factory(runtime, data_lake)` → `Arc<ViewFactory>`
7. `LivePartitionProvider::new(pool)` → `Arc<LivePartitionProvider>`
8. Build session configurator (11-line `StaticTablesConfigurator` / `NoOpSessionConfigurator` block)
9. `FlightServiceServer::new(FlightSqlServiceImpl::new(...))` with max decoding size
10. Build auth provider (10-line conditional block)
11. Build tower layer stack (`GrpcHealthService` → `LogUriService` → `AuthService`)
12. TCP listener → `ConnectedIncoming` → `Server::builder().layer().add_service().serve_with_incoming()`

This is the only current gRPC-based FlightSQL consumer. The `analytics-web-srv` and `http-gateway` are Axum-based HTTP servers with different setup patterns — they are not targets for this builder.

### StaticTablesConfigurator (`rust/analytics/src/lakehouse/static_tables_configurator.rs`)

The `new()` constructor takes `(&SessionContext, &str)`. Consumers must write ~15 lines to:
1. Read an env var for the URL
2. Fall back to `NoOpSessionConfigurator` on missing env var
3. Create a temporary `SessionContext` just for discovery
4. Handle errors with fallback to `NoOpSessionConfigurator`

### Servers module (`rust/public/src/servers/mod.rs`)

Contains `flight_sql_service_impl`, `grpc_health_service`, `log_uri_service`, `connect_info_layer`, and other server utilities. The new builder will live here.

## Design

### Part 1: `StaticTablesConfigurator::from_env`

Add a convenience constructor to `StaticTablesConfigurator`:

```rust
impl StaticTablesConfigurator {
    /// Load static tables from the URL in `env_var`.
    /// Returns `NoOpSessionConfigurator` when the variable is unset.
    /// Errors if the variable is set but loading fails (preserves fail-fast behavior).
    pub async fn from_env(
        env_var: &str,
        runtime: Arc<RuntimeEnv>,
    ) -> Result<Arc<dyn SessionConfigurator>> {
        let url = match std::env::var(env_var) {
            Ok(url) => url,
            Err(_) => {
                warn!("{env_var} not set, static tables will not be available");
                return Ok(Arc::new(NoOpSessionConfigurator));
            }
        };
        let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime);
        let configurator = Self::new(&ctx, &url)
            .await
            .with_context(|| format!("loading static tables from {url}"))?;
        Ok(Arc::new(configurator))
    }
}
```

This replaces the 15-line pattern shown in the issue and used in `flight_sql_srv.rs:56–68`.

### Part 2: `FlightSqlServer` builder

Add a new module `rust/public/src/servers/flight_sql_server.rs` with a builder that encapsulates the entire setup from env vars to serving:

```rust
type ViewFactoryFn = Box<dyn FnOnce(Arc<RuntimeEnv>, Arc<DataLakeConnection>) -> BoxFuture<Result<ViewFactory>>>;

pub struct FlightSqlServerBuilder {
    view_factory_fn: Option<ViewFactoryFn>,
    session_configurator: Option<Arc<dyn SessionConfigurator>>,
    auth_provider: Option<Arc<dyn AuthProvider>>,
    use_default_auth: bool,
    max_decoding_message_size: usize,
    listen_addr: SocketAddr,
    static_tables_env_var: Option<String>,
}
```

**Builder methods:**

- `with_view_factory_fn(f: impl FnOnce(Arc<RuntimeEnv>, Arc<DataLakeConnection>) -> ... + 'static)` — optional, overrides the default `default_view_factory()`. The closure receives the runtime and data lake created by the builder, so custom factories can use them. Use when registering custom views.
- `with_session_configurator(cfg: Arc<dyn SessionConfigurator>)` — optional, defaults to `NoOpSessionConfigurator`
- `with_static_tables_env_var(var: &str)` — optional, uses `StaticTablesConfigurator::from_env` during build. Mutually exclusive with `with_session_configurator` (last call wins).
- `with_auth_provider(provider: Arc<dyn AuthProvider>)` — optional, no auth if omitted
- `with_default_auth()` — optional, calls `micromegas::auth::default_provider::provider()` during build. **Errors if no providers are configured** (preserves the current fail-fast behavior when auth is expected but `MICROMEGAS_API_KEYS` / `MICROMEGAS_OIDC_*` env vars are missing).
- `with_max_decoding_message_size(bytes: usize)` — optional, defaults to 100 MB
- `with_listen_addr(addr: SocketAddr)` — optional, defaults to `0.0.0.0:50051`
- `build_and_serve() -> Result<()>` — async, does everything:

**`build_and_serve()` internally runs the full sequence:**
1. Read `MICROMEGAS_SQL_CONNECTION_STRING` and `MICROMEGAS_OBJECT_STORE_URI` env vars
2. `connect_to_data_lake()` → `Arc<DataLakeConnection>`
3. `migrate_lakehouse(pool)`
4. `make_runtime_env()` → `Arc<RuntimeEnv>`
5. `LakehouseContext::new(lake, runtime)` → `Arc<LakehouseContext>`
6. Call provided `view_factory_fn(runtime, lake)` or fall back to `default_view_factory(runtime, lake)` → `Arc<ViewFactory>`
7. `LivePartitionProvider::new(pool)` → `Arc<LivePartitionProvider>`
8. Resolve session configurator (from `static_tables_env_var` or explicit, or no-op)
9. Resolve auth provider (from `with_default_auth` or explicit, or none). If `use_default_auth` is set and `provider()` returns `None`, return error: "Authentication required but no auth providers configured"
10. `FlightServiceServer::new(FlightSqlServiceImpl::new(...))` with max decoding size
11. Tower layer stack: `GrpcHealthService` → `LogUriService` → `AuthService`
12. TCP listener → `ConnectedIncoming` → `Server::builder().layer().add_service().serve_with_incoming()`

**Usage after refactor (`flight_sql_srv.rs`):**

```rust
#[derive(Parser, Debug)]
#[clap(name = "Micromegas FlightSQL server")]
#[clap(about = "Micromegas FlightSQL server", version, author)]
struct Cli {
    #[clap(long)]
    disable_auth: bool,
}

#[micromegas_main(interop_max_level = "info", max_level_override = "debug")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    let mut builder = FlightSqlServer::builder()
        .with_static_tables_env_var("MICROMEGAS_STATIC_TABLES_URL");

    if !args.disable_auth {
        builder = builder.with_default_auth();
    }

    builder.build_and_serve().await?;
    Ok(())
}
```

Down from 115 lines to ~25 lines.

### What stays outside the builder

- **CLI argument parsing** — each binary has its own arg struct
- **Policy decisions** — whether auth is enabled, which static tables env var to use

Callers who need non-standard setup (custom data lake connection, custom view factory, custom runtime env) can still use `FlightSqlServiceImpl` directly — the builder is a convenience layer, not a replacement.

## Implementation Steps

### Phase 1: `StaticTablesConfigurator::from_env`

1. Add `from_env` method to `StaticTablesConfigurator` in `rust/analytics/src/lakehouse/static_tables_configurator.rs`
2. Add necessary imports (`SessionConfig`, `NoOpSessionConfigurator`, `anyhow::Context`)
3. Add unit tests in `rust/analytics/tests/` for:
   - Missing env var returns `Ok(NoOpSessionConfigurator)`
   - Invalid URL returns an error

### Phase 2: `FlightSqlServer` builder

4. Create `rust/public/src/servers/flight_sql_server.rs` with `FlightSqlServerBuilder` and `FlightSqlServer`
5. Register the module in `rust/public/src/servers/mod.rs`
6. Refactor `rust/flight-sql-srv/src/flight_sql_srv.rs` to use the new builder
7. Remove unused direct dependencies (`anyhow`, `tonic`, `tower`) from `rust/flight-sql-srv/Cargo.toml`

### Phase 3: Verify

8. Run `cargo build` from `rust/`
9. Run `cargo test` from `rust/`
10. Run `cargo clippy --workspace -- -D warnings`

## Files to Modify

- `rust/analytics/src/lakehouse/static_tables_configurator.rs` — add `from_env`
- `rust/public/src/servers/mod.rs` — add `flight_sql_server` module
- **New**: `rust/public/src/servers/flight_sql_server.rs` — builder implementation
- `rust/flight-sql-srv/src/flight_sql_srv.rs` — refactor to use builder
- `rust/flight-sql-srv/Cargo.toml` — remove unused direct dependencies (`anyhow`, `tonic`, `tower`)

## Trade-offs

### Builder in `public::servers` vs standalone crate
Putting it in the `public` crate keeps it close to the other server components (`FlightSqlServiceImpl`, `GrpcHealthService`, `LogUriService`) and avoids a new crate. A separate crate would only make sense if the builder had different dependency requirements, which it doesn't.

### All-in-one `build_and_serve()` vs separate `build()` + `serve()`
A single `build_and_serve()` is simpler for the common case. Callers who need to inspect or modify intermediate state (lakehouse context, view factory) should use `FlightSqlServiceImpl` directly — the builder targets the "just start a standard server" path.

### Hardcoded env var names (`MICROMEGAS_SQL_CONNECTION_STRING`, `MICROMEGAS_OBJECT_STORE_URI`)
These are standardized across all micromegas services. Hardcoding them in the builder avoids exposing them as configuration that never actually varies. If a consumer needs different env var names, they bypass the builder entirely.

### `from_env` on `StaticTablesConfigurator` vs standalone function
A method on the type is more discoverable than a free function. It also keeps the fallback logic colocated with the type that implements the trait.

## Testing Strategy

- **`from_env` with missing var**: set no env var, verify it returns `Ok(NoOpSessionConfigurator)`
- **`from_env` with invalid URL**: set env var to garbage, verify it returns an error
- **Builder defaults**: verify `build_and_serve` works with no optional fields set (uses default view factory, no auth, no-op session configurator)
- **`with_default_auth` without providers**: verify `build_and_serve` returns error when `with_default_auth()` is called but no auth env vars are set
- **`with_view_factory_fn`**: verify the closure receives the runtime and lake created by the builder
- **Integration**: refactor `flight-sql-srv` to use the builder, verify it compiles and passes existing tests
- **Manual**: start the refactored server against a local test environment and verify FlightSQL queries still work

## Open Questions

None — all resolved.
