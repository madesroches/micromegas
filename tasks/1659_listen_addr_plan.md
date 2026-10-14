# Uniform Listen Addresses Across Server Binaries Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1659

## Overview

Every server binary sets its listen address its own way today: `flight-sql-srv` (standalone and
the monolith role) can't set it at all, the web server takes only a port, and the rest use three
different flag names, mostly without an env binding. This plan sets one convention for all of
them and puts it in one place: each listener kind gets a shared clap `Args` struct in the
`micromegas` crate, next to `CommonServerArgs`. The standalone binary and the monolith flatten
the same struct, so a listener has the same flag, env var, and default wherever it runs. The
value is always a full `SocketAddr`, defaults stay as they are, and the old flags keep working.

## Current State

| Binary | Arg today | Default | Source |
|---|---|---|---|
| `flight-sql-srv` | none; builder default | `0.0.0.0:50051` | `rust/flight-sql-srv/src/flight_sql_srv.rs:14-24`, `rust/public/src/servers/flight_sql_server.rs:112,193-196` |
| `monolith` flightsql role | none | `0.0.0.0:50051` | `rust/monolith/src/main.rs:317-339` |
| `monolith` seed | hard-coded `http://127.0.0.1:50051` | | `rust/monolith/src/main.rs:425-449` |
| `analytics-web-srv` | `-p/--port` u16, `MICROMEGAS_PORT` | `3000`, bound `0.0.0.0` | `rust/analytics-web-srv/src/main.rs:16-17`, `web_server.rs:40,70,903` |
| `monolith` web role | `--port` u16, `MICROMEGAS_PORT` | `3000` | `rust/monolith/src/main.rs:147-148,378-382` |
| `telemetry-ingestion-srv` | `--listen-endpoint-http`, no env | `127.0.0.1:8081` | `rust/telemetry-ingestion-srv/src/main.rs:35-36` |
| `monolith` ingestion role | `--listen-endpoint-http`, no env | `127.0.0.1:8081` | `rust/monolith/src/main.rs:143-144,309` |
| `http-gateway-srv` | `--listen-endpoint-http`, no env | `0.0.0.0:3000` | `rust/http-gateway/src/http_gateway_srv.rs:20-21` |
| `object-cache-srv` | `--listen`, `MICROMEGAS_OBJECT_CACHE_LISTEN` | `0.0.0.0:8080` | `rust/object-cache-srv/src/cli.rs:23-28` |
| `flight-sql-srv` health | `--health-listen-addr`, no env | off | `rust/flight-sql-srv/src/flight_sql_srv.rs:18-20` |
| `redis-exporter` health | `--health-listen-addr`, `MICROMEGAS_REDIS_EXPORTER_HEALTH_LISTEN_ADDR` | off | `rust/redis-exporter/src/cli.rs:70-73` |

Shared server args already live in `rust/public/src/config.rs` (`CommonServerArgs`, flattened by
six binaries, env `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`). Env naming precedent in newer
binaries puts the service name first (`MICROMEGAS_OBJECT_CACHE_*`, `MICROMEGAS_REDIS_EXPORTER_*`),
and existing role prefixes already exist (`MICROMEGAS_INGESTION_*` for auth,
`MICROMEGAS_WEB_CORS_ORIGIN`, `MICROMEGAS_FLIGHTSQL_URL`).

## Design

### Convention

| Listener | Flag | Env | Default |
|---|---|---|---|
| ingestion | `--ingestion-listen-addr` | `MICROMEGAS_INGESTION_LISTEN_ADDR` | `127.0.0.1:8081` |
| flightsql | `--flightsql-listen-addr` | `MICROMEGAS_FLIGHTSQL_LISTEN_ADDR` | `0.0.0.0:50051` |
| web | `--web-listen-addr` | `MICROMEGAS_WEB_LISTEN_ADDR` | `0.0.0.0:3000` |
| gateway | `--gateway-listen-addr` | `MICROMEGAS_GATEWAY_LISTEN_ADDR` | `0.0.0.0:3000` |
| object cache | `--object-cache-listen-addr` | `MICROMEGAS_OBJECT_CACHE_LISTEN_ADDR` | `0.0.0.0:8080` |
| health sidecar | `--health-listen-addr` (unchanged) | `MICROMEGAS_<SERVICE>_HEALTH_LISTEN_ADDR` | off |

