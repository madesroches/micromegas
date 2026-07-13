# Per-Service Config Structs & MICROMEGAS_* Dedupe Plan

Tracking issue: [#1248](https://github.com/madesroches/micromegas/issues/1248)

## Overview

`MICROMEGAS_*` environment variables are read ad hoc via `std::env::var` across
~15 files with no unifying config type. The web-role bootstrap (5 env vars +
base-path validation + `WebServerConfig` assembly) is copy-pasted between
`analytics-web-srv` and `monolith`; the data-lake bootstrap (`SQL_CONNECTION_STRING`
+ `OBJECT_STORE_URI`) is duplicated across four sites; the
`parse_url_opts(url, env-vars-lowercased)` idiom appears in three places (plus an
existing fourth in `BlobStorage`); and `shutdown_grace_period_seconds` is
re-declared verbatim in six binaries.

This plan consolidates those inputs into a small set of typed, validated,
startup-time config structs and shared helpers, without introducing a config
framework (figment/config-rs) — plain structs + `from_env()` per the issue's
non-goal. Each service's inputs become visible and validated in one spot, env
access is confined to startup, and the one genuine per-request env read
(`http_gateway`) is eliminated.

## Current State

### Duplication 1 — web-role bootstrap

Copy-pasted nearly verbatim:
- `rust/analytics-web-srv/src/main.rs:36-72` — `read_base_path()` + five `std::env::var` reads + `WebServerConfig` assembly.
- `rust/monolith/src/main.rs:298-339` — same five vars, same inline `must start with '/'` base-path check, same `WebServerConfig` assembly.

The only differences between the two are CLI-derived: `port`, `frontend_dir`,
`disable_auth`, and `admin_var_name` (standalone hard-codes `"MICROMEGAS_ADMINS"`;
the monolith resolves `MICROMEGAS_ANALYTICS_ADMINS` with fallback at
`monolith/src/main.rs:230-234`). The five env-derived fields
(`cors_origin`, `base_path`, `app_db_string`, `maps_uri`, `max_upload_bytes`)
are identical.

`WebServerConfig` itself is defined at `rust/analytics-web-srv/src/web_server.rs:31-53`
and is the runtime config passed to `run_web_server`.

### Duplication 2 — data-lake bootstrap

`SQL_CONNECTION_STRING` + `OBJECT_STORE_URI` read together, then passed to a
`connect_to_*_data_lake`:
- `rust/telemetry-ingestion-srv/src/main.rs:48-52`
- `rust/monolith/src/main.rs:183-190`
- `rust/analytics/src/lakehouse/lakehouse_context.rs:47-53` (`LakehouseContext::from_env`)
- `rust/ingestion/src/web_ingestion_service.rs:118-123` (`WebIngestionService::from_env`)

Both `connect_to_data_lake` (`ingestion/src/data_lake_connection.rs:111`) and
`connect_to_remote_data_lake` (`ingestion/src/remote_data_lake.rs:45`) live in the
`ingestion` crate. `analytics` already depends on `ingestion`.

### Duplication 3 — object-store URL parsing

`object_store::parse_url_opts(&url, std::env::vars().map(lowercase))`:
- `rust/object-cache-srv/src/object_cache_srv.rs:113-117` (then requires empty prefix, wraps `PrefixStore`)
- `rust/analytics/src/lakehouse/static_tables_configurator.rs:72-75` (then registers store + `l1_wrap`)
- `rust/analytics-web-srv/src/maps.rs:95-97` (then wraps `PrefixStore`)

A fourth, already-consolidated copy exists as `BlobStorage::parse_url_opts`
(`rust/telemetry/src/blob_storage.rs:36-42`), returning `(Arc<dyn ObjectStore>, Path)`.
The three call sites above do not use it.

### Duplication 4 — shared CLI args

`shutdown_grace_period_seconds` (`--shutdown-grace-period-seconds`, env
`MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`, default `25`) is declared identically in:
- `flight-sql-srv/src/flight_sql_srv.rs`
- `monolith/src/main.rs:152-158`
- `object-cache-srv/src/cli.rs:65-70`
- `analytics-web-srv/src/main.rs:27-33`
- `telemetry-maintenance-srv/src/main.rs`
- `telemetry-ingestion-srv/src/main.rs:36-42`

All six binary crates depend on `micromegas` (the `public` facade) with the
`server` feature.

Scope note (resolved): the issue names `telemetry-admin-cli` as re-declaring the
grace-period arg. No such crate exists in the workspace — this is a stale name for
the maintenance daemon `telemetry-maintenance-srv`
(`telemetry-maintenance-srv/src/main.rs:24-30`), which is already in the
six-binary set above. No extra target.

### Object-cache-srv inline validation

`object-cache-srv/src/object_cache_srv.rs:38-100` holds ~60 lines of numeric-knob
validation inline in `main` (block_size, fetch counts, memory budget, queue
capacities), plus a delegated `cli::validate_write_tuning`. The `Cli` struct
(`object-cache-srv/src/cli.rs`) is already fully clap+env driven — the gap is that
validation is not grouped with the type.

### Genuine per-request env read (hot path)

`rust/public/src/servers/http_gateway.rs:279` reads `MICROMEGAS_FLIGHTSQL_URL`
**inside the `handle_query` request handler** — re-read and re-parsed on every
gateway request. This is the only `std::env::var("MICROMEGAS_*")` found on a
request/hot path. The `http-gateway` binary
(`rust/http-gateway/src/http_gateway_srv.rs`) already builds `HeaderForwardingConfig`
at startup and layers it as an `Extension`; `flight_url` should be resolved the
same way.

### Reads that are already at startup (no change required for the hot-path criterion)

Verified not on request paths, so they stay as-is (though several are folded into
the new structs below): `web_server.rs:63-70` (OIDC/cookie setup, called once in
`run_web_server`), `oidc.rs:265` `load_admin_users` (called once in the provider
constructor, `oidc.rs:381`), `default_provider.rs` (provider construction),
`runtime.rs:11` and `lakehouse_context.rs:63` (constructed once), the
`flightsql_client_factory` / `uri_handler` / `telemetry-sink` reads (client
construction).

## Design

Three homes, chosen to avoid dependency cycles and to reuse existing consolidation
points:

| Piece | Home | Reachable via facade as |
|-------|------|-------------------------|
| `CommonServerArgs` (clap `Args`) | new `public/src/config.rs` (`server` feature) | `micromegas::config::CommonServerArgs` |
| `DataLakeConfig` + `from_env()` | `ingestion` crate | `micromegas::ingestion::DataLakeConfig` |
| `parse_object_store_url()` free fn | `telemetry` crate (`blob_storage`) | `micromegas::telemetry::blob_storage::parse_object_store_url` |
| `WebServerConfig::from_cli_and_env()` | `analytics-web-srv` crate (existing type) | (crate-local; monolith already depends on it) |

Design principle: the clap `Cli`/`Args` structs (which already use `env = "..."`)
**are** the typed arg-and-env config for each service; this plan adds typed structs
only for the env-only groups that clap does not cover (data lake, web bootstrap)
plus one shared arg group, and moves scattered validation next to its type. This
keeps DRY (no wrapper struct that merely re-holds clap fields) and honors the
"no heavy framework" non-goal.

### `CommonServerArgs` (shared clap arg group)

```rust
// public/src/config.rs  (cfg(feature = "server"))
use std::time::Duration;

/// CLI args shared by every long-running server binary.
/// Flatten into a binary's clap struct with `#[command(flatten)]`.
#[derive(clap::Args, Debug, Clone)]
pub struct CommonServerArgs {
    /// Seconds to wait for in-flight work to complete after SIGTERM.
    #[arg(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    pub shutdown_grace_period_seconds: u64,
}

impl CommonServerArgs {
    pub fn grace(&self) -> Duration {
        Duration::from_secs(self.shutdown_grace_period_seconds)
    }
}
```

Each binary replaces its inline field with:

```rust
#[derive(Parser, Debug)]
struct Cli {
    // ... binary-specific args ...
    #[command(flatten)]
    common: micromegas::config::CommonServerArgs,
}
// usage: let grace = args.common.grace();
```

Requires adding `clap` (workspace dep, already `features = ["derive", "env"]`) to
`public/Cargo.toml` under the `server` feature.

### `DataLakeConfig`

```rust
// ingestion/src/data_lake_config.rs
use anyhow::{Context, Result};

/// The two env vars every lake-backed role needs.
#[derive(Debug, Clone)]
pub struct DataLakeConfig {
    pub sql_connection_string: String,
    pub object_store_uri: String,
}

impl DataLakeConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            sql_connection_string: std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
                .context("reading MICROMEGAS_SQL_CONNECTION_STRING")?,
            object_store_uri: std::env::var("MICROMEGAS_OBJECT_STORE_URI")
                .context("reading MICROMEGAS_OBJECT_STORE_URI")?,
        })
    }
}
```

- `WebIngestionService::from_env` and `LakehouseContext::from_env` build a
  `DataLakeConfig::from_env()?` and pass `&cfg.sql_connection_string` /
  `&cfg.object_store_uri` to the existing `connect_to_*` functions (no change to
  those signatures).
- `telemetry-ingestion-srv` and `monolith` mains use `DataLakeConfig::from_env()?`
  instead of their two inline reads.

### `parse_object_store_url` free helper

```rust
// telemetry/src/blob_storage.rs
use object_store::{ObjectStore, path::Path};
use std::sync::Arc;

