# Admin Web UX for View Sets (Issue #1768) Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1768

## Overview

Add **Admin → View Sets** to the analytics web app: a page where an admin can list every view set,
read its definition, schema and partition status, and create, edit or drop SQL-defined
(`source = 'ddl'`) view sets, without hand-writing DDL into `micromegas-query` or running
`micromegas-views`. It is a front end over SQL the server already exposes. **No server change and
no new REST route**, following the Query Deny List page, which issues its admin UDTFs through the
`/api/admin/query-stream` path (`adminMode: true`).

## Current State

**Write path (exists).** `CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` are
intercepted in FlightSQL `do_get` (`rust/public/src/servers/flight_sql_service_impl.rs:919`),
admin-gated, validated (`build_sql_batch_view` + `validate_view_definition`), applied in one
transaction under an advisory lock, and checked for dependents (`check_dependents_survive`).
`execute_view_ddl` (`:1101`) returns one `(view_set_name, status)` row (`created`, `replaced`,
`dropped`, `not_found`); `not_found` is a row only for `DROP ... IF EXISTS`, while a plain `DROP`
of a missing name returns a `not_found` error. DDL is rejected when *prepared* (`:1673`), but the
web server's `FlightSQLClient::query_stream` (`rust/public/src/client/flightsql_client.rs:82`) uses
`execute` + `do_get`, so a DDL statement sent through `/api/admin/query-stream` reaches the DDL
branch. `substitute_macros` (`rust/analytics-web-srv/src/stream_query.rs:111`) only rewrites `$key` for
keys present in `params`; with empty params the DDL text passes through untouched.
`/api/query-stream` refuses view DDL and the lakehouse-modifying admin functions; the admin-only `/api/admin/query-stream` accepts them.

**Read path (exists).**
- `list_view_set_definitions()` (admin-gated) → `(view_set_name, definition_sql, update_group,
  updated_at, updated_by, source)`. Reads Postgres directly, so a definition that failed to load
  still appears; built-ins appear with `source = 'builtin'` and no `definition_sql`.
- `list_view_sets()` → `(view_set_name, current_schema_hash, schema, has_view_maker,
  global_instance_available, description, columns, sort_order)`. Only loaded view sets appear, so
  a `ddl` row present in the first and absent in this one is the "failed to load" state.
- `list_partitions()` → per-partition `num_rows`, `file_size`, `begin_insert_time`,
  `end_insert_time`, ... for the partition-status aggregates.
- `check_view_merge(ddl)` (admin-gated) → `streams`, `sort_exec_line` for a `CREATE` statement,
  without storing anything.

**Edit semantics (documented, `mkdocs/docs/admin/materialized-views.md:130-162`).** A replace that
changes the output schema self-invalidates old partitions (different schema hash). A content-only
replace does not rebuild anything: the admin must retire and rebuild the range. A `DROP` retires
the view's partitions and is refused if another definition reads the view.

**Front-end precedent.**
- `analytics-web-app/src/routes/QueryDenyListPage.tsx` + `src/lib/query-deny-list-api.ts`:
  SQL builders, Arrow decoders, `useStreamQuery`, `DataSourceField`, `ConfirmDialog`,
  `TableFrame`. Closest match.
- `AdminPage.tsx` card list (`adminOnly: true`), `router.tsx` lazy routes under `/admin/*`.
- `SyntaxEditor.tsx` / `QueryEditor.tsx` for SQL editing.
- `mkdocs/docs/admin/web-app.md` documents the admin pages.

**Today's workflow.** `micromegas-views` (`tasks/completed/view_set_cli_plan.md`) for
files-in-git; raw DDL through `micromegas-query` otherwise. The CLI stays the right tool for
repo-managed deployments; the page serves ad-hoc inspection and quick edits.

## Design

### 1. Page and routing

`/admin/view-sets` → `ViewSetsPage` (lazy, in `router.tsx`), an `AdminPage` card
(`adminOnly: true`, icon `Layers`). Data source chosen with the same `DataSourceField` +
`useDataSourceState` as the deny list, but placed in the left column (§2) rather than the page
header. The page is wrapped in `<AuthGuard requireAdmin>` like the deny list, so a non-admin sees
AuthGuard's "Access Denied" card rather than the raw planner error ("function not found") that
`list_view_set_definitions()` yields for non-admins. The server remains the authority, since the
functions and DDL are admin-gated there.

### 2. Layout: master/detail (see Mockups)

Left: the data source selector (`DataSourceField`) above the table of view sets (name,
`ddl`/`builtin` badge, health). Right: detail for the selected one
with tabs **Definition**, **Schema**, **Partitions**.

- **Health** is deliberately cheap, computed from data already loaded: `ddl` row absent from
  `list_view_sets()` → `failed to load`; otherwise `ok`; `builtin` → `read-only`. The streaming
  warning is **not** a list column: it costs a plan per view, so it is fetched only for the
  selected view, on the Definition tab.