The rule: a main listener is named `--<role>-listen-addr` / `MICROMEGAS_<ROLE>_LISTEN_ADDR`, and
the role names are the monolith's role names (`ingestion`, `flightsql`, `web`). The flag carries
the role prefix even in a single-listener binary, so it reads the same in the standalone binary
and in the monolith. The env names also can't collide when several binaries share one
environment (`start_services.py`, the all-in-one image), which a single generic
`MICROMEGAS_LISTEN_ADDR` would. The health sidecar keeps its flag, since it sits next to a main
listener in the same binary, and gets the service-prefixed env that `redis-exporter` already
uses. For `flight-sql-srv` that's `MICROMEGAS_FLIGHTSQL_HEALTH_LISTEN_ADDR`.

### Shared args (`rust/public/src/config.rs`)

```rust
pub const DEFAULT_INGESTION_LISTEN_ADDR: SocketAddr = /* 127.0.0.1:8081 */;
pub const DEFAULT_FLIGHTSQL_LISTEN_ADDR: SocketAddr = /* 0.0.0.0:50051 */;
pub const DEFAULT_WEB_LISTEN_ADDR: SocketAddr = /* 0.0.0.0:3000 */;

#[derive(clap::Args, Debug, Clone)]
pub struct IngestionListenArgs {
    /// HTTP ingestion listen address
    #[arg(long, env = "MICROMEGAS_INGESTION_LISTEN_ADDR",
          default_value_t = DEFAULT_INGESTION_LISTEN_ADDR,
          alias = "listen-endpoint-http")]
    pub ingestion_listen_addr: SocketAddr,
}

#[derive(clap::Args, Debug, Clone)]
pub struct FlightSqlListenArgs { /* flightsql_listen_addr, same shape, no alias */ }

#[derive(clap::Args, Debug, Clone)]
pub struct WebListenArgs {
    #[arg(long, env = "MICROMEGAS_WEB_LISTEN_ADDR")]
    pub web_listen_addr: Option<SocketAddr>,
    /// Deprecated: use --web-listen-addr
    #[arg(short, long, env = "MICROMEGAS_PORT", hide = true)]
    pub port: Option<u16>,
}
impl WebListenArgs {
    pub fn resolve(&self) -> anyhow::Result<SocketAddr>;
}

/// Address a same-host client dials to reach `listen`: an unspecified IP
/// (0.0.0.0 / ::) maps to the loopback of the same family.
pub fn local_connect_addr(listen: SocketAddr) -> SocketAddr;
```

- `WebListenArgs::resolve`: `--web-listen-addr` wins. If only `--port`/`MICROMEGAS_PORT` is set,
  it returns `0.0.0.0:{port}` and logs a `warn!` naming the replacement. If neither is set, it
  returns `DEFAULT_WEB_LISTEN_ADDR`. If both are set it bails, because silently ignoring one of
  them would hide a misconfiguration. A clap alias can't cover it, since the type changes from
  a port to an address.
- `FlightSqlServer`'s builder default (`flight_sql_server.rs:112`) uses
  `DEFAULT_FLIGHTSQL_LISTEN_ADDR` instead of parsing a string literal, so the default lives in
  one place.
- The gateway and object-cache listeners belong to one binary each, so their arg stays in that
  binary's own `Cli` and follows the same shape there. Keeping them local avoids pushing
  single-binary structs into the facade crate. The convention table and the shared consts are
  what keep them uniform.

### Legacy flags

| Old | Handling |
|---|---|
| `--listen-endpoint-http` (ingestion, monolith, gateway) | clap `alias` on the new arg; works silently |
| `--listen` (object-cache) | clap `alias` |
| `-p/--port`, `MICROMEGAS_PORT` (web) | hidden arg plus `warn!` in `resolve` (above) |
| `MICROMEGAS_OBJECT_CACHE_LISTEN` (object-cache, released in v0.27) | the new arg becomes `Option`; a resolve method falls back to the old env with a `warn!` and bails when both are set, the same shape as `WebListenArgs::resolve`; the legacy value is parsed as `SocketAddr`, and a parse failure bails with context naming `MICROMEGAS_OBJECT_CACHE_LISTEN` |