/// Parse an object-store URI into the raw store + root prefix, feeding
/// `object_store` the process env vars lowercased (its expected option keys).
/// The single home for the `parse_url_opts(url, env-vars-lowercased)` idiom.
pub fn parse_object_store_url(uri: &str) -> anyhow::Result<(Arc<dyn ObjectStore>, Path)> {
    let (store, prefix) = object_store::parse_url_opts(
        &url::Url::parse(uri)?,
        std::env::vars().map(|(k, v)| (k.to_lowercase(), v)),
    )?;
    Ok((Arc::new(store), prefix))
}
```

- `BlobStorage::parse_url_opts` becomes a thin delegate to this function (keeps its
  existing public signature and doc).
- `maps::connect_maps_store` calls it, then wraps `PrefixStore` (drops its inline
  `parse_url_opts`).
- `StaticTablesConfigurator::new` calls it for the store, keeps its own
  `url::Url::parse` for `register_object_store` (which needs the parsed `Url`), and
  applies `l1_wrap` as today.
- `object_cache_srv` main calls it, keeps the empty-prefix guard and `PrefixStore`
  wrap.

### `WebServerConfig::from_cli_and_env`

Add to `analytics-web-srv/src/web_server.rs`, next to `WebServerConfig`:

```rust
/// CLI-derived inputs the web server needs; the env-derived fields are read
/// by `from_cli_and_env`. A named struct (not positional args) so the two
/// String fields can't be transposed.
pub struct WebCliArgs {
    pub port: u16,
    pub frontend_dir: String,
    pub disable_auth: bool,
    pub admin_var_name: String,
}

