# Admin Mode for Destructive Statements (Issue #1769) Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1769

## Overview

Notebook cells and admin pages send SQL to the same `/api/query-stream` endpoint, using the
viewing user's token. So when an admin opens someone else's notebook, any `DROP MATERIALIZED VIEW`,
`regenerate_partitions(...)` or `deny_queries(...)` in it runs with admin rights. This plan splits
the web server's query path in two:

- `/api/query-stream` refuses every lakehouse-mutating admin statement.
- A new, admin-only `/api/admin/query-stream` accepts them.

Which endpoint a page uses is decided by the viewing admin, never by the notebook's content:

- **Admin pages** (`/admin/*`) are always in admin mode, because being there is the choice.
- **Notebooks** are never in admin mode by default. An admin can turn it on for the current page
  visit, so admin notebooks still work, but only as a choice made by the person viewing them.

While admin mode is on, the page's chrome changes colour so the elevated state is always visible.

## Current State

**Server guard.** `rust/analytics-web-srv/src/stream_query.rs`:

- `BLOCKED_FUNCTIONS` (`:90`) and `contains_blocked_function` (`:97`) do a lower-cased substring
  match on three names: `retire_partitions`, `retire_partition_by_metadata` and
  `retire_partition_by_file`.
- `stream_query_handler` (`:187`) runs that check on `request.sql` *before* `substitute_macros`
  (`:261`), so the text that is checked is not the text that runs.
- Nothing checks DDL (`CREATE [OR REPLACE] / DROP MATERIALIZED VIEW`), which FlightSQL
  intercepts in `do_get` (`rust/public/src/servers/flight_sql_service_impl.rs:919`).
- Nothing checks `materialize_partitions`, `regenerate_partitions`, `deny_queries` or
  `remove_query_denial`.

All of these are registered only for admins (`rust/analytics/src/lakehouse/query.rs:209-270`), so
a non-admin gets "function not found" whatever happens. The risk is an *admin* viewing content
written by someone else.

**Admin-gated functions** (`query.rs:209-270`):

- Mutating: `retire_partitions`, `materialize_partitions`, `regenerate_partitions`,
  `retire_partition_by_file`, `retire_partition_by_metadata`, `deny_queries` and
  `remove_query_denial`.
- Read-only: `list_query_denials`, `list_view_set_definitions` and `check_view_merge`.

**DDL detection.** `micromegas_analytics::lakehouse::view_ddl::parse_view_ddl`
(`rust/analytics/src/lakehouse/view_ddl.rs:51`) is the function FlightSQL uses to decide whether a
statement is view DDL. It is re-exported as `micromegas::servers::view_ddl`. Its results are:

- `Ok(None)`: not DDL.
- `Ok(Some(_))`: DDL.
- `Err`: the statement starts like `CREATE`/`DROP MATERIALIZED VIEW` but is malformed, or a
  comment-led statement fails to tokenize.

**Routing.** `build_protected_routes` (`rust/analytics-web-srv/src/web_server.rs:366`) mounts
`/api/query-stream` behind `cookie_auth_middleware`. Admin REST routes use the `AdminUser`
extractor (`rust/analytics-web-srv/src/auth/handlers.rs:583`), as `settings.rs` does. Under
`--disable-auth` a hardcoded admin `ValidatedUser` is layered on every request (`web_server.rs:465`).

**Front end.**

- `streamQuery` / `fetchQueryIPC` (`analytics-web-app/src/lib/arrow-stream.ts:157,325`) hardcode
  `/query-stream`.
- `useStreamQuery` (`src/hooks/useStreamQuery.ts`) wraps `streamQuery`. `QueryDenyListPage.tsx`
  issues `list_query_denials` / `deny_queries` / `remove_query_denial` through it.
- Notebook cells execute in `src/lib/screen-renderers/useCellExecution.ts`, through `executeSql`
  (`:60`) and the two `fetchQueryIPC` calls (`:289,327`).
- Notebooks auto-run on mount and on refresh in `useCellExecution.ts:474-489`; `ScreenPage` only
  holds `handleRefresh` / `useRefreshInterval` (`:233`).
