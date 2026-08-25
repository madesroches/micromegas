# Hide the Per-Cell Query Time Range for Notebook (WASM) Cells Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1513

## Overview

A notebook cell's per-cell **Query Time Range** override (`timeRange: { from, to }`, added in #1314)
has no automatic effect when the cell's effective data source is `notebook` — i.e. when the cell is
executed locally by the WASM DataFusion engine. Server-side views know their own time column, so a
`begin`/`end` forwarded to `fetchQueryIPC`/`executeSql` prunes partitions and bounds the query without
the cell author doing anything. Tables registered into the WASM engine are plain Arrow batches with no
"this is the time column" metadata, so there is no column-agnostic place to apply the same bound.

Per the issue's decision, the fix is UI-side and small: `shouldShowTimeRange` stops deciding from
`cell.type` alone and also accounts for the *resolved* data source (`resolveCellDataSource`), so the
**Query Time Range** field disappears from the cell editor for cells that run locally. No execution
semantics change.

## Current State

### Visibility gate — `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts:343`

```ts
export function shouldShowTimeRange(cell: CellConfig): boolean {
  switch (cell.type) {
    case 'markdown':
    case 'referencetable':
    case 'hg':
      return false
    case 'variable':
      return cell.variableType === 'combobox' || cell.variableType === 'expression'
    default:
      return true // table, chart, log, propertytimeline, swimlane, transposed, flamegraph, map, perfettoexport, image, piechart
  }
}
```

Type-only. It has one production caller: `CellEditor.tsx:90` (`const showTimeRange = shouldShowTimeRange(cell)`),
which gates the `<CellTimeRangeField>` block at `CellEditor.tsx:148-153`.

### Data-source resolution — `notebook-utils.ts:288`

```ts
export function resolveCellDataSource(
  cell: CellConfig,
  variables: Record<string, VariableValue>,
  notebookDataSource: string | undefined,
): string | undefined
```

Cell-level `dataSource` → `$variable` substitution → notebook default. This is exactly what
`useCellExecution.ts:205` uses to decide `isNotebookSource`, and what `NotebookRenderer.tsx:633` and
`HorizontalGroupCell.tsx:192` use to label/route cells. Reusing it is what keeps the editor's
visibility rule in lockstep with execution.

### What the editor already has in hand

`CellEditor` already receives both inputs the resolution needs — no new props:

| Prop | Passed from | Same value execution uses? |
|---|---|---|
| `variables` | `NotebookRenderer.tsx:827` — `getAvailableVariables(selectedCellIndex)` | Yes — `useCellExecution` resolves against the same available-variable set |
| `defaultDataSource` | `NotebookRenderer.tsx:834` — the notebook-level `dataSource` | Yes — `useCellExecution` passes the same `dataSource` as the fallback |

The notebook-level default comes from `useDefaultDataSource()` (`routes/ScreenPage.tsx:40`) and is
always a real (remote) data source — `notebook` is only ever selectable per-cell
(`DataSourceSelector.tsx:71`, gated on `showNotebookOption`, passed by `CellEditor`/`VariableCell`).

### Where the override still gets consumed

- `useCellExecution.ts:197` — `resolveQueryTimeRange(cell, …)` shadows the global range so both the
  server query window (`params.begin/end`) **and** `$from`/`$to` macro substitution pick it up.
- `notebook-cell-view.ts:216` — display-axis resolution for Swimlane / PropertyTimeline / Map.
- `PerfettoExportCell.tsx:382` — the Perfetto cell's own resolution.

For a `notebook`-source cell the first of these is the only live path, and only through macro
substitution: the SQL string a WASM cell runs has `$from`/`$to` replaced by the resolved override, so
an override on a WASM cell is not *strictly* inert — it moves `$from`/`$to` if the cell author wrote
`WHERE t BETWEEN '$from' AND '$to'` by hand. That residual path buys nothing, though: whatever
expression the author would type into the cell's `from`/`to` they can type directly into the local
SQL instead, and the override still can't reach rows the upstream remote fetch never pulled. What's
absent is the automatic enforcement the server gives every remote view for free, and there's no
capability to preserve — hence hiding rather than annotating the field.

### Horizontal-group children