impl WebServerConfig {
    /// Read + validate the five web env vars and combine with CLI-derived
    /// inputs. Single source of the base-path `must start with '/'` rule.
    pub fn from_cli_and_env(cli: WebCliArgs) -> anyhow::Result<Self> {
        let cors_origin = std::env::var("MICROMEGAS_WEB_CORS_ORIGIN")
            .context("MICROMEGAS_WEB_CORS_ORIGIN environment variable not set")?;
        let base_path = read_base_path()?;      // moved here from main.rs
        let app_db_string = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
            .context("MICROMEGAS_APP_SQL_CONNECTION_STRING environment variable not set")?;
        let maps_uri = std::env::var("MICROMEGAS_MAPS_OBJECT_STORE_URI").ok();
        let max_upload_bytes = std::env::var("MICROMEGAS_MAPS_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok());
        Ok(Self {
            port: cli.port,
            frontend_dir: cli.frontend_dir,
            base_path,
            cors_origin,
            app_db_string,
            maps_uri,
            max_upload_bytes,
            disable_auth: cli.disable_auth,
            admin_var_name: cli.admin_var_name,
        })
    }
}

fn read_base_path() -> anyhow::Result<String> {
    let raw = std::env::var("MICROMEGAS_BASE_PATH")
        .context("MICROMEGAS_BASE_PATH environment variable not set")?;
    let base_path = raw.trim_end_matches('/').to_string();
    if !base_path.is_empty() && !base_path.starts_with('/') {
        anyhow::bail!("MICROMEGAS_BASE_PATH must start with '/' (e.g., '/', '/micromegas')");
    }
    Ok(base_path)
}
```

Both mains shrink to:

```rust
// analytics-web-srv/src/main.rs
let config = WebServerConfig::from_cli_and_env(WebCliArgs {
    port: args.port,
    frontend_dir: args.frontend_dir,
    disable_auth: args.disable_auth,
    admin_var_name: "MICROMEGAS_ADMINS".to_string(),
})?;
run_web_server(config, wait_for_sigterm(), args.common.grace()).await

// monolith/src/main.rs (web role)
let web_config = WebServerConfig::from_cli_and_env(WebCliArgs {
    port: args.port,
    frontend_dir: args.frontend_dir.clone(),
    disable_auth: args.disable_auth,
    admin_var_name: analytics_admin_var,
})?;
```

The monolith today reads `app_db_string` once and reuses it for both
`seed_local_data_source` and the `WebServerConfig` struct literal; after this
change, seeding reads `web_config.app_db_string` (already validated) instead of
the standalone read that `from_cli_and_env` now subsumes.

### Object-cache-srv: group validation with the type

Move the inline numeric-knob checks from `object_cache_srv.rs:38-100` into an
inherent method on `Cli`:

```rust
// object-cache-srv/src/cli.rs
impl Cli {
    /// Validate all numeric knobs. Fatal-at-startup config errors, kept next to
    /// the type and unit-testable from the integration-test crate (like the
    /// existing `validate_write_tuning`).
    pub fn validate(&self) -> anyhow::Result<()> {
        // block_size, max_concurrent_fetches, demand_reserved vs max,
        // memory_budget_mb (incl. window floor), prefetch queue/worker,
        // then delegate validate_write_tuning(...)
        ...
    }
}
```

`main` calls `args.validate()?` in place of the inline block. The window-floor
check needs `permits_for_bytes(stream_window_bytes(block_size))`; keep that math in
`validate` (it already lives in the `handlers`/`cli` modules of the same crate).
`validate_write_tuning` stays and is called from `validate`.

### Http-gateway: eliminate the per-request env read

```rust
// public/src/servers/http_gateway.rs
pub struct GatewayConfig {
    pub flight_url: http::Uri,
    pub headers: HeaderForwardingConfig,
}

impl GatewayConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let flight_url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
            .context("MICROMEGAS_FLIGHTSQL_URL not configured")?
            .parse::<http::Uri>()
            .context("Invalid MICROMEGAS_FLIGHTSQL_URL")?;
        Ok(Self { flight_url, headers: HeaderForwardingConfig::from_env()? })
    }
}
```

- `http-gateway` main builds `Arc::new(GatewayConfig::from_env()?)` and layers it as
  the `Extension` (replacing the current `HeaderForwardingConfig` extension).
- `handle_query` takes `Extension(config): Extension<Arc<GatewayConfig>>` and uses
  `config.flight_url.clone()` + `config.headers` — no `std::env::var` on the request
  path. Fail-fast on a bad/missing URL now happens once at startup instead of per
  request.

## Implementation Steps

### Phase 1 — shared building blocks
1. Add `parse_object_store_url` to `telemetry/src/blob_storage.rs`; make
   `BlobStorage::parse_url_opts` delegate to it.
2. Add `DataLakeConfig` (`ingestion/src/data_lake_config.rs`); export from
   `ingestion/src/lib.rs`.
3. Add `public/src/config.rs` with `CommonServerArgs`; declare
   `#[cfg(feature = "server")] pub mod config;` in `public/src/lib.rs`; add `clap`
   to `public/Cargo.toml` under the `server` feature.