- **Definition**: `definition_sql` in a read-only highlighted block, plus
  `check_view_merge(definition_sql)` result as a warning banner when `streams = false`
  (`sort_exec_line` shown). Built-ins have no stored definition; the tab says so.
- **Schema**: decode `list_view_sets().columns`/`sort_order` into a column table, plus the
  `description` rendered as markdown (it is a markdown usage guide). `columns` is a Utf8 of
  `name: DataType` lines joined by `\n`: split on `\n`, then on the first `": "`. `sort_order` is a
  nullable Utf8 (not a List): split on `", "`.
- **Partitions**: one aggregate row per view set, computed server-side by
  `SELECT view_set_name, count(*), sum(file_size), sum(num_rows), min(begin_insert_time),
  max(end_insert_time), max(updated) FROM list_partitions() GROUP BY view_set_name`.

### 3. Create / edit / drop

All three are single DDL statements, sent through `/api/admin/query-stream` (via
`adminMode: true`; `/api/query-stream` refuses DDL). The page issues every query with
`adminMode: true`, and adds `/admin/view-sets` to `ADMIN_ONLY_PATHS` in `src/lib/admin-mode.tsx` so
it gets the red admin chrome.

- **Create**: dialog with a name and a SQL editor pre-filled with a skeleton statement
  (`CREATE MATERIALIZED VIEW <name> WITH (...)` listing the required options). Sent as plain
  `CREATE`, so a name collision surfaces the server's `already_exists` error rather than silently
  replacing.
- **Edit**: the Definition tab switches to an editor on `definition_sql`. The name is not
  editable (a rename is create + drop, outside this scope). Save sends the text with its header
  rewritten to `CREATE OR REPLACE MATERIALIZED VIEW` by a small `toReplaceStatement(sql)`
  helper (strips leading comments, rewrites the header, mirrors the CLI's `with_or_replace`
  (`views.py:105-110`), including leaving an unterminated `/*` untouched). The user never has to remember `OR REPLACE`.
- **Save confirmation** (`ConfirmDialog`) states the documented consequences: a schema change
  invalidates old partitions; a content-only change does **not** rebuild them and requires a
  retire-and-rebuild by other means (link to docs).
- **Drop**: `ConfirmDialog` naming the view set and noting partitions are retired. Sent as
  `DROP MATERIALIZED VIEW <name>` (no `IF EXISTS`: a stale page should get a `not_found` error
  shown in the `ErrorBanner`, not a silent success).
- **Validation**: *Check* calls `check_view_merge('<ddl>')` and shows the streaming verdict.
  Everything else (parse errors, bad queries, dependents) is validated by the server on Save and
  shown verbatim in an `ErrorBanner` that keeps the dialog/editor open with the user's text
  intact. No dry-run DDL is added server-side.
- After any mutation, **refetch** the three list queries (never patch local state).

### 4. Files and shared code

- `src/lib/view-sets-api.ts`: SQL builders (`buildListViewSetDefinitionsSql`,
  `buildListViewSetsSql`, `buildPartitionStatsSql`, `buildCheckViewMergeSql`,
  `buildCreateSql`/`buildReplaceSql`/`buildDropSql`), Arrow decoders, `toReplaceStatement`,
  `mergeViewSetRows` (joins definitions + loaded sets + partition stats into the row model).
- `escapeSqlLiteral` currently lives in `query-deny-list-api.ts`; it is needed again for
  `check_view_merge('<ddl>')` and DDL contains quotes throughout. **Extract** it to a shared
  module (e.g. `src/lib/sql-literal.ts`) and import it from both, rather than duplicating.
- `src/routes/ViewSetsPage.tsx`, plus small components if the file grows
  (`ViewSetDetail`, `ViewSetEditor`).

### 5. Open questions from the issue, resolved

| Question | Resolution |
|---|---|
| Which properties are editable? | The whole stored statement, as text, except the name. No structured form: the statement carries several SQL queries and options, and the DDL parser is the validator. Built-ins are read-only. |
| Existing partitions on edit/delete? | Same as the DDL's documented behavior; the UI states it in the confirm dialog. It does not offer partition retirement. |
| How are backend errors surfaced? | Server message verbatim in an `ErrorBanner`; edits are never discarded on error. |

## Mockups

Open in a browser (self-contained, app tokens copied from `src/styles/globals.css`):

- `tasks/1768_view_sets_admin_ui_mockups/option-a-master-detail.html`: list left, detail with
  Definition/Schema/Partitions tabs right. One page, no extra routes, quick to hop between view
  sets; the editor is cramped on narrow screens. **Chosen**, with the data source selector in the
  left column above the list (the mockup shows this). Its list shows a
  "sort buffers" badge as an illustration; drop the badge when implementing.