- Under `--disable-auth`, `auth_me_no_auth` (`web_server.rs:202,662-670`) returns 200 with
  `is_admin: true`, so the identity menu renders and offers the toggle (`lib/auth.tsx:58-62`,
  `Header.tsx:138`).
- The global header is `components/layout/Header.tsx`, rendered through `PageLayout`.
- The admin routes are listed in `router.tsx:55-63`, and `AppShell.tsx` already keeps an
  `ADMIN_ONLY_PATHS` list.

**Known limitation this removes.** `mkdocs/docs/admin/web-app.md:204-210` documents that the deny
list page cannot create a rule whose expression mentions `retire_partitions`, because the substring
guard trips on the string literal.

## Design

### 1. Classifying statements (server)

New module `rust/analytics-web-srv/src/admin_statements.rs`:

```rust
pub enum AdminStatement {
    ViewDdl,                  // parse_view_ddl returned Some or Err
    Function(&'static str),   // name of the matched mutating function
}

const MUTATING_ADMIN_FUNCTIONS: &[&str] = &[
    "retire_partitions", "retire_partition_by_metadata", "retire_partition_by_file",
    "materialize_partitions", "regenerate_partitions",
    "deny_queries", "remove_query_denial",
];

pub fn classify_admin_statement(sql: &str) -> Option<AdminStatement>;
```

- **DDL** is detected with `parse_view_ddl`, so the web server and FlightSQL agree exactly on what
  counts as DDL. `Err` also counts as DDL (fail closed). FlightSQL would reject that text anyway,
  and an admin page gets the real parse error back from the admin endpoint. A comment-led plain
  `SELECT` that fails to tokenize also returns `Err`, so it is refused too.
- **Functions** keep the current lower-cased substring match, extended to the full mutating list.
  It over-blocks, for example a log search for `'%regenerate_partitions%'`, but it never
  under-blocks. Comments, quoted identifiers and unusual whitespace all still match.
- The read-only admin functions are **not** in the list and stay on `/api/query-stream`.
- `BLOCKED_FUNCTIONS` and `contains_blocked_function` are removed, and their tests move to the new
  module.

### 2. Two endpoints (server)

Split `stream_query_handler` into a shared `run_stream_query(auth_token, cache, request, mode)`
and two thin handlers:

| Route | Gate | Classification |
|---|---|---|
| `POST /api/query-stream` (unchanged URL) | authenticated | an `AdminStatement` → `403 FORBIDDEN` |
| `POST /api/admin/query-stream` (new) | `AdminUser` → `403` for non-admins | not checked |

`mode` is an enum (`QueryMode::Standard | QueryMode::Admin`), not a bool, so each call site states
what it means. `run_stream_query` and `QueryMode` are `pub` (like the existing
`stream_query_handler`) so the integration tests in `rust/analytics-web-srv/tests/` can reach them. In `Standard` mode the check runs on the SQL **after** `substitute_macros`, which
fixes the check-before-substitute gap described in Current State.

- **Forbidden error message.** It names the statement and states the fix: `'regenerate_partitions'
  modifies the lakehouse; run it from an admin page, or, as an admin, enable admin mode on a
  notebook from the user menu`. For a `ViewDdl` that came from `Err`, it keeps the refusal but says
  the statement could not be parsed as view DDL. It uses the existing `ErrorCode::Forbidden`.
- **Attribution.** Admin-mode requests use FlightSQL client type `web-admin` instead of `web`, so
  `x-client-type` (and therefore the query audit log) shows which statements came through the
  elevated path. The info log line gains `mode=admin`.
- **`--disable-auth`.** Everyone is admin, so the admin endpoint is open to everyone, as every
  admin route already is in that mode. `/api/query-stream` still refuses admin statements, so the
  notebook protection holds there too.

### 3. Choosing the endpoint (front end)

- `StreamQueryParams` gains `adminMode?: boolean`.
- `streamQuery` and `fetchQueryIPC` build their URL with one helper,
  `queryStreamUrl(adminMode) → ${getApiBase()}/query-stream | /admin/query-stream`.
