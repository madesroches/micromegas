# Notebook `$me.*` Viewer Identity Macro Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1617

## Overview

Notebook SQL has no way to refer to the person viewing the screen, so a shared "my usage"
notebook needs either one copy per person or a text variable the viewer fills in by hand. This
plan adds a built-in `$me` namespace, filled from the web app's auth context: `$me.email`,
`$me.name`, and `$me.sub` (the OIDC subject claim, i.e. the identity provider's stable user id).
Values get the same single-quote escaping as every other SQL macro. `me` becomes a reserved
variable name. On a server started with `--disable-auth`, `$me.*` is reported as unresolved
instead of being replaced with placeholder values. `$me` is a convenience for scoping a shared
screen, not an authorization boundary. Audience-based read filtering stays the enforcement layer.

## Current State

- **Macro engines.** `analytics-web-app/src/lib/screen-renderers/macro-substitution.ts` handles SQL
  (`substituteMacros`, `substituteMacrosRaw`, `validateMacros`), and `template-evaluator.ts`
  handles markdown/templates. Both send every value lookup through `resolveMacro` in
  `macro-resolve.ts`. `$variable.col` (`kind: 'varCol'`) resolves against
  `ctx.variables[name][col]` and returns `UNRESOLVED` when the variable or column is missing.
  An unresolved `$var.col` stays in the SQL as source text, so the query fails loudly. It does
  not silently become an empty string.
- **Where the variables map comes from.** Cells only ever see a `Record<string, VariableValue>`.
  Two sites build it from the variable cells above the current cell:
  - `NotebookRenderer.tsx:526` `getAvailableVariables(index)`: rendering, editor panels, and
    validation.
  - `useCellExecution.ts:142`: the loop that builds `availableVariables` for execution.
  ~50 call sites (`substituteMacros`/`validateMacros` in every cell type, `notebook-utils.ts:338`,
  `components/map/overlay.ts:451`, `resolveCellDataSource`) receive this map, so anything injected
  into it is visible to every macro shape and every cell type.
- **Identity.** `src/lib/auth.tsx` exposes `useAuth()` → `{ user: { sub, email?, name?, is_admin? } }`,
  loaded from `GET {base}/auth/me`. `AuthGuard` makes sure it has loaded before any screen renders.
  `useAuth` throws outside an `AuthProvider`, and `NotebookRenderer.test.tsx` renders without one.
- **Auth disabled.** `rust/analytics-web-srv/src/web_server.rs:680` `auth_me_no_auth` returns a
  synthetic user (`sub: "anonymous"`, `email: "anonymous@localhost"`, `name: "Anonymous (No Auth)"`,
  `is_admin: true`) through `NoAuthUserInfo` (`web_server.rs:673`). The frontend can't tell this
  apart from a real user.
- **Reserved names.** `validateCellName` (`notebook-utils.ts:218`) already rejects variable names in
  `RESERVED_URL_PARAMS` (`from`, `to`, `type`). `validateMacros` skips `from`/`to`/`order_by`
  in its simple-variable pass.
- **Docs.** `mkdocs/docs/web-app/notebooks/variables.md` § "SQL Macro Substitution" has the macro
  syntax table.

## Design

### Approach: inject a reserved multi-column variable

Treat the viewer as one more entry in the variables map: `variables.me = { email, name, sub }`.
It is added at the two sites that build the map, not in the macro engines. Because `$me.email`
is an ordinary `varCol` lookup, it immediately works in SQL, markdown, chart labels/units, map
detail templates, and `validateMacros`, with the same escaping and no change to `resolveMacro`,
either parser, or any cell's call site.

The entry is **not** written into `variableValues` (the URL-synced state in
`useNotebookVariables`), so it is never persisted to the URL or saved config, and a `?me=` URL
param is ignored the same way as any param that isn't a variable cell.

### New helpers (`notebook-utils.ts`)

```ts
export const VIEWER_VARIABLE_NAME = 'me'

/** Viewer identity as a multi-column variable; undefined when there is no real viewer. */
export function viewerVariable(user: User | null): Record<string, string> | undefined
```

- Returns `undefined` when `user` is null or `user.auth_disabled` is true.
- Only includes the claims that are present and non-null (`user.email != null`). The IdP claims
  come back from `/auth/me` as JSON `null`, not absent keys, so a missing `email` or `name` leaves
  that key out. `sub` is always present.

```ts
/** Variables visible to a cell: the viewer entry, then upstream variable cells. */
export function collectAvailableVariables(
  cells: CellConfig[],               // cells above the target
  values: Record<string, VariableValue>,
  viewer: Record<string, string> | undefined,
): Record<string, VariableValue>
```

`NotebookRenderer.getAvailableVariables` and the variables part of the `useCellExecution` loop
both call this, so the render path and the execution path can't disagree about what `$me` means.
Internally it walks `cells` via `forEachCell`, not a plain top-level loop, so it also picks up
variable cells nested inside a horizontal group; that matters because the two callers pass
different shapes (`NotebookRenderer` passes the raw top-level `cells.slice(0, index)`, while
`useCellExecution` passes its already-flattened `executionCells`), and `forEachCell` handles both.
The viewer entry goes in **first**, so a legacy variable cell named `me` overwrites it (see
Conflicts).

### Supplying the viewer

- `auth.tsx`: add `auth_disabled?: boolean` to `User`, type `email`/`name` as
  `string | null | undefined` (the IdP claim can come back as JSON `null`), and add
  `export function useOptionalAuthUser(): User | null`, which reads the context directly and returns
  `null` outside a provider instead of throwing. That keeps `NotebookRenderer` usable in tests and in
  any embedding without an `AuthProvider`.
- `NotebookRenderer`: `const user = useOptionalAuthUser()` then
  `const viewer = useMemo(() => viewerVariable(user), [user])`, and pass `viewer` into
  `collectAvailableVariables` and into `useCellExecution({ ..., viewer })`.
- `useCellExecution`: new `viewer` option. The execute path reads it from a ref (same pattern as
  `variableValuesRef`), so a mid-session auth refresh doesn't change `executeCell`'s identity.

### Auth-disabled servers

`NoAuthUserInfo` gets `auth_disabled: bool`, always `true` in `auth_me_no_auth`. The real
`UserInfo` doesn't change: the field is simply absent, and the frontend treats absent as false.
This is an additive JSON field on an internal web endpoint and not part of the SQL surface.
The existing header display ("Anonymous (No Auth)") keeps working unchanged.

With no viewer entry, `$me.email` is unresolved: it stays in the SQL (the query errors), and the
editor's validation reports it. `validateMacros` gets one targeted message: when the dotted or
simple pass hits `me` and `variables.me` is undefined, it emits
`$me is unavailable: no signed-in viewer` instead of
`Unknown variable: me`.

### Conflicts with a variable cell named `me`

- **New or renamed cells:** `validateCellName(..., isVariable=true)` rejects
  `VIEWER_VARIABLE_NAME` with `"me" is reserved for the signed-in viewer`. Both call sites need the
  `isVariable` flag: `components/CellEditor.tsx:69` already passes it, and the horizontal-group
  child rename path (`cells/HorizontalGroupCell.tsx:312`) must pass
  `child.type === 'variable'` for it, since a variable cell renamed inside a group currently
  bypasses the check.
- **Saved notebooks that already have a `me` variable cell:** the cell keeps precedence, and
  the variable cell's editor (`VariableCell.tsx`, next to its
  validation errors) shows a warning:
  `"me" is reserved for the signed-in viewer; this variable hides $me.email / $me.name / $me.sub. Rename it.`

