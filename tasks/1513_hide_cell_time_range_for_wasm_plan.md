# Hide the Per-Cell Query Time Range for Notebook (WASM) Cells Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1513

## Overview

A notebook cell's per-cell **Query Time Range** override (`timeRange: { from, to }`, added in #1314)
has no automatic effect when the cell's effective data source is `notebook` — i.e. when the cell is
executed locally by the WASM DataFusion engine. Server-side views know their own time column, so a
`begin`/`end` forwarded to `fetchQueryIPC`/`executeSql` prunes partitions and bounds the query without
the cell author doing anything. Tables registered into the WASM engine are plain Arrow batches with no
"this is the time column" metadata, so there is no column-agnostic place to apply the same bound.

Per the issue's decision, the fix has two parts. `shouldShowTimeRange` stops deciding from
`cell.type` alone and also accounts for the *resolved* data source (`resolveCellDataSource`), so the
**Query Time Range** field disappears from the cell editor for cells that run locally. And because
hiding the field alone leaves a saved override live and invisible — see below — `resolveQueryTimeRange`
itself is gated on the same resolved data source, so the override has no effect at execution, at
render, or in macro substitution either. The guiding principle for both parts: **a cell whose resolved
data source is `notebook` has no per-cell time range — its effective range is always the screen's
global range.**

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
- `notebook-cell-view.ts:216` — `buildCellRendererProps` resolves the same override for *every* cell,
  with no data-source check, and passes it as the `timeRange` prop. That prop draws the displayed
  Swimlane/PropertyTimeline axis and bounds Map playback, and feeds render-time `$from`/`$to`
  substitution in Table/TransposedTable link templates and Chart/PieChart unit/label text.
