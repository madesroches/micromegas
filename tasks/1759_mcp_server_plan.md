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
  `mcp::register_routes(router, McpConfig)`.
  - Open/closed: `http-gateway` or a standalone binary can mount the same module later without
    change.
- `analytics-web-srv` calls it from `web_server.rs` **outside** the protected (cookie) router
  layer, at `{base_path}/mcp`. The monolith gets it through the `web` role.
- The route does **not** use `cookie_auth_middleware`. A bearer middleware validates the token in
  the web server with a `MultiAuthProvider` (OIDC plus analytics keys, built with
  `ProviderBuilder` as the monolith already does for FlightSQL, wrapped in `MembershipProvider`
  so admin/group membership resolves as it does for the cookie path). It returns 401 +
  `WWW-Authenticate` on a missing or invalid token and puts the resulting `AuthContext` in
  request extensions. The validated token is also forwarded to FlightSQL, which re-validates it
  and applies audience scoping, deny list, admission, scan cap and audit. Validating here is
  what gives the screens/notebook tools a real identity.
- FlightSQL target: the web server's default data source URL (`DataSourceCache`), overridable
  with `MICROMEGAS_MCP_FLIGHTSQL_URL`. The MCP route never takes a data source from the caller.
- Disabled unless `MICROMEGAS_MCP_ENABLED=true`, so existing deployments don't grow a new
  agent-facing surface by upgrading.

### Transport

`rmcp` (official Rust SDK) with its Streamable HTTP service nested in axum. Stateless-friendly:
every tool call is self-contained (`execute_query` takes the full query arguments, there is no
handle), so no server-side state is needed beyond what `rmcp` keeps for `initialize`.
Cap request body size (reuse `tower-http` `limit` already in the workspace).

### Query and discovery tools (all `readOnlyHint: true`)

| Tool | Behavior |
|---|---|
| `prepare_query(sql, begin?, end?, limit?)` | Read-only check, then FlightSQL `GetFlightInfo` with `query_range_begin/end`. Returns the schema, `total_bytes` estimate, `estimate_precision`, within-cap flag. Executes nothing and returns no handle. Over-cap or opaque-without-range results carry the hint to narrow the range / filter on sort columns. |
| `execute_query(sql, begin?, end?, limit?)` | Same arguments as `prepare_query`, so the agent re-sends its (possibly revised) SQL. `limit` defaults to the default row cap and is clamped to the hard row maximum. Runs the read-only check, executes through FlightSQL (which re-plans and enforces the scan cap); streams batches into a bounded renderer; cancels the stream at the row or byte cap. |
| `list_views()` | Names + first line of description, from `list_view_sets()`. |
| `describe_view(name)` | Description, columns, sort order, on-demand flag, from `list_view_sets()` (#1758). |

- **No handle**: prepare and execute are independent calls with the same arguments. FlightSQL
  re-plans and re-checks the cap on every execute, so a handle would add state and an opaque
  value to validate without removing any check. The read-only check runs on both tools.
- **Rendering** (`mcp/render.rs`): markdown table (CSV for wide results is not needed in v1),
  stops pulling batches once `max_rows` or `max_bytes` is reached, then appends
  `truncated at N rows, add filters or aggregate`. Defaults 1,000 rows / 128 KiB (about 32k tokens), hard maxima 10,000 rows / 1 MiB. Defaults and
  maxima are configurable via env, and the config cannot exceed the compiled-in absolute limits.
  The byte cap is the binding one for wide rows (long `msg` values), so it is checked per row.
- **Large results via export**: `execute_query` returns the capped inline preview plus an MCP
  `resource_link` to `POST {base_path}/mcp/export` (JSON body: the same query arguments plus
  `format=csv|parquet`; the link carries a body template the client fills in). That route
  runs the read-only check, then executes through FlightSQL with the caller's `Authorization` header (no secret in
  the URL) and streams batches to the HTTP body without buffering. FlightSQL still enforces the
  scan cap and audits it as `x-client-type: mcp`. Limits: `MICROMEGAS_MCP_EXPORT_MAX_BYTES`
  (default 1 GiB) and a timeout; the inline caps are unchanged. It only helps agents that can
  make an authenticated HTTP fetch or run a shell, so the inline caps must stay usable alone.
  The export logic lives in `mcp/export.rs`, reusing the renderer's batch stream plumbing and an
  arrow CSV/Parquet writer.