HG children are edited by `ChildEditorView` (`HorizontalGroupCell.tsx:278-393`), which renders name +
`DataSourceField` + the type editor — it has **no** `CellTimeRangeField` today (`CellEditor.tsx` is
`CellTimeRangeField`'s only render site). Nothing to hide there; out of scope.

## Design

Widen `shouldShowTimeRange` to take the resolution context, and route it through
`resolveCellDataSource`:

```ts
/**
 * Returns true if a cell should show the per-cell query time range field.
 *
 * Cells that resolve to the `notebook` (local WASM) data source never show it:
 * a WASM-registered table is plain Arrow batches with no designated time column,
 * so there is no column-agnostic way to enforce a begin/end the way a server-side
 * view does. Hiding it keeps the editor from offering a control that can't be
 * honoured (#1513).
 *
 * Distinct from `shouldShowDataSource`, which excludes `chart` for per-query
 * data-source reasons that don't apply to the cell-level time window.
 */
export function shouldShowTimeRange(
  cell: CellConfig,
  variables: Record<string, VariableValue>,
  notebookDataSource: string | undefined,
): boolean {
  switch (cell.type) {
    case 'markdown':
    case 'referencetable':
    case 'hg':
      return false
    case 'variable':
      if (cell.variableType !== 'combobox' && cell.variableType !== 'expression') return false
      break
  }
  return resolveCellDataSource(cell, variables, notebookDataSource) !== 'notebook'
}
```

Required (not optional) parameters, so the compiler enumerates every call site — per `CLAUDE.md`'s
Rust-API stance, which the web app follows in spirit: a silently defaulted `variables: {}` would make
a `$variable`-driven data source resolve differently in the editor than at execution.

Caller, `CellEditor.tsx:90`:

```ts
const showTimeRange = shouldShowTimeRange(cell, variables, defaultDataSource)
```

### Behaviour after the change

| Cell's data source | Field |
|---|---|
| unset (→ notebook default, always remote) | shown |
| an explicit remote data source | shown |
| `notebook` | **hidden** |
| `$var` resolving to `notebook` | **hidden** |
| `$var` resolving to a remote source, or unset/empty (→ notebook default) | shown |

The field reappears the moment the user switches the cell back to a remote data source — the config
value is never mutated by this change, so an override set before the switch survives round-tripping.

### Chart cells

`chart` keeps the existing `shouldShowDataSource` carve-out semantics: v2 chart configs hold
`dataSource` per query (`ChartCell.tsx:451`) and have no cell-level field, so
`resolveCellDataSource` falls back to the (remote) notebook default and the range field stays
visible — correct whenever any series is remote, a harmless leftover control when every series is
local. A legacy v1 chart config that still carries a top-level `dataSource: 'notebook'` hides it,
which is right for that single-query shape. Inspecting per-query sources would mean teaching
`notebook-utils` about chart internals (and its v1→v2 migration); not worth it for a leftover field.

## Implementation Steps

1. **`analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`** — widen `shouldShowTimeRange`
   to `(cell, variables, notebookDataSource)` and return `false` when
   `resolveCellDataSource(...) === 'notebook'`. Update the doc comment to name the reason and #1513.
   `resolveCellDataSource` is defined in the same module — no new imports.
2. **`analytics-web-app/src/components/CellEditor.tsx:90`** — pass `variables` and `defaultDataSource`.
3. **`analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts:1303`** — update the
   existing `shouldShowTimeRange` describe block for the new signature and add the notebook-source cases.
4. **Docs** — `mkdocs/docs/web-app/notebooks/variables.md` "Per-Cell Query Time Range" section: note
   that the field is hidden (and the override not enforced) for cells whose data source resolves to
   `notebook`, and why.
5. **`CHANGELOG.md`** — one bullet under `## Unreleased` → `**Web App:**`.
6. Run `yarn lint`, `yarn tsc --noEmit` (or the repo's typecheck script), and `yarn test` in
   `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/components/CellEditor.tsx`
- `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts`
- `mkdocs/docs/web-app/notebooks/variables.md`
- `CHANGELOG.md`

## Trade-offs

- **Hide vs. show-disabled vs. keep-and-annotate.** Annotating the field ("for notebook cells this
  only sets `$from`/`$to`") was considered and rejected: that use is redundant — the same expression
  can be written straight into the local SQL — so the annotation would document a feature with no
  reason to exist. A disabled-but-visible field keeps a stale override readable but uncleatable, and
  adds a prop to `CellTimeRangeField` for an advisory control. Hiding matches how
  `shouldShowDataSource` already removes irrelevant fields rather than greying them out.
- **UI-only vs. also ignoring the override at execution.** This plan does **not** change
  `resolveQueryTimeRange` or `useCellExecution`. A cell that already has an override saved and is
  later switched to `notebook` keeps feeding that range into `$from`/`$to` for its locally-executed
  SQL — an invisible-but-live effect, the one wart of the UI-only approach. Forcing the global range
  for notebook-source cells would make "hidden" and "no effect" identical, at the cost of a silent
  behaviour change on saved configs that hand-wrote `WHERE t BETWEEN '$from' AND '$to'` against a
  local table. Keeping execution untouched makes this a pure visibility change with no migration
  risk; the residual path stays reachable only by editing the screen JSON.
- **Not attempting to make the override work for WASM.** Enforcing a range locally would require a
  per-table time-column declaration (registered alongside `engine.register_table`) plus a rewrite that
  injects the bound — and even then it could only narrow within whatever range the *upstream* remote
  fetch already used, so a widened override would still return nothing new. That's a much larger
  feature; the issue explicitly rules it out for now.
- **Signature widening over an options object.** Two extra positional params for a one-call-site
  helper; an options bag would be more churn than the change is worth.

## Documentation

- `mkdocs/docs/web-app/notebooks/variables.md` — "Per-Cell Query Time Range": add a bullet that the
  field is not offered for cells whose data source resolves to `notebook`, because a WASM-registered
  table has no designated time column to bound; use SQL (`WHERE`) or narrow the upstream cell's range
  instead. Amend the existing bullet that says the field is shown "for every cell type that supports it".
- `mkdocs/docs/web-app/notebooks/cell-types.md:7` — the sentence pointing every query-backed cell at
  the shared `timeRange` field should get the same "not for `notebook`-source cells" qualifier.
- `mkdocs/docs/web-app/notebooks/execution.md` — optional: a line in "Local WASM Query Engine" noting
  local queries are bounded by whatever range the upstream fetch used, not by a per-cell override.
- `CHANGELOG.md` under `## Unreleased` → `**Web App:**`: hide the per-cell **Query Time Range** field
  for cells whose data source resolves to `notebook` (local WASM), since there's no designated time
  column on a WASM-registered table to enforce it against (#1513).

## Testing Strategy

Unit tests in `notebook-utils.test.ts` (`shouldShowTimeRange` describe block) — keep them proportional
to the change; no new CellEditor render test is warranted for a visibility flag:

- Existing type-based cases, updated for the new signature (`{}` variables, a remote default) — they
  must still pass unchanged in outcome.
- `dataSource: 'notebook'` on a query cell → `false`.
- `dataSource: '$ds'` with `variables: { ds: 'notebook' }` → `false`; with `{ ds: 'remote' }` → `true`.
- `dataSource: '$ds'` with the variable missing/empty → falls back to the notebook default → `true`.
- A combobox variable cell with `dataSource: 'notebook'` → `false` (it was `true` before).
- `markdown`/`referencetable`/`hg` stay `false` regardless of data source.

Manual check: open a notebook, add a table cell, confirm **Query Time Range** is present; switch its
data source to **Notebook**, confirm the field disappears; switch back, confirm it returns with any
previously entered value intact.

## Open Questions

1. Should HG children gain the **Query Time Range** field at all? They don't have it today, so #1513
   doesn't apply to them — but the gap is worth its own issue if per-cell ranges are meant to work
   inside groups.

**Settled:** whether a `notebook`-source cell should also *ignore* a previously saved `timeRange` at
execution — no. Hiding the control is the fix; execution stays as-is (see Trade-offs). The
`$from`/`$to`-only path it leaves behind is redundant with writing the expression directly in the
local SQL, so nothing is lost by making it unreachable from the UI.