### Phase 2 — web config dedup
4. Add `WebCliArgs` + `WebServerConfig::from_cli_and_env` + move `read_base_path`
   into `analytics-web-srv/src/web_server.rs`.
5. Rewrite `analytics-web-srv/src/main.rs` to use `from_cli_and_env` and
   `args.common.grace()`.
6. Rewrite the monolith web-role block (`monolith/src/main.rs:296-342`) to use
   `from_cli_and_env`; read `app_db_string` from the built config for seeding.

### Phase 3 — data-lake dedup
7. `WebIngestionService::from_env` and `LakehouseContext::from_env` build via
   `DataLakeConfig::from_env()`.
8. `telemetry-ingestion-srv` and `monolith` mains build via `DataLakeConfig::from_env()`.

### Phase 4 — object-store call sites
9. Route `maps::connect_maps_store`, `StaticTablesConfigurator::new`, and
   `object_cache_srv` main through `parse_object_store_url`.

### Phase 5 — shared CLI args + validation grouping
10. Flatten `CommonServerArgs` into the six binaries; delete their inline
    `shutdown_grace_period_seconds` fields; replace
    `Duration::from_secs(args.shutdown_grace_period_seconds)` with
    `args.common.grace()`.
11. Move object-cache-srv numeric validation into `Cli::validate`; call it from main.