- `useStreamQuery` passes the flag through unchanged.

**Admin pages: always on.**

- `QueryDenyListPage` passes `adminMode: true` on all three of its queries. Any query issued from
  an admin page uses the admin endpoint.
- The View Sets page from #1768 does the same, so there is no toggle and no disabled state there.
- The red chrome (§4) is driven by one `isAdminRoute(pathname)` helper in `src/lib/admin-mode.tsx`,
  which owns the `ADMIN_ONLY_PATHS` list (moved out of `AppShell.tsx`; `AppShell` imports it) and matches (exact `/admin/<name>` prefixes, not `startsWith('/admin')`).
  `/admin` and `/admin/ingestion-keys` are excluded because they are role-filtered and visible to
  non-admins (`AppShell.tsx:7-9`).

**Notebooks: viewer's choice, per page visit.**

- `ScreenPage` holds `adminMode` in React state, initially `false`. Nothing on the page itself
  shows admin mode while it is off.
- The control is an **Enable admin mode** item in the identity menu (`Header.tsx`, between
  *Audience access* and *Sign out*). It appears only when `user.is_admin` and the current page
  allows the toggle. A notebook `ScreenPage` registers that through `AdminModeContext` (§4); the
  menu stays as it is everywhere else. While the mode is on, the item reads **Disable admin
  mode**.
- Non-notebook screen types (Table/Log/ProcessList/Metrics renderers) never get admin mode; their
  mutating statements are always refused. The `kind: 'toggle'` registration is gated on
  `screenType === 'notebook'`.
- It passes `adminMode` to the renderer through `ScreenRendererProps`. Only `NotebookRenderer`
  reads it, and it forwards it to `useCellExecution`.
- `useCellExecution` reads it through `useLatestRef`, the way it reads `viewer`, so toggling never
  re-runs cells on its own. The admin re-runs the cell, or the whole notebook, explicitly. The
  forbidden error says to do so.
- The flag is set on `executeSql` and both `fetchQueryIPC` calls in `useCellExecution`.
  `executeSql` (module-level, `:60`) gains an `adminMode` parameter, passed from both call sites,
  `runQuery` (`:306`) and `runQueryAs` (`:343`).
  `PerfettoExportCell` / `perfetto-trace.ts` issue app-generated, non-mutating SQL and stay on the
  standard endpoint.
- A reload, navigating away, or opening another screen resets it: `ScreenPage` resets the state
  when the route (name / isNew / typeParam) changes, and also when the draft is discarded, because
  discard remounts the generic renderer element (key `${screen?.name ?? 'new'}:${discardNonce}`, `ScreenPage.tsx:502`) and its mount
  effect would otherwise re-run every cell with the mode still on.
- **Auto-refresh is paused while it is on.** `ScreenPage` passes `adminMode ? 0 :
  refreshIntervalMs` to `useRefreshInterval`, so a mutating cell is never re-run on a timer. The
  stored interval (URL/config) is untouched and resumes when the mode is turned off. `Header`
  reads `AdminModeContext` (`active && kind === 'toggle'`) and passes a `paused` flag to
  `RefreshIntervalPicker` (both render sites, `Header.tsx:103` and `:120`), which disables the interval dropdown, appends "· paused" to the interval chip label
  (`:49-53`, e.g. "30s · paused") and replaces the "Auto-refreshing every X" title (`:45`) with
  "Paused while admin mode is on"; the Refresh button stays active.