- `tasks/1768_view_sets_admin_ui_mockups/option-b-table-and-editor.html` (not chosen)

## Implementation Steps

1. Extract `escapeSqlLiteral` to `src/lib/sql-literal.ts`; update `query-deny-list-api.ts` and its
   test import.
2. `src/lib/view-sets-api.ts` with builders, decoders, `toReplaceStatement`, `mergeViewSetRows`;
   unit tests alongside the existing `src/lib/__tests__/` files.
3. `ViewSetsPage.tsx`: list + detail tabs (`AuthGuard requireAdmin`, loading/error states).
4. Create dialog, edit mode, drop confirm, Check button; refetch after mutations.
5. Route in `router.tsx`, card in `AdminPage.tsx`, `/admin/view-sets` in `ADMIN_ONLY_PATHS`
   (`src/lib/admin-mode.tsx`), and 'View Sets' in `adminOnlyTitles` in
   `src/routes/__tests__/AdminPage.test.tsx` ("ten cards" becomes eleven).
6. Docs and `CHANGELOG.md`.

## Files to Modify

- New: `analytics-web-app/src/lib/sql-literal.ts`, `src/lib/view-sets-api.ts`,
  `src/routes/ViewSetsPage.tsx`, tests under `src/lib/__tests__/` and `src/routes/__tests__/`
- Edit: `src/lib/query-deny-list-api.ts`, `src/router.tsx`, `src/routes/AdminPage.tsx`,
  `src/lib/admin-mode.tsx`, `src/routes/__tests__/AdminPage.test.tsx`,
  `mkdocs/docs/admin/web-app.md` (admin-hub card count/list, ~lines 107/110),
  `mkdocs/docs/admin/materialized-views.md` (pointer), `mkdocs/docs/admin/views-as-code.md`
  (pointer), `CHANGELOG.md`
- Paths without a prefix are under `analytics-web-app/`.

## Trade-offs

- **SQL through the existing statement path vs. new REST routes.** REST would allow server-side
  dry-run and typed errors but duplicates the admin gate, the advisory-lock/transaction logic and
  the dependency check already in `execute_view_ddl`. Reusing the SQL surface adds no server
  code and matches the deny list.
- **Raw SQL editor vs. structured form.** A form can only cover the option subset it models and
  must regenerate DDL; the raw statement round-trips exactly (`definition_sql` is verbatim).

## Decisions

- No `.sql` download: `micromegas-views pull <name>` already writes a definition in the CLI file layout.
- No partition retire/rebuild action: leaving out retire/rebuild is a scope choice.
- Layout: option A (master/detail), data source selector in the left column.
- Rename is out of scope.
- Streaming warning is fetched for the selected view only, never per list row.

## Documentation

- `mkdocs/docs/admin/web-app.md`: add the View Sets page next to the other admin pages.
- `mkdocs/docs/admin/materialized-views.md` and `views-as-code.md`: one line each noting the page
  as an alternative to raw DDL / the CLI.
- `CHANGELOG.md`: entry for the new page (no Rust API change, so no breaking-change clause).

## Testing Strategy

All automated coverage is no-DB unit tests (vitest), per the repo testing rules:

- `view-sets-api`: every SQL builder (quote escaping with DDL containing `'` and `$$`), Arrow
  decoders (nullable `definition_sql`, `description`; `columns` split on `\n` then first `": "`;
  null and multi-item `sort_order` split on `", "`), `toReplaceStatement` (plain `CREATE`,
  already `OR REPLACE`, leading `--` and `/* */` comments, unterminated `/*` left untouched,
  lowercase keywords),
  `mergeViewSetRows` (ddl present/absent from loaded sets → `failed to load`; builtin;
  missing partition stats).
- `ViewSetsPage`: non-admin sees AuthGuard's "Access Denied" card and issues no query; error from
  a mutation keeps the editor text and shows the server message; confirm dialogs gate Save and
  Drop, and the Save dialog shows the content-only warning; refetch after a successful mutation.
  Mocks `streamQuery` from `@/lib/arrow-stream`, as `QueryDenyListPage.test.tsx` does.
- No live-DB test: no bug has occurred, and the DDL semantics are already covered server-side.

## Manual Verification

Against `local_test_env/ai_scripts/start_services.py --monolith`, with auth disabled so the user
is admin:

1. Open `/admin/view-sets`; confirm built-ins show read-only and `log_stats` shows as `ddl`.
2. Create a view set from the skeleton; confirm it appears and `list_view_sets()` agrees.
3. Edit it to change only a filter and save; confirm the change round-trips end to end.
4. Drop it; confirm it disappears. Try dropping a view another definition reads; confirm the
   server's refusal shows in the banner.

Not automated: it checks the browser → web server → FlightSQL wiring (that DDL really passes
the `/api/admin/query-stream` path), which a unit test against a mocked hook cannot show.