### Phase 6 — hot-path fix
12. Add `GatewayConfig` to `http_gateway.rs`; build it in `http_gateway_srv.rs` (the
    `http-gateway` binary's main); thread it through `handle_query`; remove the
    per-request env read at line 279.

### Phase 7 — verify
13. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
    `python3 build/rust_ci.py`. Smoke-test monolith + split-mode services via
    `local_test_env/ai_scripts/start_services.py`.

## Files to Modify

Create:
- `rust/ingestion/src/data_lake_config.rs`
- `rust/public/src/config.rs`

Modify:
- `rust/telemetry/src/blob_storage.rs`
- `rust/ingestion/src/lib.rs`, `rust/ingestion/src/web_ingestion_service.rs`
- `rust/analytics/src/lakehouse/lakehouse_context.rs`, `.../static_tables_configurator.rs`
- `rust/public/src/lib.rs`, `rust/public/Cargo.toml`, `rust/public/src/servers/http_gateway.rs`
- `rust/http-gateway/src/http_gateway_srv.rs`
- `rust/analytics-web-srv/src/web_server.rs`, `.../main.rs`, `.../maps.rs`
- `rust/monolith/src/main.rs`
- `rust/telemetry-ingestion-srv/src/main.rs`
- `rust/flight-sql-srv/src/flight_sql_srv.rs`
- `rust/telemetry-maintenance-srv/src/main.rs`
- `rust/object-cache-srv/src/cli.rs`, `.../object_cache_srv.rs`

## Trade-offs

- **Reuse `BlobStorage`'s parse over a new helper crate.** The idiom is already
  consolidated once in `telemetry`; extracting a free function there and delegating
  is strictly less code than a new `micromegas-config` crate, and `telemetry` is a
  universal dependency. Rejected: a dedicated config crate (adds a workspace member
  for ~40 lines).
- **Three homes instead of one module.** A single `micromegas::config` module can't
  hold `DataLakeConfig` (needed by `analytics`/`ingestion`, which can't depend on
  `public` — cycle) or the object-store helper (lowest sensible home is
  `telemetry`). Each piece lives at the lowest crate that all its users share; the
  facade re-exports make them discoverable under `micromegas::*`.
- **Clap `Cli` as the config, not a redundant wrapper.** The issue suggests a
  per-binary `Config` struct. Since clap `env = "..."` already yields a typed,
  validated arg+env struct, wrapping it again would duplicate every field. Instead
  we add typed structs only for env-only groups clap doesn't model (data lake, web)
  and move stray validation onto the existing clap struct (`Cli::validate`). This
  meets the intent — "one place to see/validate each service's inputs" — with less
  code.
- **`WebCliArgs` named struct over positional args.** `from_cli_and_env` needs four
  CLI-derived values, two of them `String`; a named struct prevents transposing
  `frontend_dir` and `admin_var_name`.
- **`LakehouseContext::from_env` / `WebIngestionService::from_env` retained.**
  Rather than threading `DataLakeConfig` from every `main` (a larger churn that
  `flight-sql-srv` and `telemetry-maintenance-srv` route through the FlightSQL
  builder), these keep their `from_env` convenience but build it from
  `DataLakeConfig::from_env()`, so the two-var idiom exists once. Env access stays at
  startup, satisfying the hot-path criterion.

## Documentation

- `rust/monolith/src/main.rs` header comment (lines 6-14) lists the env vars it
  reads — keep accurate after the refactor.
- `rust/telemetry-ingestion-srv/src/main.rs` header comment (lines 6-10) — same.
- Rustdoc on the new `DataLakeConfig`, `CommonServerArgs`, `parse_object_store_url`,
  `WebServerConfig::from_cli_and_env`, `GatewayConfig` serves as the "one place to
  see each service's inputs."
- No user-facing docs (mkdocs) change: env var names, defaults, and CLI flags are
  all unchanged — this is an internal refactor.

## Testing Strategy

- **Unit tests** (in each crate's `tests/` folder per project convention):
  - `WebServerConfig::from_cli_and_env`: base-path cases (`""`, `/`, `/micromegas`,
    trailing-slash trim, missing-leading-slash rejection) and required-var errors,
    using a serialized env-guard helper.
  - `DataLakeConfig::from_env`: both vars present, each missing.
  - `Cli::validate` (object-cache): each zero/ordering/window-floor rejection,
    mirroring the current inline guards; extends the existing integration-test crate
    that already exercises `validate_write_tuning`.
  - `parse_object_store_url`: a `file://` URI parses to a store + expected prefix.
- **Regression:** full `python3 build/rust_ci.py` (fmt check, clippy, tests).
- **End-to-end smoke:** start monolith (`--monolith`) and split-mode services via
  `local_test_env/ai_scripts/start_services.py`; confirm each role boots, the web
  app serves with a non-root `MICROMEGAS_BASE_PATH`, and a gateway query still
  reaches FlightSQL (exercises the `GatewayConfig` path).

## Open Questions

1. `http-gateway`: should `MICROMEGAS_FLIGHTSQL_URL` also become a CLI flag
   (`--flightsql-url`, env-backed) for consistency with other binaries, or stay
   env-only? Plan assumes env-only (behavior-preserving); a flag is a small add if
   desired.
