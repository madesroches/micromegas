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
- `stream_query_handler` (`:197`) runs that check on `request.sql` *before* `substitute_macros`
  (`:264`), so the text that is checked is not the text that runs.
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
  (`:61`) and the two `fetchQueryIPC` calls (`:289,327`).
- Notebooks auto-run on mount, on refresh, and on the auto-refresh interval
  (`routes/ScreenPage.tsx:219-233`).
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
  and an admin page gets the real parse error back from the admin endpoint.
- **Functions** keep the current lower-cased substring match, extended to the full mutating list.
  It over-blocks, for example a log search for `'%regenerate_partitions%'`, but it never
  under-blocks. Comments, quoted identifiers and unusual whitespace all still match. An AST walk
  would be precise but would let through any node type the walker misses (see Trade-offs).
  Over-blocking is cheap now, because a blocked statement only needs admin mode, which an admin
  can turn on.
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
what it means. In `Standard` mode the check runs on the SQL **after** `substitute_macros`, which
fixes the check-before-substitute gap described in Current State.

- **Forbidden error message.** It names the statement and states the fix: `'regenerate_partitions'
  modifies the lakehouse; run it from an admin page, or enable admin mode for this notebook from
  the user menu`. It uses the existing `ErrorCode::Forbidden`.
- **Attribution.** Admin-mode requests use FlightSQL client type `web-admin` instead of `web`, so
  `x-client-type` (and therefore the query audit log) shows which statements came through the
  elevated path. The info log line gains `mode=admin`.
- **No extra credential.** The issue suggested a short-lived admin-mode token. It isn't needed,
  because the elevation is *which endpoint the app calls*, and the code that chooses the endpoint
  depends only on the viewer's own actions (§3). Notebook config, cell SQL, variables and the URL
  cannot reach it.
- **Cross-site requests.** The posture is the same as today's `/api/query-stream`:
  `SameSite=Lax` cookies and a JSON body, which needs a CORS preflight.
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
- The red chrome (§4) is driven by one `isAdminRoute(pathname)` helper in `AppShell` (`/admin/*`,
  excluding the role-filtered `/admin` landing page and `/admin/ingestion-keys`, which issue no
  SQL). The same helper backs `ADMIN_ONLY_PATHS` where the lists overlap.

**Notebooks: viewer's choice, per page visit.**

- `ScreenPage` holds `adminMode` in React state, initially `false`. Nothing on the page itself
  shows admin mode while it is off.
- The control is an **Enable admin mode** item in the identity menu (`Header.tsx`, between
  *Audience access* and *Sign out*). It appears only when `user.is_admin` and the current page
  allows the toggle. A notebook `ScreenPage` registers that through `AdminModeContext` (§4); the
  menu stays as it is everywhere else. While the mode is on, the item reads **Disable admin
  mode**, and the header pill's **Turn off** does the same.
- It passes `adminMode` to the renderer through `ScreenRendererProps`. Only `NotebookRenderer`
  reads it, and it forwards it to `useCellExecution`.
- `useCellExecution` reads it through `useLatestRef`, the way it reads `viewer`, so toggling never
  re-runs cells on its own. The admin re-runs the cell, or the whole notebook, explicitly. The
  forbidden error says to do so.
- The flag is set on `executeSql` and both `fetchQueryIPC` calls in `useCellExecution`.
  `PerfettoExportCell` / `perfetto-trace.ts` issue app-generated, non-mutating SQL and stay on the
  standard endpoint.
- **It is never persisted.** It is not stored in the notebook config, the URL or any storage. A
  shared link or a saved notebook therefore cannot turn it on. A reload, navigating away, or
  opening another screen resets it: `ScreenPage` resets the state when `screen?.name` changes.
- **Auto-refresh is paused while it is on.** `ScreenPage` passes `adminMode ? 0 :
  refreshIntervalMs` to `useRefreshInterval`, so a mutating cell is never re-run on a timer. The
  stored interval (URL/config) is untouched and resumes when the mode is turned off. The header's
  interval picker is disabled and shows *paused* (tooltip: "Paused while admin mode is on").