- **Errors**: FlightSQL `Status` becomes a tool result with `isError: true` and the message;
  `Unauthenticated` becomes HTTP 401 at the transport level so clients re-auth.
- **No paging in v1**: the agent aggregates or filters (answers the issue's paging question).
  No process convenience tools in v1 either.

### Read-only enforcement (`mcp/read_only.rs`)

Parse with `sqlparser` (already a DataFusion dependency) and reject, before any FlightSQL call:

- anything that is not a single `Query` statement (DDL, `INSERT`, `SET`, `COPY`, multiple
  statements, `CREATE/DROP MATERIALIZED VIEW`);
- calls to a deny list of mutating table functions/UDFs. The list is **shared with
  `view_definition.rs:281`** (move it to one `pub const` rather than keeping two copies) so a new
  mutating function added in one place is covered in both.

The same check gates `prepare_query`, `execute_query` and the export route.

### Guidance

1. **Server `instructions`** in the initialize result: the short mandatory rule list.
2. **Tool descriptions** repeat the critical rules on `prepare_query`/`execute_query`.
3. **Skills over MCP** (`io.modelcontextprotocol/skills`): serve the built-in `micromegas-query`
   skill by `include_str!` of the existing shipped
   `claude-plugin/skills/micromegas-query/SKILL.md` (single source, no copy), with a digest
   manifest computed at startup. Extra skills are loaded from `MICROMEGAS_MCP_SKILLS_DIR` (read
   once at startup, files only, no symlink escape). Layers 1 and 2 must stand alone because
   client support for layer 3 is partial.

### Attribution

Per call, set on the FlightSQL client: `x-client-type: mcp`, `x-client-agent` from the MCP
`clientInfo` (name/version), `x-client-session` from the MCP session id, a fresh `x-request-id`
(prepare and execute are correlated in the audit by session and query fingerprint, not by id). `x-client-ip` is taken from the connection, never
from a client header (same rule as `build_origin_metadata` in `http_gateway.rs`; reuse it rather
than reimplementing).

### Auth phases

- **Phase 1** (this plan): analytics key or OIDC token as bearer, static header. Query and
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
| `list_screen_types()` | read-only | Types with descriptions, as `GET /screen-types`. |
| `get_screen_type_default(screen_type)` | read-only | Default config for a type, a starting point for `create_screen`. |
| `list_screens(folder_path?)` | read-only | Name, type, folder, `managed_by`, `updated_at`; no config (keeps the list small). |
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
  tools call them. `ValidatedUser` already converts from `AuthContext`
  (`auth/claims.rs:40`), so the bearer middleware's identity feeds them directly. Name, folder
  path and screen-type validation stay in that one place.
- **Tool provider seam.** `mcp/` defines a `ToolProvider` trait (tool list + call). The query
  tools are one provider in `rust/public`; the screens provider lives in `analytics-web-srv`
  (it needs the app DB) and is registered when mounting the route. This keeps `mcp/` free of
  web-app types (open/closed) and lets another host mount only the query tools.
- **Authorization.** The web app today lets any authenticated user mutate screens, so the tools
  do the same. Additionally:
  - write tools are only registered when `MICROMEGAS_MCP_SCREEN_WRITES=true` (default off);
  - they require an OIDC identity. An analytics key has no user, so it gets the read tools only;
  - `update_screen` and `delete_screen` refuse a screen whose `managed_by` is set, since a CLI
    keeps those in sync from git and an agent edit would be overwritten or cause drift.
- **Attribution.** `created_by`/`updated_by` get the caller's email as in the UI. The info log
  line for each mutation also records the MCP client agent and session, since the row does not.
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
2. Add `rmcp` to the workspace and `rust/public/Cargo.toml` (`server` feature); check
   `cargo deny`.
3. `mcp/config.rs`: `McpConfig` (enabled, caps with hard maxima, flightsql URL, skills dir).
4. `mcp/read_only.rs` + extract the shared mutating-function list.
5. `mcp/render.rs`: capped renderer over a `RecordBatch` stream.
6. `mcp/flight.rs`: per-call FlightSQL client (bearer + attribution), prepare/execute helpers
   added to `client/flightsql_client.rs` (prepare returning schema + estimate once #1760 exists).
7. `mcp/export.rs` (streaming export route), `mcp/tools.rs`, `mcp/server.rs` (`ServerHandler`, instructions, skills resources),
   `mcp/mod.rs::register_routes`.
8. Mount in `analytics-web-srv/src/web_server.rs`; bearer validation middleware.
8a. Extract shared screen/folder store functions from `screens.rs`/`folders.rs`; HTTP handlers call them (existing handler tests must still pass unchanged).
8b. `mcp_screens.rs` provider and registration behind `MICROMEGAS_MCP_SCREEN_WRITES`; notebooks skill.
9. Docs, CHANGELOG, and an `mcp` section in the Python/CLI docs if clients are listed.

## Files to Modify

- `rust/Cargo.toml`, `rust/public/Cargo.toml` — `rmcp`.
- `rust/public/src/servers/mod.rs` — `pub mod mcp;`
- `rust/public/src/servers/mcp/{mod,config,read_only,render,flight,tools,server}.rs` — new.
- `rust/public/src/client/flightsql_client.rs` — prepare returning schema and estimate.
- `rust/analytics/src/lakehouse/view_definition.rs` (+ `query.rs`) — shared mutating-function list.
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
- **Handle vs. re-sending the query**: re-sending costs a second planning pass (the same one a
  stateless FlightSQL handle would cost) and loses an explicit prepare-to-execute id link, in
  exchange for no state and one fewer concept for the agent.

## Decisions

- Screen write tools are opt-in, OIDC-only, and never touch `managed_by` screens.
- Screen logic is extracted once and shared by the HTTP handlers and MCP tools; no duplicate SQL.

- The MCP route validates tokens itself (user call: screen management needs the identity) and
  still forwards the token so FlightSQL remains the enforcement point for queries.
- The MCP layer rejects mutating SQL even for admin keys; it is not a security boundary for
  non-admin callers (FlightSQL still is).

## Documentation

- New `mkdocs/docs/admin/mcp.md`: enabling, env vars, client configuration with a static
  `Authorization` header, tools, caps, attribution values in `flightsql_query_audit`.
- `mkdocs/docs/admin/monolith.md` and `web-app.md`: mention the `/mcp` route and flag.
- `CHANGELOG.md`: new feature entry.

## Testing Strategy

No-DB unit tests (the default tier):
- `read_only`: accepts a plain select/CTE/`view_instance` query; rejects DDL, DML, `SET`,
  multi-statement, each mutating function (iterating the shared list so a new entry is covered),
  case/quoting variants, and nested calls inside subqueries.
- `render`: row cap, byte cap, truncation message, empty result, wide/NULL/nested types,
  stops consuming the stream at the cap (use a counting stream).
- `screens` (no-DB: store functions behind a trait seam with canned data): `list_screens` omits config; `create_screen` defaults config from the type; `update_screen` honors `expected_updated_at` and refuses `managed_by` rows; `delete_screen` refuses `managed_by`; write tools absent when the flag is off or the caller is an analytics key; invalid name/folder/type map to `isError`.
- Existing `screens.rs`/`folders.rs` tests prove the extraction didn't change HTTP behavior.
- `export`: streams CSV and Parquet from a canned batch stream without buffering it, stops at the byte cap, requires the bearer header, rejects a query FlightSQL refuses.
- `config`: hard maxima clamp a larger env value; `limit` defaults to the default row cap and clamps to the hard row maximum.
- Tool layer against a fake FlightSQL seam returning canned `FlightInfo`/streams: over-cap hint,
  opaque-without-range hint, `Status` -> `isError`, `Unauthenticated` -> 401, same arguments accepted by prepare and execute.
- Router test (no DB, lazily-connected state): missing bearer -> 401 with `WWW-Authenticate`;
  disabled flag -> 404; `initialize` returns `instructions` and the skills capability.
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
3. Query `micromegas-query "SELECT client_type, count(*) FROM flightsql_query_audit GROUP BY 1"`
   and expect an `mcp` row. Not automated: it needs a running service and a real MCP client.

## Open Questions

- Is a bearer-authenticated export link enough, or do hosted clients that can't fetch with a header need another delivery path (e.g. MCP resources read over the transport, with paging)?

- Should #1758 metadata carry machine-readable per-view limits that `prepare_query` enforces?
- Should the FlightSQL target be allowed to differ from the web app's default data source?
- Are write tools opt-in (`MICROMEGAS_MCP_SCREEN_WRITES`, OIDC callers only, `managed_by` rows refused) the right default, or should analytics keys be able to write too?
- Should folder rename/delete be exposed later?
- Rate limiting per key beyond the admission scheduler, or leave to #1748?
