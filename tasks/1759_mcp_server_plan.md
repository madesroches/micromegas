# MCP Server for Agent Access Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1759

## Overview

Agents that can't spawn processes (hosted assistants, IDE agents without a shell) can't use the
`micromegas-query` CLI. This adds an MCP server (Streamable HTTP) that exposes guarded query
tools (`prepare_query`, `execute_query`, `list_views`, `describe_view`), screen/notebook
management tools, and delivers the usage
guidance as MCP-native content. The server is a `/mcp` route hosted **inside `analytics-web-srv`**
(and therefore inside the monolith's `web` role). It validates the bearer itself (OIDC or analytics key)
and forwards it to FlightSQL, which stays the enforcement point where identity,
audience scoping, deny list, admission, scan cap and audit apply.

## Current State

- **`http-gateway`** (`rust/public/src/servers/http_gateway.rs`) forwards `Authorization` to
  FlightSQL and returns the whole result as one JSON string with no row/byte cap
  (`handle_query`, ~L250). It builds a new gRPC channel per request. It is an optional deployment.
- **`analytics-web-srv`** serves `/api/query-stream` behind cookie/OIDC auth
  (`web_server.rs:376`, `auth/handlers.rs:472`, `auth/state.rs:70`). The query path resolves a
  FlightSQL URL from `DataSourceCache` and builds a `BearerFlightSQLClientFactory`
  (`stream_query.rs:277-310`). It is already part of the monolith (`web` role).
- **FlightSQL client** (`rust/public/src/client/flightsql_client.rs`): `query_stream` streams
  `FlightRecordBatchStream`; `prepare_statement` only returns the schema. Attribution headers
  `x-client-type/-agent/-session/-notebook/-cell` are read in `flight_sql_service_impl.rs:775`.
- **FlightSQL prepare** (`do_action_create_prepared_statement`, `flight_sql_service_impl.rs:1663`)
  returns the SQL as handle and builds only a logical plan; no estimate, no audit. **#1760**
  (open, dependency) changes this: prepare-time estimate in `FlightInfo.total_bytes`,
  `estimate_precision` in `app_metadata`, begin/end bound into the handle, per-caller scan cap
  enforced in `do_get`, per-query memory limit/timeout.
- **#1758** (plan in `tasks/1758_view_descriptions_plan.md`) appends `description`, `columns`,
  `sort_order` to `list_view_sets()`. These back `list_views` / `describe_view`.
- Mutating table functions are registered in `register_lakehouse_functions`
  (`rust/analytics/src/lakehouse/query.rs:143`, names at ~L237) and a name list already exists
  at `view_definition.rs:281` (`retire_partitions`, `materialize_partitions`, ...).
- No MCP code or `rmcp` dependency exists.

## Design

### Hosting: a module mounted by `analytics-web-srv`

- New module `rust/public/src/servers/mcp/` (feature `server`) holding the tool logic, guardrails
  and rendering, with no dependency on the web app. It exposes
  `mcp::register_routes(router, McpConfig, providers)`, where `providers` is a list of `ToolProvider`s.
  - Open/closed: `http-gateway` or a standalone binary can mount the same module later without
    change.
- `analytics-web-srv` calls it from `web_server.rs` **outside** the protected (cookie) router
  layer, at `{base_path}/mcp`. The monolith gets it through the `web` role.
- The route does **not** use `cookie_auth_middleware`. A bearer middleware validates the token in
  the web server with a `MultiAuthProvider` (OIDC plus analytics keys, wrapped in
  `MembershipProvider` so admin/group membership resolves as it does for the cookie path). The web
  server's existing validator is OIDC-only (`OidcConfig::from_env()` in `auth/state.rs`) and does
  not read `MICROMEGAS_ANALYTICS_*`, so the MCP route builds its own provider with
  `ProviderBuilder` plus a DB key store built with
  `dedicated_key_store_pool(&analytics_keys_pool)` (the existing lazy pool over
  `MICROMEGAS_SQL_CONNECTION_STRING`, `web_server.rs`; `with_db_key_store` expects the dedicated
  pool, not the raw one), and attaches a group store so `compose()` wraps the chain in
  `MembershipProvider`. It returns 401 +
  `WWW-Authenticate` on a missing or invalid token and puts the resulting `AuthContext` in
  request extensions. The validated token is also forwarded to FlightSQL, which re-validates it
  and applies audience scoping, deny list, admission, scan cap and audit. Validating here is
  what gives the screens/notebook tools a real identity.
- FlightSQL target: `MICROMEGAS_MCP_FLIGHTSQL_URL` if set, otherwise the default data source,
  resolved through a new `data_sources.is_default` lookup (`DataSourceCache` only has
  `resolve(name)`). When neither exists the route is not mounted. The MCP route never takes a
  data source from the caller.
- Disabled unless `MICROMEGAS_MCP_ENABLED=true`, so existing deployments don't grow a new
  agent-facing surface by upgrading. When disabled, an explicit 404 handler is registered for
  `/mcp` (the SPA fallback would otherwise return `index.html` with 200). Under `--disable-auth`
  there is no `auth_state`, so the route is not mounted.

### Transport

`rmcp` (official Rust SDK) with its Streamable HTTP service nested in axum, in stateless mode
(set explicitly: `StreamableHttpServerConfig::default()` has `legacy_session_mode: true`, so use
`with_legacy_session_mode(false)` / `NeverSessionManager`; verify the exact API at implementation):
every tool call is self-contained (`execute_query` takes the full query arguments, there is no
handle), so no server-side state is needed. Per-call data (headers, the validated `AuthContext`)
is read from the `http::request::Parts` in the request context extensions.
Cap request body size (reuse `tower-http` `limit` already in the workspace).

`McpConfig` carries a host allowlist and an origin allowlist, set on rmcp's
`StreamableHttpServerConfig` (`allowed_hosts` defaults to localhost only, so a real hostname gets
403; empty `allowed_origins` means no Origin validation, which the MCP Streamable HTTP spec
requires). No CORS is added for `/mcp`; the global `CorsLayer` stays as is.

### Query and discovery tools (all `readOnlyHint: true`)

| Tool | Behavior |
|---|---|
| `prepare_query(sql, begin?, end?, limit?)` | Read-only check, then FlightSQL `GetFlightInfo` with `query_range_begin/end`. Returns the schema, `total_bytes` estimate, `estimate_precision`, within-cap flag. Executes nothing and returns no handle. Over-cap or opaque-without-range results carry the hint to narrow the range / filter on sort columns. Without #1760 the estimate fields are absent/"unknown" and only the MCP row/byte caps apply. |
| `execute_query(sql, begin?, end?, limit?)` | Same arguments as `prepare_query`, so the agent re-sends its (possibly revised) SQL. `limit` defaults to the default row cap and is clamped to the hard row maximum. Runs the read-only check, executes through FlightSQL (which re-plans and enforces the scan cap); streams batches into a bounded renderer; cancels the stream at the row or byte cap. |
| `list_views()` | Names + first line of description, from `list_view_sets()`. |
| `describe_view(name)` | Description, columns, sort order, on-demand flag, from `list_view_sets()` (#1758). |

- **No handle**: prepare and execute are independent calls with the same arguments. FlightSQL
  re-plans and re-checks the cap on every execute, so a handle would add state and an opaque
  value to validate without removing any check. Re-sending costs a second planning pass (the same
  one a stateless FlightSQL handle would cost) and loses an explicit prepare-to-execute id link.
  The read-only check runs on both tools.
- **Rendering** (`mcp/render.rs`): markdown table (CSV for wide results is not needed in v1),
  stops pulling batches once `max_rows` or `max_bytes` is reached, then appends
  `truncated at N rows, add filters or aggregate`. Defaults 1,000 rows / 128 KiB (about 32k tokens), hard maxima 10,000 rows / 1 MiB. Defaults and
  maxima are configurable via env, and the config cannot exceed the compiled-in absolute limits.
  The byte cap is the binding one for wide rows (long `msg` values), so it is checked per row.
- **Errors**: FlightSQL `Status` becomes a tool result with `isError: true` and the message;
  `Unauthenticated` becomes HTTP 401 at the transport level so clients re-auth.
- **No paging in v1**: the agent aggregates or filters (answers the issue's paging question).
  No process convenience tools in v1 either.

### Read-only enforcement (`mcp/read_only.rs`)

Parse with `sqlparser` (via the `datafusion::sql::sqlparser` re-export) and reject, before any FlightSQL call:

- anything that is not a single `Query` statement (DDL, `INSERT`, `SET`, `COPY`, multiple
  statements, `CREATE/DROP MATERIALIZED VIEW`);
- calls to a deny list of mutating table functions and scalar UDFs (`retire_partitions`,
  `materialize_partitions`, `regenerate_partitions`, `deny_queries`, and the admin-only scalar
  UDFs `retire_partition_by_file`, `retire_partition_by_metadata`, `remove_query_denial`), plus
  every other admin-only function registered under `lakehouse_admin` (`list_query_denials`,
  `list_view_set_definitions`, `check_view_merge`, ...): no admin features are exposed through MCP,
  read-only or not. A test iterates the registered admin function names and asserts each is denied.
  `read_only.rs` matches function-call expressions as well as table factors.
  `MUTATING_TABLE_FUNCTIONS` in `view_definition.rs:280` covers table functions only and also holds
  read-only functions banned from stored scans (`view_instance`, `process_spans`,
  `perfetto_trace_chunks`), so split it: a `pub const` of the truly mutating subset (used here,
  with the three scalar UDF names added), and the stored-scan-banned superset built from it. A new
  mutating function is then covered in both.

The same check gates `prepare_query` and `execute_query`.

### Guidance

1. **Server `instructions`** in the initialize result: the short mandatory rule list.
2. **Tool descriptions** repeat the critical rules on `prepare_query`/`execute_query`.
3. **Skills over MCP** (`io.modelcontextprotocol/skills`): serve the built-in `micromegas-query`
   skill by `include_str!` of the existing shipped
   `claude-plugin/skills/micromegas-query/SKILL.md` (single source), with a digest
   manifest computed at startup. `rust/public` (`micromegas`) is published to crates.io and
   `include_str!` outside the crate directory breaks `cargo publish`, so keep the skill files
   inside the crate (e.g. `rust/public/src/servers/mcp/skills/`) and make `claude-plugin/skills/`
   a symlink or build-time copy of them. Extra skills are loaded from `MICROMEGAS_MCP_SKILLS_DIR` (read
   once at startup, files only, no symlink escape). Layers 1 and 2 must stand alone because
   client support for layer 3 is partial.

### Attribution

Per call, set on the FlightSQL client: `x-client-type: mcp`, `x-client-agent` from the request's
`User-Agent` header or `unknown` (stateless mode has no `clientInfo`), a fresh `x-request-id`
(prepare and execute are correlated in the audit by query fingerprint, not by id). The FlightSQL
service also reads `x-client-entrypoint`; it is left unset.
`x-client-ip` comes from the shared `http_utils::get_client_ip`, which honors `X-Forwarded-For` /
`X-Real-IP` before `ConnectInfo`, so it assumes a trusted proxy in front, as FlightSQL does. `build_origin_metadata`
in `http_gateway.rs` is not reusable (it appends `+gateway` to `x-client-type` and echoes a
client `x-request-id`), so `mcp/flight.rs` sets the other attribution headers itself.

### Auth phases

- **Phase 1** (this plan): analytics key or OIDC token as bearer, static header. The web app
  `client_id` is the OIDC audience, so FlightSQL must accept that audience (its OIDC config lists
  it) for OIDC tokens to be forwarded successfully. Query and
  screen read tools work with a long-lived analytics key; the screen write tools need an OIDC
  identity, and a pasted OIDC token expires, so writes are practical mainly once phase 2 lands.
- **Phase 2** (separate plan, after a decision on token passthrough): the web server already
  holds the OIDC configuration, which makes serving
  `/.well-known/oauth-protected-resource` straightforward. The open decision is the audience
  question: FlightSQL accepting an MCP-audience token vs. delegated identity. Nothing in phase 1
  forecloses either.

### Screen and notebook tools

Hosting in the web server lets the MCP route manage the same screens (notebooks, tables, ...) and
folders the UI does, so an agent can find a curated notebook, reuse its queries, or save the
analysis it just built.

| Tool | Hints | Behavior |
|---|---|---|
| `list_screen_types()` | read-only | Types with descriptions; exposes the existing `/api/screen-types` endpoint. |
| `get_screen_type_default(screen_type)` | read-only | Default config for a type, a starting point for `create_screen`; exposes the existing `get_default_config` endpoint. |
| `list_screens(folder_path?)` | read-only | Name, type, folder, `managed_by`, `updated_at`; no config (keeps the list small). The extracted store function adds the folder filter and config omission; the current HTTP handler has neither. |
| `get_screen(name)` | read-only | Full record including the config JSON. Large configs are subject to the byte cap with a truncation notice. |
| `list_folders()` | read-only | Folder tree with counts. |
| `create_folder(path)` | write | Idempotent, as the HTTP route. |
| `create_screen(name, screen_type, config?, folder_path?)` | write | `config` defaults to the type default. |
| `update_screen(name, config?, folder_path?, expected_updated_at?)` | write | `expected_updated_at` is an optional guard: the update is refused if the stored `updated_at` differs, so an agent doesn't silently overwrite a concurrent edit (the HTTP route is last-writer-wins). |
| `delete_screen(name)` | destructive | Deletes one screen. |

Folder rename/delete are left out: they move or remove many screens at once and the UI is the
right place for them.

- **Shared logic, not a second implementation.** Today the SQL, advisory-lock ordering and
  validation live inside axum handlers (`screens.rs`, `folders.rs`). Extract them into plain
  functions taking `(&PgPool, &ValidatedUser, args)` and make both the HTTP handlers and the MCP
  tools call them. The bearer middleware's identity feeds them via
  `ValidatedUser::from(&AuthContext)`. Name, folder
  path and screen-type validation stay in that one place.
- **Tool provider seam.** `mcp/` defines a `ToolProvider` trait (tool list + call, receiving the full `AuthContext` of the caller, since
  `ValidatedUser` drops `auth_type`). The query
  tools are one provider in `rust/public`; the screens provider lives in `analytics-web-srv`
  (it needs the app DB) and is registered when mounting the route. This keeps `mcp/` free of
  web-app types (open/closed) and lets another host mount only the query tools.
- **Authorization.** The web app today lets any authenticated user mutate screens, so the tools
  do the same. Additionally:
  - write tools are only registered when `MICROMEGAS_MCP_SCREEN_WRITES=true` (default off);
  - they require `AuthType::Oidc` on the `AuthContext`. `ValidatedUser::from` also succeeds for an
    analytics key (subject = key name, no email), so the provider must check `auth_type` itself; a
    key gets the read tools only;
  - `managed_by` screens are editable like any other: their source of truth is backed up in git,
    so an edit is recoverable and no MCP-specific refusal is added. The tools leave `managed_by`
    unchanged.
- **Attribution.** `created_by`/`updated_by` get the caller's email as in the UI, falling back to `subject` when
  the email is absent, as the HTTP handlers do. The info log
  line for each mutation also records the MCP client agent, since the row does not.
- **Cell queries go through the guarded path.** A notebook's cell SQL is run by the agent via
  `prepare_query`/`execute_query`, so caps, scan cap and the read-only check apply unchanged.
  There is no "run notebook" tool.
- **Authoring guidance.** The notebook config format is documented for humans in
  `mkdocs/docs/web-app/notebooks/`. A built-in `micromegas-notebooks` skill (condensed from those
  pages, `include_str!` of a file kept next to the query skill) is served over the Skills
  extension, and the write tools' descriptions point to it and to `get_screen_type_default`.

## Implementation Steps

1. **Dependency gate**: land #1760 (and #1758 for the two discovery tools). Steps 2-5 can proceed
   against the existing FlightSQL; `prepare_query` degrades to schema-only until #1760 lands.
2. Add `rmcp` (latest stable at implementation time, streamable-http server feature) to the
   workspace and `rust/public/Cargo.toml` (`server` feature).
3. `mcp/config.rs`: `McpConfig` (enabled, caps with hard maxima, flightsql URL, skills dir, host and origin allowlists).
4. `mcp/read_only.rs` + expose the truly-mutating function list as a `pub const`.
5. `mcp/render.rs`: capped renderer over a `RecordBatch` stream.
6. `mcp/flight.rs`: per-call FlightSQL client (bearer + attribution), prepare/execute helpers
   added to `client/flightsql_client.rs` (prepare returning schema + estimate once #1760 exists).
7. `mcp/provider.rs` (`ToolProvider` trait), `mcp/tools.rs`
   (query tools as one provider), `mcp/server.rs` (`ServerHandler`, instructions, skills resources),
   `mcp/mod.rs::register_routes(router, McpConfig, providers)`.
7a. Extract shared screen/folder store functions from `screens.rs`/`folders.rs`; HTTP handlers call them (existing handler tests must still pass unchanged).
8. Mount in `analytics-web-srv/src/web_server.rs`; build the `MultiAuthProvider` (`ProviderBuilder` + DB key store via `dedicated_key_store_pool(&analytics_keys_pool)`, plus a group store for `MembershipProvider`) and the bearer validation middleware.
8b. `mcp_screens.rs` provider and registration behind `MICROMEGAS_MCP_SCREEN_WRITES`; notebooks skill.
9. Docs, CHANGELOG.

## Files to Modify

- `rust/Cargo.toml`, `rust/public/Cargo.toml` — `rmcp`.
- `rust/public/src/servers/mod.rs` — `pub mod mcp;`
- `rust/public/src/servers/mcp/{mod,config,read_only,render,flight,tools,server,provider,skills}.rs` — new (`provider.rs` holds `ToolProvider`; `skills.rs` the skills resources).
- `rust/analytics-web-srv/src/mcp_auth.rs` — new, the bearer validation middleware.
- `rust/public/src/client/flightsql_client.rs` — prepare returning schema and estimate.
- `rust/analytics/src/lakehouse/view_definition.rs` (+ `query.rs`) — split the function list into a `pub const` truly-mutating subset and the stored-scan-banned superset.
- `rust/analytics-web-srv/src/web_server.rs` — mount `/mcp`.
- `rust/analytics-web-srv/src/{screens,folders}.rs` — extract shared store functions used by the HTTP handlers and the MCP tools.
- `rust/analytics-web-srv/src/mcp_screens.rs` — new, the screens/folders `ToolProvider`.
- `claude-plugin/skills/` (or `rust/public/src/servers/mcp/skills/`) — notebooks authoring skill.
- `mkdocs/docs/admin/` — new `mcp.md`; `mkdocs/mkdocs.yml` nav; `CHANGELOG.md`.

## Trade-offs

- **Web server vs. `http-gateway` vs. separate binary.** The web server is already deployed
  wherever the UI is, is in the monolith, and has the data source config. Cost: agent traffic
  shares a process with the UI, and the web server is not independently scalable for it. The
  module boundary keeps the other two hosts available.
- **Validate in the web app and forward vs. forward only.** Forward-only would avoid building a
  second auth provider in the web server, but screen management needs a validated identity
  there. The cost is token validation on both hops (a cached key/JWKS lookup) and a provider
  configuration to keep consistent with FlightSQL's.

## Decisions

- Screen write tools are opt-in and OIDC-only (analytics keys do not write); `managed_by` screens are editable because they are backed up in git.
- The FlightSQL target is fixed by server config (`MICROMEGAS_MCP_FLIGHTSQL_URL` or the default data source); callers never choose one.
- Folder rename/delete are out of scope.
- No MCP-layer rate limiting in v1; FlightSQL admission and the scan cap apply.
- The MCP route validates tokens itself (user call: screen management needs the identity) and
  still forwards the token so FlightSQL remains the enforcement point for queries.
- The MCP layer rejects mutating SQL and all admin-only functions even for admin callers; it is
  not a security boundary for non-admin callers (FlightSQL still is).
- The `micromegas-notebooks` skill stays in this plan.
- Export of large results is deferred to a follow-up.

## Documentation

- New `mkdocs/docs/admin/mcp.md`: enabling, env vars, client configuration with a static
  `Authorization` header, tools, caps, host/origin allowlists, and how attribution appears in the audit log (`log_entries` with `target = 'flightsql_query_audit'`; `client` is in the JSON `msg`).
- `mkdocs/docs/admin/monolith.md` and `web-app.md`: mention the `/mcp` route and flag; update the `Auth: OIDC (ID tokens via httpOnly cookies)` line in `web-app.md` (~L412) to note the bearer-validated `/mcp` route.
- `mkdocs/docs/admin/authentication.md` and `api-keys.md`: document using OIDC tokens and analytics keys as the MCP bearer (read tools only for keys).
- `CHANGELOG.md`: new feature entry.

## Testing Strategy

No-DB unit tests (the default tier):
- `read_only`: accepts a plain select/CTE/`view_instance` query; rejects DDL, DML, `SET`,
  multi-statement, each mutating function, both table functions and scalar UDFs (iterating the mutating subset so a new entry is covered),
  case/quoting variants, and nested calls inside subqueries.
- `render`: row cap, byte cap, truncation message, empty result, wide/NULL/nested types,
  stops consuming the stream at the cap (use a counting stream).
- `screens` (no-DB: store functions behind a trait seam with canned data): `list_screens` omits config; `create_screen` defaults config from the type; `update_screen` honors `expected_updated_at`; write tools absent when the flag is off or the caller is an analytics key; invalid name/folder/type map to `isError`.
- Existing `screens_tests.rs`/`folders_tests.rs` are `#[ignore]` live-DB tests, so they don't prove the extraction; the new no-DB store-function tests do.
- `config`: hard maxima clamp a larger env value; `limit` defaults to the default row cap and clamps to the hard row maximum.
- Tool layer against a fake FlightSQL seam returning canned `FlightInfo`/streams: over-cap hint,
  opaque-without-range hint, `Status` -> `isError`, `Unauthenticated` -> 401, same arguments accepted by prepare and execute.
  Degraded case (no #1760): estimate fields absent/"unknown", only MCP caps applied.
- Router test (no DB, lazily-connected state): missing bearer -> 401 with `WWW-Authenticate`;
  disabled flag -> explicit 404 handler; disallowed `Host`/`Origin` -> 403; `initialize` returns `instructions` and the skills capability.
- Skills: manifest digest matches content; skills dir rejects path escape.

No live-DB test: nothing here reproduces a witnessed bug, and cap/estimate enforcement belongs to
#1760's tests.

## Manual Verification

1. Start the monolith with `MICROMEGAS_MCP_ENABLED=true`
   (`python3 local_test_env/ai_scripts/start_services.py --monolith`).
2. Connect an MCP client (e.g. `npx @modelcontextprotocol/inspector`) to
   `http://127.0.0.1:3000/mcp` with a bearer header; run `list_views`, `describe_view`, then
   `prepare_query` + `execute_query` on `log_entries`. Expect a table with the truncation line on
   a large result.
3. Query `micromegas-query "SELECT msg FROM log_entries WHERE target = 'flightsql_query_audit' ORDER BY time DESC LIMIT 10"`
   and expect an entry whose JSON `msg` has `client` identifying `mcp`. Not automated: it needs a running service and a real MCP client.