These follow the v0.31.0 upgrade-shim precedent (a `warn!` naming the replacement), and the
shims are removed in v0.33.0, with a follow-up issue filed. A clap alias can't warn, and a
warning on a renamed flag would need a duplicate hidden arg each, so the renamed flags just keep
working; their deprecation is recorded in docs and CHANGELOG.

### Per-binary wiring

- **`flight-sql-srv`**: flatten `FlightSqlListenArgs` and call
  `builder.with_listen_addr(args.listen.flightsql_listen_addr)`. Add
  `env = "MICROMEGAS_FLIGHTSQL_HEALTH_LISTEN_ADDR"` to `--health-listen-addr`.
- **`monolith`**: replace `listen_endpoint_http` and `port` with flattened
  `IngestionListenArgs`, `FlightSqlListenArgs`, and `WebListenArgs`. The flightsql role calls
  `with_listen_addr`. `seed_local_data_source` takes the flightsql listen addr and seeds
  `format!("http://{}", local_connect_addr(addr))`. `SocketAddr`'s `Display` already brackets
  IPv6 addresses.
- **`analytics-web-srv`**: flatten `WebListenArgs`. `WebCliArgs.port` and
  `WebServerConfig.port: u16` become `listen_addr: SocketAddr`, and `run_web_server` binds it
  directly (`web_server.rs:903`). Both this binary and the monolith call `args.web.resolve()?`
  when building `WebCliArgs`; the monolith does so only inside the `roles.web` branch
  (`main.rs:378-382`), so a monolith running without the web role never hits the "both set" bail.
- **`telemetry-ingestion-srv`**: flatten `IngestionListenArgs`.
- **`http-gateway-srv`**: rename the arg to `gateway_listen_addr` with env, keeping the alias.
- **`object-cache-srv`**: rename `listen` to `object_cache_listen_addr` (now `Option`) with the
  new env and the `--listen` alias. `cli.rs` gets a resolve method taking the legacy env value
  as an `Option<String>` parameter, parses it as `SocketAddr`, and bails with context naming
  `MICROMEGAS_OBJECT_CACHE_LISTEN` if it doesn't parse (following `redis-exporter`'s
  `into_config(self, redis_password: Option<String>)`). `main` reads
  `MICROMEGAS_OBJECT_CACHE_LISTEN` and calls it after `validate()`; the result replaces
  `args.listen` at `object_cache_srv.rs:214-218`. It bails when both are set, matching
  `WebListenArgs::resolve`.

## Implementation Steps

### Phase 1: flight-sql-srv (ships alone if needed)

1. `rust/public/src/config.rs`: add `DEFAULT_FLIGHTSQL_LISTEN_ADDR` and `FlightSqlListenArgs`.
2. `rust/public/src/servers/flight_sql_server.rs`: use the const as the builder default and
   update the `with_listen_addr` doc.
3. `rust/flight-sql-srv/src/flight_sql_srv.rs`: flatten the args, call `with_listen_addr`, and
   add the health env.
4. `mkdocs/docs/admin/flight-sql.md`: replace the "Listen address is fixed" note, and add the
   flag and env rows.

### Phase 2: shared args for the remaining listeners

5. `rust/public/src/config.rs`: add `IngestionListenArgs`, `WebListenArgs` + `resolve`,
   `local_connect_addr`, and the remaining default consts.
6. `rust/telemetry-ingestion-srv/src/main.rs`: flatten `IngestionListenArgs`.
7. `rust/analytics-web-srv/src/main.rs`, `web_server.rs`: flatten `WebListenArgs`, add a `#[cfg(test)]` test calling `Cli::command().debug_assert()`, and replace
   `port` with `listen_addr` in `WebCliArgs` / `WebServerConfig` (built from `args.web.resolve()?`), and update the `WebCliArgs`
   construction in `rust/analytics-web-srv/tests/web_server_config_tests.rs:42-47`.