- **Auto-execute is off while it is on.** Only explicit actions run cells: a cell's Run / Run
  from here, and the header's Refresh button (run all). These triggers that normally run cells by
  themselves become no-ops:
  - a time-range change (`useCellExecution`'s `rawTimeRange` effect);
  - a selection change in a table/chart cell (`updateCellSelection` still records the selection
    but doesn't re-run downstream cells);
  - editing the config or SQL of a cell with `autoRunFromHere` (`useNotebookAutoRun.scheduleAutoRun`'s
    300ms debounce, which would otherwise run a half-typed `DROP` as the admin types it);
  - picking a variable value on a cell with `autoRunFromHere`
    (`useNotebookAutoRun.triggerAutoRun`);
  - a sort change on a table/log cell (`useCellSortCheck`, which re-runs the cell's SQL through
    `executeCellByName`);
  - the mount effect (`useCellExecution.ts:475-480`): while the mode is on it skips the initial
    run, and once the engine is ready and cells exist it sets `hasExecutedRef = true`. The WASM
    engine loads asynchronously, and a new empty notebook runs `executeFromCell(0)` when its first
    cell is added, either of which can happen after the mode was turned on. Consuming the run
    matters because the effect deps (`[cells.length, engine, executeFromCell]`, `:480`) would
    otherwise re-fire on the first cell edit, time-range or data-source change after the mode is
    turned off and run `executeFromCell(0)`. Turning the mode off never replays it; the notebook
    waits for an explicit Run / Refresh.

  `scheduleAutoRun` reads the flag inside its 300ms timer callback (`useNotebookAutoRun.ts:64-67`),
  not only when scheduling, so a run scheduled before the mode was turned on is dropped. These hooks
  read the flag through a ref, so turning the mode off restores auto-execution with no re-run. Cells whose inputs changed keep their previous results until the admin runs them; there
  is no stale marker.

### 4. Visual treatment

A new `AdminModeContext` (provider in `AppShell`) exposes `{ kind: 'none' | 'page' | 'toggle',
active, setActive? }`:

- `AppShell`'s provider derives `kind: 'page'` from `isAdminRoute(pathname)` (always active, no
  `setActive`).
- A notebook `ScreenPage` sets `kind: 'toggle'` and supplies `setActive`.
- Leaving either resets it to `none`; `ScreenPage` clears its toggle registration on unmount.

`Header` uses it for the menu item and the pill. `Header` and `AppShell` both use it for the
chrome when `active` is true:

- a 4px hatched red stripe above the header, and a 2px red frame on the app shell
  (`--admin: #dc2626`, the existing `--accent-error-bright`);
- an `ADMIN` pill in the global header. In toggle mode it reads `ADMIN MODE` and has a **Turn
  off** button.

What the mode allows, and
that it lasts for this page only, is said once, in the menu item's subtitle ("This page only.
Lets cells modify the lakehouse.").

## Mockups

Open in a browser (self-contained; tokens copied from `src/styles/globals.css`):

- `tasks/1769_admin_mode_mockups/option-a-in-place-toggle.html`: **chosen**. Three states:
  - a notebook with the mode off: no admin control on the page, *Enable admin mode* in the open
    identity menu, and a refused cell with the hint;
  - the same notebook with the mode on, with the stripe, frame and pill;
  - an admin page, where the mode is always on.
- `tasks/1769_admin_mode_mockups/option-b-admin-tab.html` (not chosen): **Open as admin ↗** opens
  a second, fully re-themed browser tab.

## Implementation Steps

1. **Server classification:** `admin_statements.rs` with `classify_admin_statement`; register it
   in `lib.rs`; remove `BLOCKED_FUNCTIONS` / `contains_blocked_function`.
2. **Server endpoints:** in `stream_query.rs`, split out `run_stream_query` with `QueryMode` (both `pub`),
   run `substitute_macros` and the classification before the empty-data-source check and
   `cache.resolve`, add the `web-admin` client type, and add
   `admin_stream_query_handler` with the `AdminUser` extractor. In `web_server.rs`, mount
   `/api/admin/query-stream` next to `/api/query-stream`.
3. **Front-end plumbing:** `adminMode` in `StreamQueryParams`; `queryStreamUrl` in
   `arrow-stream.ts`; `useStreamQuery` passes it through.
4. **Admin pages:** `QueryDenyListPage` passes `adminMode: true`.
5. **Admin mode context:** `src/lib/admin-mode.tsx` (`AdminModeContext`, `isAdminRoute`, `ADMIN_ONLY_PATHS`);
   provider and frame in `AppShell.tsx`; stripe, pill and the identity-menu item in `Header.tsx`;
   `RefreshIntervalPicker.tsx` (`paused` passed at both `Header` render sites) disables the interval
   dropdown and shows *paused*.
6. **Notebook toggle:** `ScreenPage.tsx` (state, registering `kind: 'toggle'`, reset on route
   change and on discard); `ScreenRendererProps.adminMode`; `NotebookRenderer.tsx` →
   `useCellExecution.ts` (endpoint, auto-execute gates), `useNotebookAutoRun.ts` and
   `useCellSortCheck.ts` (gates).
7. **Update the #1768 plan:** in `tasks/1768_view_sets_admin_ui_plan.md` §3, replace "shows the
   admin-mode toggle state and disables Create / Edit / Drop with a prompt to enable it while off"
   with "issues every query with `adminMode: true`", and drop or reword its Decisions entry
   "Writes depend on #1769's admin mode; a client-side toggle alone is not a security boundary.".
   Also have it add its View Sets route to the `ADMIN_ONLY_PATHS` list in `admin-mode.tsx`, otherwise the page gets no red chrome. Reword its stale text: Current State (`:30`, cites `BLOCKED_FUNCTIONS`, which this plan removes), the partition-retirement row (`:135`, "blocked on the web path by design") and Decisions (`:187`, "`retire_partitions*` is blocked for web queries on purpose"); leaving out retire/rebuild is a scope choice, not a web-path block.
8. **Docs and `CHANGELOG.md`.**

## Files to Modify

- New: `rust/analytics-web-srv/src/admin_statements.rs`,
  `rust/analytics-web-srv/tests/admin_statements_tests.rs`, `analytics-web-app/src/lib/admin-mode.tsx`
- Rust: `rust/analytics-web-srv/src/stream_query.rs`, `src/web_server.rs`, `src/lib.rs`,
  `tests/stream_query_tests.rs`, `tests/routing_tests.rs`
- Web app: `src/lib/arrow-stream.ts`, `src/hooks/useStreamQuery.ts`,
  `src/lib/query-deny-list-api.ts` (header comment: queries go through the admin endpoint via
  `adminMode: true`, not `/api/query-stream`), `src/routes/QueryDenyListPage.tsx`,
  `src/routes/ScreenPage.tsx`,
  `src/lib/screen-renderers/NotebookRenderer.tsx`, `src/lib/screen-renderers/useCellExecution.ts`,
  `src/lib/screen-renderers/useNotebookAutoRun.ts`,
  `src/lib/screen-renderers/useCellSortCheck.ts`,
  `src/lib/screen-renderers/index.ts` (`ScreenRendererProps`, `:16-46`),
  `src/components/layout/AppShell.tsx`, `src/components/layout/Header.tsx`,
  `src/components/layout/RefreshIntervalPicker.tsx`, plus tests
- Docs: `mkdocs/docs/admin/web-app.md`, `mkdocs/docs/query-guide/query-audit-log.md`,
  `mkdocs/docs/admin/functions-reference.md`,
  `mkdocs/docs/admin/materialized-views.md`,
  `mkdocs/docs/web-app/notebooks/execution.md`, `CHANGELOG.md`, `tasks/1768_view_sets_admin_ui_plan.md`

## Trade-offs

- **Endpoint choice vs. a short-lived signed admin-mode credential (cookie or header), as the issue
  proposed.** A credential guards against the elevation being triggered by something the viewer
  didn't do. Here nothing the notebook controls can choose the endpoint, so a token would add
  minting, expiry and storage without closing any path. Expiry is replaced by the mode ending with
  the page visit.
- **Re-authentication step (sudo mode).** This is the strongest form of "explicit". It needs an
  OIDC `prompt=login` / `max_age` round trip and pays off when the threat is a stolen session,
  which this issue isn't about. It can be added later on the admin endpoint without changing the
  front end's endpoint choice.
- **Substring match vs. AST walk for functions.** The walk is precise, but it can let a call
  through any expression or table-factor position the visitor misses. Substring matching
  can only over-block, and over-blocking now costs a click rather than "use the CLI".
- **Enforce in FlightSQL (a client-supplied "restricted" header that skips registering mutating
  functions and refuses DDL)** instead of in the web server. It is exact, but it touches the public
  FlightSQL service and every session build for a web-only concern. The web server is the only
  client that mixes untrusted content with an admin's token.

## Decisions

- Admin pages are implicitly in admin mode. Notebooks are off by default and the viewing admin
  turns them on; the author never can (user call).
- Admin mode is never persisted (config, URL or storage) and resets on reload or navigation.
- Read-only admin functions (`list_query_denials`, `list_view_set_definitions`, `check_view_merge`)
  stay allowed on `/api/query-stream`.
- Turning admin mode on does not re-run cells.
- Option A (in-place toggle with red chrome) over a separate admin tab (user call).
- The notebook toggle sits in the identity menu, not on the page, so there is no permanent "admin
  mode off" control (user call).
- No in-page admin-mode banner; the header stripe, frame and pill are the indicator (user call).
- Auto-refresh is paused while notebook admin mode is on; explicit refresh/run still works (user
  call).
- Auto-execute (time range, selection, config edit, variable auto-run, sort change, on-mount run) is off while notebook admin
  mode is on; only Run, Run from here and Refresh execute (user call).

## Security

- **Not in scope:** stolen sessions (see the re-authentication trade-off), and a malicious admin.
  FlightSQL's own admin gate still applies to everything.
- **Residual:** an admin who turns admin mode on in a notebook they haven't read is running that
  notebook's statements. The red chrome and the explicit re-run limit this, but don't prevent it.

## Documentation

- `mkdocs/docs/admin/web-app.md`: a new "Admin mode" section covering the two endpoints, implicit
  admin pages, the notebook toggle, what is refused, and the red chrome. Rewrite the Query Deny
  List text at about `:184-210` (it says the page uses `POST /api/query-stream`, and documents the
  substring-guard limitation, which is gone). Document `POST
  /api/admin/query-stream` next to `/api/query-stream`.
- `mkdocs/docs/query-guide/query-audit-log.md`: add `web-admin` to the client column values
  (`:204`) and update the attribution text at `:259-260` and `:277-281`, which says
  `/api/query-stream` requests are `client="web"`. Note in it and in `web-app.md` that a deny rule
  targeting `client = 'web'` no longer covers admin-mode queries (client `web-admin`).
- `mkdocs/docs/web-app/notebooks/execution.md` (auto-refresh `:39`, auto-run `:41-58`): say auto-refresh
  and auto-run are paused while notebook admin mode is on.
- `mkdocs/docs/admin/functions-reference.md` and `materialized-views.md`: one line each saying
  that from the web app these run only from admin pages or in a notebook with admin mode on.
- `CHANGELOG.md` (Web app): new endpoint, the extended refusal (DDL plus four more functions), the
  fixed check-before-substitution gap, and the `web-admin` client type. The SQL surface is
  unchanged. **Minor breaking change:** `contains_blocked_function` / `BLOCKED_FUNCTIONS` are
  removed from `analytics_web_srv::stream_query`. Notebooks that ran these statements through
  `/api/query-stream` now need admin mode.

## Testing Strategy

All coverage is no-DB unit tests.

**Rust** (`tests/admin_statements_tests.rs`):

- Each mutating function in any case, inside comments, quoted identifiers and subqueries →
  matched.
- Each read-only admin function → not matched.
- DDL: `CREATE`, `CREATE OR REPLACE` and `DROP MATERIALIZED VIEW`, with leading `--` / `/* */`
  comments and lower-case keywords → `ViewDdl`.
- A malformed `CREATE MATERIALIZED VIEW x` (no `WITH`) → `ViewDdl` (fail closed).
- `CREATE VIEW` / `CREATE TABLE` / plain `SELECT` → `None`.

**Rust** (`tests/stream_query_tests.rs`, handler level):

- `Standard` mode refuses a statement that only becomes `regenerate_partitions(...)` after
  `substitute_macros`. This pins the ordering fix.
- `Admin` mode does not classify.
- The refusal happens before data-source resolution: the request has a non-empty `data_source` and
  a failing cache (`unreachable_pool()` from `maps_tests.rs`, 50ms `acquire_timeout`, +
  `DataSourceCache::new`; not `routing_tests.rs:420` `lazy_pool()`, whose sqlx default 30s timeout
  would stall), which proves no FlightSQL connection was attempted.
- Admin mode + mutating SQL + empty `data_source` → `400 DATA_SOURCE_NOT_FOUND`, while `Standard`
  mode with the same input → `403`.
- Non-admin 403 / admin-reaches-handler on `/api/admin/query-stream`: see the handler-router test
  below; it lives in this file.

**Rust** (`tests/routing_tests.rs`):

- `/api/admin/query-stream` is mounted under `--disable-auth` (`disabled_auth_app()`, `:425`).
  `build_protected_routes(&None)` always injects an admin `ValidatedUser`, so the 403 case is not
  tested here.
- POST to `/api/admin/query-stream` with an empty `data_source` → `400` rather than `404`.

**Rust** (handler-router test in `tests/stream_query_tests.rs`; `build_handler_router_with_user` and
`non_admin_user()` are copied from `maps_tests.rs:89-99`, as there is no shared `tests/common`):

- `/api/admin/query-stream` returns 403 for `non_admin_user()` and reaches the handler for an
  admin.

**Vitest:**

- `arrow-stream`: `adminMode` selects the URL for both `streamQuery` and `fetchQueryIPC`.
- `useCellExecution`: `adminMode` reaches every remote call path (stream, IPC, `runQueryAs`), and
  flipping it doesn't trigger execution.
- `QueryDenyListPage`: every query is sent with `adminMode: true`.
- `useCellExecution`: with admin mode on, a time-range change and a selection change don't
  execute, an explicit `executeCell` / `executeFromCell` / refresh trigger does, and turning the
  mode off doesn't run anything.
- `useCellExecution`: with admin mode on, the engine becoming ready or the first cell being added
  executes nothing; after turning the mode off, a cell edit, time-range change or data-source
  change doesn't run `executeFromCell(0)` either (the mount effect consumed the initial run).
- `useNotebookAutoRun`: with admin mode on, `scheduleAutoRun` and `triggerAutoRun` don't execute.
- `useNotebookAutoRun`: `scheduleAutoRun` called, then admin mode enabled, then timers advanced
  → no execution.
- `useCellSortCheck`: with admin mode on, a sort change doesn't re-run the cell's SQL.
- `ScreenPage`: with admin mode on, `useRefreshInterval` gets `0` and the stored interval is
  unchanged; turning it off restores the interval.
- `ScreenPage`: the state resets when the route changes and when the draft is discarded; a saved
  config never contains it.
- `Header`: the identity menu offers *Enable admin mode* only to an admin on a notebook (not to a
  non-admin, not on other screen types or pages). It becomes *Disable admin mode* while on. The
  pill's *Turn off* clears it. While the toggle is on, the refresh picker is passed `paused` at both render sites:
  dropdown disabled, chip label ends in "· paused", title reads *paused*.
- `AppShell` / `Header`: the chrome appears on admin routes and with the toggle on, and not on
  `/admin` or `/admin/ingestion-keys`.

## Manual Verification

Against `python3 local_test_env/ai_scripts/start_services.py --monolith --disable-auth`, so the
user is admin:

1. Create a notebook whose cell is `SELECT * FROM regenerate_partitions('log_stats', now() -
   INTERVAL '1 hour', now(), 3600)`. Expected: the cell errors with the admin-mode hint.
2. Open the identity menu, choose **Enable admin mode**, and re-run the cell. Expected: red
   stripe, frame and pill appear, and the cell returns task-progress log rows. The query audit log
   shows `client='web-admin'` for the re-run.
3. On Admin → Query Deny List, add a rule with expression `sql LIKE '%retire_partitions%'`.
   Expected: the rule is created (it was refused before). Remove the rule afterwards.