- `PerfettoExportCell.tsx:382` — the Perfetto cell's own resolution.
- `useCellExecution.ts:156-171` — a fourth, independent consumer that reads the override *before*
  `resolveQueryTimeRange` is even called: it pulls `cellTimeRange` straight off the cell config and
  feeds `cellTimeRange.from`/`.to` into `findUnresolvedSelectionMacro`, alongside the cell's SQL. If
  either bound references a `$cell.selected.column` macro whose row isn't selected yet, the cell is
  set to `status: 'blocked'` and execution halts for every downstream cell. This is deliberate, tested
  behaviour (`__tests__/useCellExecution.test.ts:858`, "blocks when the timeRange override references
  an unresolved row selection") and documented at `mkdocs/docs/web-app/notebooks/variables.md:120`.

None of these paths reach a server for a `notebook`-source cell — there is no server query to bound —
but all four are live today regardless: the first still moves `$from`/`$to` inside the cell's own
locally-run SQL, the second still redraws a display-axis cell's axis/playback window and render-time
templates, the third resolves independently of whatever the editor shows, and the fourth can halt the
whole notebook run over a field the editor no longer shows. That is the bug this plan fixes, not a
capability worth preserving: the override's shadowing in `resolveQueryTimeRange` doesn't add a filter
on top of the global range, it *replaces* what `$from`/`$to` mean inside that cell's SQL — so a saved
override on a `notebook`-source cell makes the screen's own global range unreachable from that cell's
macros, with (once the field is hidden) no UI left to see or clear it. The Design section below closes
all four paths at their common root (gating on the resolved data source before `resolveQueryTimeRange`
and before the unresolved-selection check) rather than leaving them live and merely un-editable.

### Horizontal-group children

HG children are edited by `ChildEditorView` (`HorizontalGroupCell.tsx:278-393`), which renders name +
`DataSourceField` + the type editor — it has **no** `CellTimeRangeField` today (`CellEditor.tsx` is
`CellTimeRangeField`'s only render site). There is still no field to hide for children, so the
visibility half of this plan has nothing to do there.

The enforcement half, however, does reach them. `NotebookRenderer.tsx:333` flattens HG children into
`cells` via `flattenCellsForExecution` before handing them to the single `useCellExecution` run, so a
child's saved `timeRange` override is resolved by the same `resolveQueryTimeRange` call
(`useCellExecution.ts:197`) and the same unresolved-selection check (`useCellExecution.ts:156-171`) as
any top-level cell — gated on the child's own resolved data source exactly like a top-level cell's. At
render, `HorizontalGroupCell.tsx:192` builds each child's renderer props via `buildCellRendererProps`,
passing `dataSource: resolveCellDataSource(child, variables, defaultDataSource)` — already the child's
*resolved* source, the same shape `notebook-cell-view.ts:216` gates on. So a JSON-set override on a
`notebook`-source HG child is honoured today, at both execution and render, and will be ignored after
this change too — consistent with the plan's invariant, not an exception to it. Both paths already
pass a resolved data source, so this needs no extra implementation step; it's a behaviour note, not new
scope. Open Question 1 (whether children should gain the field at all) is unaffected and left as-is.

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

| Cell's data source | Field | Effective range at execution / render / macros |
|---|---|---|
| unset (→ notebook default, always remote) | shown | override (if set), else global |
| an explicit remote data source | shown | override (if set), else global |
| `notebook` | **hidden** | **always global** — override ignored |
| `$var` resolving to `notebook` | **hidden** | **always global** — override ignored |
| `$var` resolving to a remote source, or unset/empty (→ notebook default) | shown | override (if set), else global |

The field reappears the moment the user switches the cell back to a remote data source — the config
value is never mutated by this change, so an override set before the switch is honoured again, and
its effective range reverts to the global one only for as long as the cell stays `notebook`-sourced.

### Execution/render-time enforcement — gate `resolveQueryTimeRange` on the resolved data source

Hiding the field is not sufficient on its own: `resolveQueryTimeRange` currently shadows the global
range for *any* cell with a saved override, with no data-source check, so a `notebook`-source cell
whose override was set before this change (or edited directly in the screen JSON) keeps having its
SQL's `$from`/`$to`, its Swimlane/PropertyTimeline axis, its Map playback window, and its
Table/Chart/PieChart render-time macros all resolve against the override instead of the screen's
global range — invisibly, since the field that would show or clear it is now hidden. That is a bug
(the screen's own global range becomes unreachable from that cell), not a capability worth preserving.

A fourth consumer sits outside `resolveQueryTimeRange` entirely and has to be gated separately:
`useCellExecution.ts:156-171` reads `cellTimeRange.from`/`.to` off the cell config directly — before
`resolveQueryTimeRange` is even called — and feeds them into `findUnresolvedSelectionMacro`. For a
`notebook`-source cell whose (now-ignored, now-hidden) override references an unresolved row selection
(e.g. `{ from: '$Processes.selected.start_time' }`), this still sets the cell to `status: 'blocked'`
and halts every downstream cell, even though the override's value no longer affects anything once
resolved. This check has to move below `resolveCellDataSource(...)` and skip its two
`cellTimeRange`-derived clauses when the resolved source is `notebook` — the `cellSql` clause stays
unconditional, since a notebook cell's own SQL can legitimately reference a selection macro.

There are exactly three production call sites of `resolveQueryTimeRange`: `useCellExecution.ts:197`,
`notebook-cell-view.ts:216`, and `PerfettoExportCell.tsx:382`. Rather than repeat a data-source check
at each one (and risk them drifting), the gate lives once inside `resolveQueryTimeRange` itself, via a
new required field on its `MacroCtx` parameter (`notebook-utils.ts:305`):

```ts
interface MacroCtx {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  /** The cell's already-resolved data source (`resolveCellDataSource`'s result). Required so every
   * call site is forced to supply it — each of the three already computes or has access to this
   * value, and a caller that doesn't know it can't correctly decide whether to shadow the range. */
  cellDataSource: string | undefined
}

export function resolveQueryTimeRange(
  config: CellConfig,
  ctx: MacroCtx,
): { begin: string; end: string } {
  if (ctx.cellDataSource === 'notebook') return ctx.timeRange
  const raw = 'timeRange' in config ? (config as QueryBackedCellConfig).timeRange : undefined
  // ...unchanged from here
}
```

Required, not optional, for the same reason `shouldShowTimeRange`'s new parameters are required above:
`MacroCtx` is built as an object literal at all three call sites, so the compiler enumerates every one
of them rather than letting a fourth, future call site silently default to "always honour the
override." `CellExecutionContext` (`cell-registry.ts:67-75`) gets the same field, required, for the
same reason — see `PerfettoExportCell.tsx` below.

What each call site supplies:

- **`useCellExecution.ts:197`** — already computes `resolveCellDataSource(...)` for `isNotebookSource`,
  but at line 205, *after* both the unresolved-selection check at lines 156-171 and the
  `resolveQueryTimeRange` call at line 197. Move that computation above the unresolved-selection
  check (not merely above line 197), and:
  - skip the `cellTimeRange?.from`/`cellTimeRange?.to` clauses of that check when
    `cellDataSource === 'notebook'` (keep the `cellSql` clause unconditional — a notebook cell's SQL
    can still legitimately reference a selection macro);
  - pass `cellDataSource` into both the `MacroCtx` literal and the `CellExecutionContext` literal
    built a few lines below (which downstream `execute` functions, including Perfetto's, receive).
- **`notebook-cell-view.ts:216`** — no new lookup needed. `buildCellRendererProps`'s `context.dataSource`
  is *already* the cell's resolved data source, not the notebook-level default: `NotebookRenderer.tsx`
  computes `resolveCellDataSource(cell, availableVariables, dataSource)` once at line 633 and passes
  that result as `dataSource: cellDataSource` into the `CellViewContext` literal at line 646 — the same
  field `buildCellRendererProps` already forwards unchanged as `CellRendererProps.dataSource` (line
  246). So this site passes `context.dataSource` straight through as `cellDataSource`. (Calling
  `resolveCellDataSource` a second time here, as if `context.dataSource` were the notebook-level
  default, would in fact be harmless — the function is idempotent on an already-resolved value — but
  it's an unnecessary second call and a new import; passing the value straight through is simpler.)
- **`PerfettoExportCell.tsx:382`** — its `execute` receives only `CellExecutionContext`, which has no
  data-source field today. Add `cellDataSource: string | undefined`, required, to the interface
  (`cell-registry.ts:67-75`); `useCellExecution.ts` is the only production site that constructs this
  interface as an object literal (verified — every other reference destructures it as a parameter
  type), so this is a one-site change there, using the same value the previous bullet already computes.

Test files that build `CellExecutionContext`-shaped literals to call a cell type's `execute` directly
(e.g. `PerfettoExportCell.test.tsx`, `VariableCell.test.tsx`) are not affected by making the field
required: `tsconfig.json` excludes `*.test.ts(x)` and `__tests__` from type-checking, so those literals
aren't compiler-checked, and an omitted field reads as `undefined` at runtime — which is `!== 'notebook'`
and so behaves exactly as before. No changes needed there.

### Chart cells

`chart` keeps the existing `shouldShowDataSource` carve-out semantics: v2 chart configs hold
`dataSource` per query (`ChartCell.tsx:451`) and have no cell-level field, so
`resolveCellDataSource` falls back to the (remote) notebook default and the range field stays
visible — correct whenever any series is remote, a harmless leftover control when every series is
local. A legacy v1 chart config that still carries a top-level `dataSource: 'notebook'` hides it,
which is right for that single-query shape. Inspecting per-query sources would mean teaching
`notebook-utils` about chart internals (and its v1→v2 migration); not worth it for a leftover field.

## Implementation Steps

1. **`analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`** —
   a. Widen `shouldShowTimeRange` to `(cell, variables, notebookDataSource)` and return `false` when
      `resolveCellDataSource(...) === 'notebook'`. Update the doc comment to name the reason and #1513.
   b. Add the required `cellDataSource: string | undefined` field to `MacroCtx` (line 305), and make
      `resolveQueryTimeRange` return `ctx.timeRange` unchanged when `ctx.cellDataSource === 'notebook'`.
   `resolveCellDataSource` is defined in the same module — no new imports for (a).
2. **`analytics-web-app/src/lib/screen-renderers/cell-registry.ts`** — add the required
   `cellDataSource: string | undefined` field to `CellExecutionContext` (lines 67-75).
3. **`analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`** — move the existing
   `resolveCellDataSource(...)`/`isNotebookSource` computation (currently at line 205) above the
   unresolved-selection check (lines 156-171), not merely above the `resolveQueryTimeRange` call
   (line 197). Skip that check's `cellTimeRange?.from`/`cellTimeRange?.to` clauses when
   `cellDataSource === 'notebook'` (leave the `cellSql` clause unconditional), and pass
   `cellDataSource` into both the `resolveQueryTimeRange` call's `MacroCtx` literal and the
   `CellExecutionContext` literal built below it.
4. **`analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts:216`** — pass `context.dataSource`
   (already the cell's resolved data source) as `cellDataSource` in the `resolveQueryTimeRange` call's
   `MacroCtx` literal.
5. **`analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx:382`** — pass
   `context.cellDataSource` as `cellDataSource` in its `resolveQueryTimeRange` call's `MacroCtx` literal.
6. **`analytics-web-app/src/components/CellEditor.tsx:90`** — pass `variables` and `defaultDataSource`
   to `shouldShowTimeRange`.
7. **`analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts`** — update both the
   `shouldShowTimeRange` describe block (line ~1303, new signature, notebook-source cases) and the
   `resolveQueryTimeRange` describe block (line ~1234: add `cellDataSource` to `baseCtx`, plus cases
   proving a `notebook` data source returns the global range even with an override set, and a remote
   data source still honours the override).
8. **`analytics-web-app/src/lib/screen-renderers/__tests__/useCellExecution.test.ts`** — add a case
   next to the existing "blocks when the timeRange override references an unresolved row selection"
   test (line 858): the same unresolved-selection-in-`timeRange` setup, but on a `notebook`-source
   cell and with `engine: createMockEngine()` supplied (the helper already defined at line 77 and used
   for exactly this purpose at lines 1334, 1369, 1432, 1466), asserting the cell's status reaches
   `'success'` — proving the unresolved-selection check's `cellTimeRange` clauses are actually skipped
   for that data source and the cell runs end-to-end, not merely that it avoids `'blocked'`.
9. **`analytics-web-app/src/lib/screen-renderers/__tests__/notebook-cell-view.test.ts`** — add a case
   to the existing `describe('per-cell timeRange override')` block (line ~396): using the `makeContext`
   helper's `dataSource` override, prove `buildCellRendererProps` with `dataSource: 'notebook'` and a
   cell carrying a `timeRange` override returns the global range for `result.timeRange` (the override
   is ignored), while the existing remote-source case alongside it keeps proving the override is
   honoured.
10. **Docs** — `mkdocs/docs/web-app/notebooks/variables.md` "Per-Cell Query Time Range" section: note
   that for cells whose data source resolves to `notebook`, the field is hidden *and* any existing
   override is ignored entirely — `$from`/`$to` and the display axis follow the screen's global range —
   and why. Also qualify the two bullets this change invalidates: **Errors** (`variables.md:119`),
   which no longer applies since a `notebook`-source override never reaches `parseRelativeTime`, and
   **Waiting for selection** (`variables.md:120`), which no longer applies since the unresolved-selection
   check skips the `cellTimeRange` clauses for a `notebook`-source cell — both need a "does not apply
   when the cell's data source resolves to `notebook`" qualifier. Amend the bullet saying the field is
   shown "for every cell type that supports it" (`variables.md:121`) to note it is not shown when the
   cell's data source resolves to `notebook`. Also update
   `mkdocs/docs/web-app/notebooks/cell-types.md:7`, which currently points every query-backed cell at
   the shared `timeRange` field, and `cell-types.md:114`, which tells the Flame Graph cell's author to
   "use the cell-level `timeRange` field instead" to change what the cell's SQL fetches — both with the
   same `notebook`-source qualifier.
11. **`CHANGELOG.md`** — one bullet under `## Unreleased` → `**Web App:**` describing both the hidden
   field and the behaviour change for cells already carrying a saved override.
12. Run `yarn lint`, `yarn tsc --noEmit` (or the repo's typecheck script), and `yarn test` in
    `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/PerfettoExportCell.tsx`
- `analytics-web-app/src/components/CellEditor.tsx`
- `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/__tests__/useCellExecution.test.ts`
- `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-cell-view.test.ts`
- `mkdocs/docs/web-app/notebooks/variables.md`
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `CHANGELOG.md`

## Trade-offs

- **Hide vs. show-disabled vs. keep-and-annotate.** Annotating the field ("for notebook cells this
  only sets `$from`/`$to`") was considered and rejected: that use is redundant — the same expression
  can be written straight into the local SQL — so the annotation would document a feature with no
  reason to exist. A disabled-but-visible field keeps a stale override readable but uncleatable, and
  adds a prop to `CellTimeRangeField` for an advisory control. Hiding matches how
  `shouldShowDataSource` already removes irrelevant fields rather than greying them out.
- **UI-only vs. also ignoring the override at execution.** Rejected. An earlier draft of this plan
  stayed UI-only, leaving `resolveQueryTimeRange` unchanged, on the theory that a `notebook`-source
  cell's residual paths (moving `$from`/`$to` in its own SQL; narrowing a display-axis cell's axis or
  playback window) were harmless leftovers reachable only by editing the screen JSON. That
  understatement missed what the shadowing actually does: it doesn't add a filter on top of the global
  range, it *replaces* what `$from`/`$to` mean inside the cell's SQL — so a saved override makes the
  screen's own global range unreachable from that cell's macros, with no UI left to see or clear it.
  That is a bug, not a preserved capability, so this plan takes the second option: `resolveQueryTimeRange`
  now ignores the override whenever `cellDataSource === 'notebook'`, making "hidden" and "no effect"
  identical everywhere the override is consumed.

  This is a deliberate, silent behaviour change on any already-saved screen: a `notebook`-source cell
  that hand-wrote `WHERE t BETWEEN '$from' AND '$to'` against an override, or a Swimlane/PropertyTimeline/
  Map cell whose axis or playback window an override narrowed, will render against the wider global
  range the next time the screen loads — no migration path is provided, because the prior behaviour is
  the thing being fixed, not a feature being deprecated. The hand-written-`WHERE`-against-an-override
  path is judged effectively unused in practice (the same bound can be written directly into the SQL,
  without the override, to the same effect), so this risk is treated as hypothetical rather than as a
  reason to keep the bug.
- **Not attempting to make the override work for WASM.** Enforcing a range locally would require a
  per-table time-column declaration (registered alongside `engine.register_table`) plus a rewrite that
  injects the bound — and even then it could only narrow within whatever range the *upstream* remote
  fetch already used, so a widened override would still return nothing new. That's a much larger
  feature; the issue explicitly rules it out for now.
- **Signature widening over an options object.** Two extra positional params for a one-call-site
  helper; an options bag would be more churn than the change is worth.

## Documentation

- `mkdocs/docs/web-app/notebooks/variables.md` — "Per-Cell Query Time Range": add a bullet that for
  cells whose data source resolves to `notebook`, the field is not offered *and* any override is
  ignored entirely — `$from`/`$to` macros, the display axis, and playback all follow the screen's
  global range instead, because a WASM-registered table has no designated time column to bound. Use
  SQL (`WHERE`) or narrow the upstream cell's range instead. Amend the existing bullet that says the
  field is shown "for every cell type that supports it" (`variables.md:121`). Also qualify the **Errors** bullet
  (`variables.md:119`) and the **Waiting for selection** bullet (`variables.md:120`) as not applying
  to a cell whose data source resolves to `notebook`: the former because the early return in
  `resolveQueryTimeRange` never reaches `parseRelativeTime`, so an unparseable override on such a cell
  no longer surfaces as an error; the latter because the unresolved-selection check now skips the
  `cellTimeRange` clauses for that data source, so the cell is no longer blocked on it.
- `mkdocs/docs/web-app/notebooks/cell-types.md:7` — the sentence pointing every query-backed cell at
  the shared `timeRange` field gets the same "ignored for `notebook`-source cells" qualifier.
- `mkdocs/docs/web-app/notebooks/cell-types.md:114` — the Flame Graph cell's note that "to change what
  data the cell's SQL fetches, use the cell-level `timeRange` field instead" gets the same "does not
  apply when the cell's data source resolves to `notebook`" qualifier.
  (`execution.md` is left as-is: its "Local WASM Query Engine" section already doesn't mention the
  per-cell override, so there's nothing there to correct.)
- `CHANGELOG.md` under `## Unreleased` → `**Web App:**`: hide the per-cell **Query Time Range** field,
  and ignore any saved override, for cells whose data source resolves to `notebook` (local WASM) — the
  screen's global range now applies there unconditionally, since there's no designated time column on
  a WASM-registered table to enforce a per-cell override against (#1513).

## Testing Strategy

Unit tests in `notebook-utils.test.ts`, across both affected describe blocks, plus one case each in
`useCellExecution.test.ts` and `notebook-cell-view.test.ts` for the two call sites whose correctness
isn't covered by the `notebook-utils.ts` unit tests alone — keep them proportional to the change; no
new CellEditor or renderer render test is warranted for a visibility/gating flag:

**`shouldShowTimeRange`:**

- Existing type-based cases, updated for the new signature (`{}` variables, a remote default) — they
  must still pass unchanged in outcome.
- `dataSource: 'notebook'` on a query cell → `false`.
- `dataSource: '$ds'` with `variables: { ds: 'notebook' }` → `false`; with `{ ds: 'remote' }` → `true`.
- `dataSource: '$ds'` with the variable missing/empty → falls back to the notebook default → `true`.
- A combobox variable cell with `dataSource: 'notebook'` → `false` (it was `true` before).
- `markdown`/`referencetable`/`hg` stay `false` regardless of data source.

**`resolveQueryTimeRange`:**

- Add `cellDataSource` to the existing `baseCtx` literal (a non-`'notebook'` value, e.g. `'remote-src'`)
  so all existing cases keep exercising the "remote" path explicitly rather than relying on an implicit
  `undefined`.
- `cellDataSource: 'notebook'` with a `timeRange` override set on the cell → returns `ctx.timeRange`
  (the global range) unchanged, proving the override is ignored, not just narrowed.
- `cellDataSource` set to a remote source with the same override → still resolves the override, exactly
  as today — proving the new gate is additive for remote-sourced cells.

**`useCellExecution` (`useCellExecution.test.ts`):**

- Next to the existing "blocks when the timeRange override references an unresolved row selection"
  test (line 858): the same setup (a `timeRange` override referencing `$Processes.selected.*` with no
  row selected), but with the downstream cell's data source resolving to `notebook` and an `engine:
  createMockEngine()` supplied → assert `status` reaches `'success'`, proving the unresolved-selection
  check skips the `cellTimeRange` clauses for a notebook-source cell and the cell actually executes
  against the screen's global range — not merely that it avoids `'blocked'` — while still leaving the
  `cellSql` clause (and the remote-source case) live.

**`buildCellRendererProps` (`notebook-cell-view.test.ts`):**

- In the existing `describe('per-cell timeRange override')` block: `makeContext({ dataSource:
  'notebook' })` plus a cell carrying a `timeRange` override → `result.timeRange` equals the global
  range (`context.timeRange`), not the override — proving the render call site's `context.dataSource`
  wiring (`NotebookRenderer.tsx:646`, `HorizontalGroupCell.tsx:192`) actually reaches the gate. The
  existing remote-source case in the same block continues to prove the override is honoured there.

Manual check: open a notebook, add a table cell with a **Query Time Range** override set, confirm it
narrows the result; switch the cell's data source to **Notebook**, confirm the field disappears *and*
the cell now runs and (for a display-axis cell) renders against the screen's global range, not the
override; switch back to a remote source, confirm the field reappears with the override intact and
back in effect.

## Open Questions

1. Should HG children gain the **Query Time Range** field at all? They don't have it today, so #1513
   doesn't apply to them — but the gap is worth its own issue if per-cell ranges are meant to work
   inside groups.

**Settled:** whether a `notebook`-source cell should also *ignore* a previously saved `timeRange` at
execution/render time — yes. An earlier draft of this plan answered no and kept execution/rendering
untouched, on the theory that the residual `$from`/`$to`-substitution and axis/playback-narrowing paths
were harmless leftovers. That was wrong: the override doesn't add a filter, it *replaces* what
`$from`/`$to` mean inside the cell's SQL, so leaving it live made the screen's own global range
unreachable from that cell's macros with no UI left to see or clear it — a bug. `resolveQueryTimeRange`
is now gated on the resolved data source (see Design), so a `notebook`-source cell's effective range is
always the screen's global range, at execution, at render, and in macros, matching what the hidden
field already implies.