8. `rust/monolith/src/main.rs`: flatten all three, wire the flightsql listen addr, derive
   the seeded URL from it, and call `args.web.resolve()?` only inside the `roles.web` branch
   (`378-382`). Add a `#[cfg(test)]` test calling `Cli::command().debug_assert()`.
9. `rust/http-gateway/src/http_gateway_srv.rs`, `rust/object-cache-srv/src/cli.rs` (+ its uses
   of `args.listen` in `object_cache_srv.rs`): rename the args and add the env, aliases, and fallback.

### Phase 3: callers and docs

10. Move scripts, Dockerfiles, and compose files to the new flags. The aliases mean nothing
    breaks if one is missed, so this step is about consistency:
    `docker/ingestion.Dockerfile`, `docker/monolith.Dockerfile`,
    `docker/analytics-web.Dockerfile` (drop `--port 3000` from CMD rather than replacing it; the
    default is already `0.0.0.0:3000`, and an env-only `MICROMEGAS_PORT` keeps working with the
    warning), `docker/docker-compose.monolith.yaml`,
    `docker/all-in-one.Dockerfile` (comment),
    `local_test_env/ai_scripts/start_services.py` (take `web_port` from the port of
    `MICROMEGAS_WEB_LISTEN_ADDR` when set, else `MICROMEGAS_PORT`, else 3000, so the CORS origin,
    auth redirect URI, and printed URL stay correct),
    `local_test_env/ai_scripts/start_services_with_oidc.py`, `local_test_env/dev.py`,
    `build/run_ingestion_container.py`,
    `analytics-web-app/start_analytics_web.py` (it passes `--port` from its own port option; leave
    it, since switching to `--web-listen-addr` would conflict with an ambient `MICROMEGAS_PORT`), `analytics-web-app/start_analytics_web_docker.py` (drop `"--port", "3000"` from `cmd_args`, line 285, like `docker/analytics-web.Dockerfile`; leave the script's own argparse `--port` at line 135 and the docker `-p {port}:3000` mapping at line 212).
11. Docs (see Documentation) and the CHANGELOG entry.

## Files to Modify

- `rust/public/src/config.rs`
- `rust/public/src/servers/flight_sql_server.rs`
- `rust/public/tests/listen_args_tests.rs` (new)
- `rust/public/Cargo.toml` (`[[test]] name = "listen_args_tests"`, `required-features = ["server"]`)
- `rust/public/src/lib.rs` (crate doc example)
- `rust/flight-sql-srv/src/flight_sql_srv.rs`
- `rust/monolith/src/main.rs` (incl. `debug_assert` test)
- `rust/analytics-web-srv/src/main.rs` (incl. `debug_assert` test), `rust/analytics-web-srv/src/web_server.rs`, `rust/analytics-web-srv/tests/web_server_config_tests.rs`
- `rust/telemetry-ingestion-srv/src/main.rs`
- `rust/http-gateway/src/http_gateway_srv.rs`
- `rust/object-cache-srv/src/cli.rs`, `rust/object-cache-srv/src/object_cache_srv.rs:214-218`, `rust/object-cache-srv/tests/cli_tests.rs`
- Dockerfiles, compose, and scripts listed in step 10
- `rust/object-cache-srv/README.md`
- Docs listed below, `CHANGELOG.md`, `CLAUDE.md`

## Decisions

- Defaults stay as they are, including ingestion's `127.0.0.1:8081` and the gateway's
  `0.0.0.0:3000`, even though the two differ in bind host. Changing either would break existing
  deployments.
- The monolith seed only runs on an empty `data_sources` table, so changing
  `--flightsql-listen-addr` after the first run leaves the seeded row pointing at the old
  address. Accepted: the seed is a first-run convenience, and the row can be edited in the web
  app.

- The `--port`/`MICROMEGAS_PORT` and `MICROMEGAS_OBJECT_CACHE_LISTEN` warn shims are removed in
  v0.33.0, and the clap aliases stay.

## Documentation

- `mkdocs/docs/admin/flight-sql.md`: new flag and env, drop the fixed-address note, add the
  health env.
- `mkdocs/docs/admin/ingestion.md`, `monolith.md`, `web-app.md`, `object-cache.md`: flag and env
  tables, plus deprecated names.
- `mkdocs/docs/admin/service-lifecycle.md`: add a short "Listen addresses" section with the
  convention table, and link it from each admin page. This page already covers the other
  cross-binary args (shutdown grace, health probes).
- `rust/object-cache-srv/README.md`: replace `--listen` / `MICROMEGAS_OBJECT_CACHE_LISTEN`.
- `rust/public/src/lib.rs`: the crate doc example's `--listen-endpoint-http 127.0.0.1:9000`.
- `mkdocs/docs/gateway/configuration.md`, `gateway/index.md`, `getting-started.md`,
  `development/build.md`, `docker/README.md`, `doc/GETTING_STARTED.md`,
  `analytics-web-app/README.md`, `CLAUDE.md`: update the example commands.
- The blog post (`mkdocs/docs/blog/posts/2026-06-13-one-binary-whole-stack.md`) stays as written,
  since it's historical and its flags still work through the aliases.
- `CHANGELOG.md`: one Unreleased entry. It lists the new flags and env vars and the deprecated
  names, plus a **Minor breaking change** clause: `WebServerConfig.port` / `WebCliArgs.port` are
  replaced by `listen_addr: SocketAddr`, and `object-cache-srv`'s `Cli.listen` is renamed. It also
notes that setting both the new web address and `MICROMEGAS_PORT` now fails at startup.

## Testing Strategy

No-DB unit tests in `rust/public/tests/listen_args_tests.rs`, registered in
`rust/public/Cargo.toml` with a `[[test]] name = "listen_args_tests"
required-features = ["server"]` block, since `config` is gated behind `server`. They parse with `try_parse_from`
on a test `#[derive(Parser)]` wrapper. The default-path parse cases carry the same shell-env
exposure as the existing `rust/object-cache-srv/tests/cli_tests.rs` and
`rust/redis-exporter/tests/cli_tests.rs`, since clap's `env` feature reads env vars inside
`try_parse_from`. The `resolve` cases run on constructed structs, which are env-free:

- Each struct parses its new flag and its alias, and with neither given yields its default.
- `WebListenArgs::resolve`: addr only, port only (`0.0.0.0:{port}`), neither (default), both
  (error).
- `local_connect_addr`: `0.0.0.0:p` → `127.0.0.1:p`, `[::]:p` → `[::1]:p`, a specific IP passes
  through unchanged.
- A `#[cfg(test)]` test in each of `rust/monolith/src/main.rs` and
  `rust/analytics-web-srv/src/main.rs` calling `Cli::command().debug_assert()` (via
  `clap::CommandFactory`) on the real composed `Cli`. This catches a flag, alias, or short-option
  collision introduced in a shared struct, which would otherwise only panic at startup.
- `rust/object-cache-srv/tests/cli_tests.rs` gets
  cases for `--listen` and the new flag. The old-env fallback is tested by passing the env value
  into the resolve function as a parameter, following `redis-exporter`'s `into_config(password)`
  pattern, so the test never touches process env. A resolve case covers new only, old env only,
  neither (default), both (error), and old env unparseable (error).

## Manual Verification

Binding a real socket in the monolith's role tasks is wiring, and breakage there is obvious at
startup:

1. `cargo run --bin micromegas-monolith -- --roles all --flightsql-listen-addr 127.0.0.1:50061 --web-listen-addr 127.0.0.1:3001 --ingestion-listen-addr 127.0.0.1:9000 --frontend-dir ../analytics-web-app/dist --disable-auth`
   against a fresh app DB. Expected: the logs show FlightSQL listening on `127.0.0.1:50061`, and
   the seeded `local` data source URL is `http://127.0.0.1:50061`. A query from the web app on
   port 3001 succeeds.
2. `MICROMEGAS_FLIGHTSQL_LISTEN_ADDR=127.0.0.1:50062 cargo run --bin flight-sql-srv -- --disable-auth`.
   Expected: `Listening on 127.0.0.1:50062`, and `micromegas-query` connects there.
3. `MICROMEGAS_PORT=8000 cargo run --bin analytics-web-srv -- --disable-auth`. Expected: binds
   `0.0.0.0:8000` and logs the deprecation warning.