### Bare `$me`

A bare `$me` resolves like any multi-column variable (the sorted-key JSON dump from
`getVariableString`). No special case. The docs point to the dotted form.

## Implementation Steps

1. **Server flag.** `rust/analytics-web-srv/src/web_server.rs`: add `auth_disabled: bool` to
   `NoAuthUserInfo`, set it to `true` in `auth_me_no_auth`. Extend the existing
   `auth_me_route_still_works_with_observability_layer` test in
   `rust/analytics-web-srv/tests/routing_tests.rs` (or add a sibling test) to assert that the
   body has `auth_disabled: true`.
2. **Auth context.** `analytics-web-app/src/lib/auth.tsx`: add `auth_disabled?: boolean` to `User`,
   retype `email`/`name` as `string | null | undefined`, and add `useOptionalAuthUser()`.
3. **Helpers.** `notebook-utils.ts`: add `VIEWER_VARIABLE_NAME`, `viewerVariable`,
   `collectAvailableVariables`, and the reserved-name check in `validateCellName`. Pass
   `child.type === 'variable'` as `isVariable` at the `validateCellName` call in
   `cells/HorizontalGroupCell.tsx:312`, so a variable cell renamed inside a horizontal group is
   also rejected.
4. **Render path.** `NotebookRenderer.tsx`: compute `viewer` and switch `getAvailableVariables` to
   `collectAvailableVariables`.
5. **Execution path.** `useCellExecution.ts`: accept `viewer`, keep it in a ref, and build
   `availableVariables` with `collectAvailableVariables` (results/selections stay in the existing
   loop).