- **Auto-execute is off while it is on.** Only explicit actions run cells: a cell's Run / Run
  from here, and the header's Refresh button (run all). These triggers that normally run cells by
  themselves become no-ops:
  - a time-range change (`useCellExecution`'s `rawTimeRange` effect);
  - a selection change in a table/chart cell (`updateCellSelection` still records the selection
    but doesn't re-run downstream cells);
  - editing a cell's config or SQL (`useNotebookAutoRun.scheduleAutoRun`'s 300ms debounce, which
    would otherwise run a half-typed `DROP` as the admin types it);
  - picking a variable value on a cell with `autoRunFromHere`
    (`useNotebookAutoRun.triggerAutoRun`).

  Both hooks read the flag through a ref, so turning the mode off restores auto-execution with no
  re-run. Cells whose inputs changed keep their previous results until the admin runs them; there
  is no stale marker. The on-mount run is unaffected because the mode always starts off.

### 4. Visual treatment

A new `AdminModeContext` (provider in `AppShell`) exposes `{ kind: 'none' | 'page' | 'toggle',
active, setActive? }`:

- Admin routes set `kind: 'page'` (always active, no `setActive`).
- A notebook `ScreenPage` sets `kind: 'toggle'` and supplies `setActive`.
- Leaving either resets it to `none`.

`Header` uses it for the menu item and the pill. `Header` and `AppShell` both use it for the
chrome when `active` is true:

- a 4px hatched red stripe above the header, and a 2px red frame on the app shell
  (`--admin: #dc2626`, the existing `--accent-error-bright`);
- an `ADMIN` pill in the global header. In toggle mode it reads `ADMIN MODE` and has a **Turn
  off** button.

There is no in-page banner: the header chrome is the indicator. What the mode allows, and
that it lasts for this page only, is said once, in the menu item's subtitle ("This page only.
Lets cells modify the lakehouse.").

Red always means the same thing: statements from this page can modify the lakehouse. Reads and
writes on admin pages need no extra control.

## Mockups

Open in a browser (self-contained; tokens copied from `src/styles/globals.css`):

- `tasks/1769_admin_mode_mockups/option-a-in-place-toggle.html`: **chosen**. Three states:
  - a notebook with the mode off: no admin control on the page, *Enable admin mode* in the open
    identity menu, and a refused cell with the hint;
  - the same notebook with the mode on, with the stripe, frame and pill;
  - an admin page, where the mode is always on.
- `tasks/1769_admin_mode_mockups/option-b-admin-tab.html` (not chosen): **Open as admin ↗** opens
  a second, fully re-themed browser tab. See Trade-offs.

## Implementation Steps

1. **Server classification:** `admin_statements.rs` with `classify_admin_statement`; register it
   in `lib.rs`; remove `BLOCKED_FUNCTIONS` / `contains_blocked_function`.
2. **Server endpoints:** in `stream_query.rs`, split out `run_stream_query` with `QueryMode`,
   check after `substitute_macros`, add the `web-admin` client type, and add
   `admin_stream_query_handler` with the `AdminUser` extractor. In `web_server.rs`, mount
   `/api/admin/query-stream` next to `/api/query-stream`.
3. **Front-end plumbing:** `adminMode` in `StreamQueryParams`; `queryStreamUrl` in
   `arrow-stream.ts`; `useStreamQuery` passes it through.
4. **Admin pages:** `QueryDenyListPage` passes `adminMode: true`.
5. **Admin mode context:** `src/lib/admin-mode.tsx` (`AdminModeContext`, `isAdminRoute`);
   provider and frame in `AppShell.tsx`; stripe, pill and the identity-menu item in `Header.tsx`.
   Check that the identity menu renders under `--disable-auth`; the manual run in Manual
   Verification depends on it.
6. **Notebook toggle:** `ScreenPage.tsx` (state, registering `kind: 'toggle'`, reset on screen
   change); `ScreenRendererProps.adminMode`; `NotebookRenderer.tsx` →
   `useCellExecution.ts` (endpoint, auto-execute gates) and `useNotebookAutoRun.ts` (gates).
7. **Update the #1768 plan:** in `tasks/1768_view_sets_admin_ui_plan.md` §3, replace "shows the
   admin-mode toggle state and disables Create / Edit / Drop" with "issues every query with
   `adminMode: true`", and drop the toggle from its Decisions.
8. **Docs and `CHANGELOG.md`.**

## Files to Modify

- New: `rust/analytics-web-srv/src/admin_statements.rs`,
  `rust/analytics-web-srv/tests/admin_statements_tests.rs`, `analytics-web-app/src/lib/admin-mode.tsx`
- Rust: `rust/analytics-web-srv/src/stream_query.rs`, `src/web_server.rs`, `src/lib.rs`,
  `tests/stream_query_tests.rs`, `tests/routing_tests.rs`
- Web app: `src/lib/arrow-stream.ts`, `src/hooks/useStreamQuery.ts`,
  `src/routes/QueryDenyListPage.tsx`, `src/routes/ScreenPage.tsx`,
  `src/lib/screen-renderers/NotebookRenderer.tsx`, `src/lib/screen-renderers/useCellExecution.ts`,
  `src/lib/screen-renderers/useNotebookAutoRun.ts`,
  the `ScreenRendererProps` type, `src/components/layout/AppShell.tsx`,
  `src/components/layout/Header.tsx`, plus tests
- Docs: `mkdocs/docs/admin/web-app.md`, `mkdocs/docs/admin/functions-reference.md`,
  `mkdocs/docs/admin/materialized-views.md`, `CHANGELOG.md`, `tasks/1768_view_sets_admin_ui_plan.md`

## Trade-offs

- **In-place toggle vs. a separate admin browser tab (option B).** A dedicated tab separates
  elevated and normal work more strongly, and could survive a reload. The new tab has to learn that
  it is elevated without a URL flag, because a shared `?admin=1` link would elevate whoever opens
  it. The remaining ways are an opener `postMessage` handshake, or `sessionStorage` copied across
  `window.open`, whose behaviour with `noopener` differs between browsers. The in-place toggle gets
  the same visual separation (red chrome) with React state alone. It is per-visit by construction
  and cannot be reached from a link.
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
  through through any expression or table-factor position the visitor misses. Substring matching
  can only over-block, and over-blocking now costs a click rather than "use the CLI".
- **Enforce in FlightSQL (a client-supplied "restricted" header that skips registering mutating
  functions and refuses DDL)** instead of in the web server. It is exact, but it touches the public
  FlightSQL service and every session build for a web-only concern. The web server is the only
  client that mixes untrusted content with an admin's token.
- **Admin pages use the admin endpoint for reads too** rather than per query: a single rule ("this
  page is admin") with nothing to get wrong page by page.

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
- Auto-execute (time range, selection, config edit, variable auto-run) is off while notebook admin
  mode is on; only Run, Run from here and Refresh execute (user call).

## Security

- **Boundary:** `/api/query-stream` refuses the mutating set whatever the client does. Elevation
  needs a request to `/api/admin/query-stream`, which only the app's own code issues, and only on
  an admin route or after the admin turns the notebook toggle on.
- **Threat closed:** a notebook, a shared link or notebook variables, written by anyone, running a
  mutating statement under an admin's identity without that admin choosing to.
- **Not in scope:** stolen sessions (see the re-authentication trade-off), and a malicious admin.
  FlightSQL's own admin gate still applies to everything.
- **Residual:** an admin who turns admin mode on in a notebook they haven't read is running that
  notebook's statements. The red chrome and the explicit re-run limit this, but don't prevent it.

## Documentation

- `mkdocs/docs/admin/web-app.md`: a new "Admin mode" section covering the two endpoints, implicit
  admin pages, the notebook toggle, what is refused, and the red chrome. Rewrite the Query Deny
  List paragraph at `:204-210`, since that limitation is gone. Document `POST
  /api/admin/query-stream` next to `/api/query-stream`.
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
- The refusal happens before data-source resolution, so a cache that would fail proves no
  FlightSQL connection was attempted.

**Rust** (`tests/routing_tests.rs`):

- `/api/admin/query-stream` returns 403 for a non-admin `ValidatedUser` and reaches the handler for
  an admin.
- It is mounted under `--disable-auth`.

These are silent-failure auth checks, so they are automated even though they are easy to try by
hand.

**Vitest:**

- `arrow-stream`: `adminMode` selects the URL for both `streamQuery` and `fetchQueryIPC`.
- `useCellExecution`: `adminMode` reaches every remote call path (stream, IPC, `runQueryAs`), and
  flipping it doesn't trigger execution.
- `QueryDenyListPage`: every query is sent with `adminMode: true`.
- `useCellExecution`: with admin mode on, a time-range change and a selection change don't
  execute, an explicit `executeCell` / `executeFromCell` / refresh trigger does, and turning the
  mode off doesn't run anything.
- `useNotebookAutoRun`: with admin mode on, `scheduleAutoRun` and `triggerAutoRun` don't execute.
- `ScreenPage`: with admin mode on, `useRefreshInterval` gets `0` and the stored interval is
  unchanged; turning it off restores the interval. The state resets when the screen changes; a saved config never contains it; a
  `?admin=…` URL param has no effect.
- `Header`: the identity menu offers *Enable admin mode* only to an admin on a notebook (not to a
  non-admin, not on other screen types or pages). It becomes *Disable admin mode* while on. The
  pill's *Turn off* clears it.
- `AppShell` / `Header`: the chrome appears on admin routes and with the toggle on, and not on
  `/admin` or `/admin/ingestion-keys`.

No live-DB test: no bug has occurred in the wild.

## Manual Verification

Against `local_test_env/ai_scripts/start_services.py --monolith`, with auth disabled so the user is
admin:

1. Create a notebook whose cell is `SELECT * FROM regenerate_partitions('log_stats', 'global', now()
   - INTERVAL '1 hour', now())`. Expected: the cell errors with the admin-mode hint.
2. Open the identity menu, choose **Enable admin mode**, and re-run the cell. Expected: red
   stripe, frame and pill appear, and the cell returns log rows.
3. Reload the page. Expected: admin mode is off and the cell is refused again.
4. On Admin → Query Deny List, add a rule with expression `sql LIKE '%retire_partitions%'`.
   Expected: the rule is created (it was refused before).

Not automated: these check the browser → web server → FlightSQL wiring and the look of the
chrome, which a mocked hook cannot show. Any breakage would be obvious the first time someone
tries it.