6. **Validation message.** `macro-substitution.ts` `validateMacros`: add the `me`-unavailable
   message in the dotted and simple passes.
7. **Conflict warning.** `cells/VariableCell.tsx` editor: add the reserved-name warning when
   `varConfig.name` sanitizes to `me`.
8. **Docs.** See Documentation.

## Files to Modify

- `rust/analytics-web-srv/src/web_server.rs`
- `rust/analytics-web-srv/tests/routing_tests.rs`
- `analytics-web-app/src/lib/auth.tsx`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`
- `analytics-web-app/src/lib/screen-renderers/macro-substitution.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`
- tests under `analytics-web-app/src/lib/screen-renderers/__tests__/`
- `mkdocs/docs/web-app/notebooks/variables.md`

## Trade-offs

- **Reserved variable vs. a new `MacroSpan` kind.** A `{ kind: 'viewer' }` span with a `viewer`
  field on `ResolveCtx` would need a new regex pass in both engines, a new validation branch, and
  a new argument threaded through `substituteMacros`/`validateMacros` at ~50 call sites. Injecting
  a variable reuses `varCol` end to end and touches only the two map builders. The cost is that
  `me` shows up anywhere variable names are listed, which is accurate: it *is* available there.
- **Explicit `auth_disabled` flag vs. detecting `sub === "anonymous"`.** Matching the placeholder
  string would couple the frontend to a literal a real IdP could in principle issue. A flag makes
  the server state the fact directly.
- **Omitting missing claims vs. substituting `''`.** An empty email in `WHERE email = '$me.email'`
  silently returns no rows, and with `LIKE`/`<>` it can return everyone's rows. Leaving the macro
  unresolved fails loudly, which matches the issue's reasoning for the auth-disabled case.
- **Legacy `me` cell wins vs. built-in wins.** Letting the built-in win would silently change
  results for any saved notebook that already has a `me` variable. Letting the cell win and
  warning keeps those notebooks working and still tells the author to rename.
- **Server-side UDF (`current_user_email()`).** Out of scope, per the issue's follow-up note. It
  would cover CLI/Python clients but needs Flight SQL caller plumbing.

## Documentation

`mkdocs/docs/web-app/notebooks/variables.md`:
- Add `$me.email`, `$me.name`, `$me.sub` rows to the Syntax table.
- Add a short "Viewer identity" subsection with:
  - an example (`WHERE username = '$me.email'`);
  - that `sub` is the IdP's stable subject id;
  - that `me` is a reserved variable name;
  - that `$me.*` is unresolved on auth-disabled servers and when the IdP omits a claim;
  - that it is **not an authorization mechanism**, since viewers can edit a notebook's SQL, and
    audience-based read filtering is the enforcement boundary.
- Variable Scope: note that `$me` is always available regardless of cell position, and appears
  in the Available Variables panel like any other variable.
- Reserved Parameters: note that `me` is also rejected as a variable name, reserved for the
  viewer rather than a URL param.

## Testing Strategy

All unit tests (vitest / cargo test), with no DB:

- `notebook-utils.test.ts`:
  - `viewerVariable`: full user → three keys in `email, name, sub` order; `email: null`/`name: null`
    (the missing-claim shape `/auth/me` actually sends) → those keys absent;
    `auth_disabled: true` → `undefined`; `null` → `undefined`.
  - `collectAvailableVariables`: viewer present; only upstream variable cells included; a legacy
    `me` cell overrides the viewer.
  - `validateCellName`: `me` rejected for variables and allowed for non-variable cells.
  - `validateMacros` → the auth-disabled message when `me` is absent.
- `useCellExecution.test.ts`: executing a cell whose SQL contains `$me.email` with a `viewer`
  option sends the substituted SQL. This is the execution-path wiring that a render-only test
  wouldn't reach.
- `NotebookRenderer.test.tsx`: renders without an `AuthProvider` (existing tests keep passing,
  which covers `useOptionalAuthUser`'s no-provider branch).
- `routing_tests.rs`: `--disable-auth` `/auth/me` body includes `auth_disabled: true`.

## Manual Verification

Checked by hand because it needs a real OIDC login, which a unit test can't reach:

1. Start the monolith with auth enabled and open a notebook with a table cell
   `SELECT '$me.email' AS email, '$me.name' AS name, '$me.sub' AS sub`. Expected: one row with the
   signed-in account's values, matching the header's user menu.
2. Restart with `--disable-auth` and reload the same notebook. Expected: the cell shows the
   `$me is unavailable` validation message, and the query fails instead of returning
   `anonymous@localhost`.
