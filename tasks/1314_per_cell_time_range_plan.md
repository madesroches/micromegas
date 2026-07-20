# Per-Cell Query Time Range Plan

## Overview
Give every **query-backed** notebook cell an optional per-cell time range override. Today each cell inherits the screen's single global (URL-synced) time range and passes it unmodified into its SQL and server query window. This adds an optional `timeRange: { from, to }` config field (raw range strings resolved through the existing macro engine) so a cell can pin a fixed relative/absolute range, or derive its range from a variable, an upstream cell result, or a row/drag selection — while defaulting to the global range when unset.

Tracks issue #1314. Supersedes #1310 (which asked for this on the Perfetto export cell only). The design reuses the macro-resolution machinery that already backs the Flame Graph cell's `initialFrom`/`initialTo`, so no changes to the macro/variable engine are required.

## Current State

### Config types — `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
`CellConfigBase` (`name`, `type`, `layout`, `autoRunFromHere`) is the only shared base. `dataSource?: string` is duplicated independently on four configs:
- `QueryCellConfig` (covers `table | chart | log | propertytimeline | swimlane | transposed | flamegraph | map`) — also has `sql`, `options`.
- `VariableCellConfig` — has `variableType` (`combobox | text | expression | datasource`), optional `sql`, `dataSource`.
- `PerfettoExportCellConfig` — has `processIdVar`, `spanType`, `dataSource`.
- `ImageCellConfig` — has `sql`, `dataSource`.

Non-query configs (out of scope): `MarkdownCellConfig`, `ReferenceTableCellConfig`, `HorizontalGroupCellConfig`.

### Time range types — `analytics-web-app/src/lib/time-range.ts`
- `TimeRange { from: string; to: string }` — raw, unparsed range strings (config/URL layer). This is exactly the shape the new field needs.
- Runtime range is a resolved `{ begin: string; end: string }` (ISO), produced by `getTimeRangeForApi(from, to)` and fed to the API.
- `parseRelativeTime(value)` handles `now`, `now-1h`, or ISO strings → `Date`.

### How time range flows today
1. **Execution engine** — `useCellExecution.ts:177` resolves the global raw range once per run: `const timeRange = getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to)`. It builds a per-cell `CellExecutionContext` (`useCellExecution.ts:187`) carrying `variables`, `cellResults`, `cellSelections`, `timeRange`, `runQuery`, `runQueryAs`, then calls `meta.execute(cell, context)`. `runQuery`/`runQueryAs` pass `timeRange.begin/end` both as macro context **and** as the actual server query window (`params: { begin, end }`, top-level `begin`/`end`).
2. **Each cell's `execute`** does `substituteMacros(sql, variables, timeRange, cellResults, cellSelections)` then `runQuery(sql)` — e.g. `TableCell.tsx:308`, `LogCell.tsx:382`, `SwimlaneCell.tsx:549`, `PropertyTimelineCell.tsx:207`, `TransposedTableCell.tsx:223`, `MapCell.tsx:1031`, `ImageCell.tsx:277`, `VariableCell.tsx:406`, `ChartCell.tsx` (per-series). In SQL, `$from`/`$to` resolve from the `timeRange` argument (`macro-substitution.ts:104-105`).
3. **Renderer props** — `notebook-cell-view.ts:buildCellRendererProps` sets `timeRange: context.timeRange` on every `CellRendererProps`. Some cells read this prop directly (not via `execute`):
   - **`PerfettoExportCell`** has **no `execute`** (`PerfettoExportCell.tsx:367` comment). The trace is fetched on button click in the renderer using the `timeRange` prop (`fetchPerfettoTrace({ timeRange })`).
   - **`SwimlaneCell` / `PropertyTimelineCell`** use the `timeRange` prop to draw the visible-window time axis.
   - **`MapCell`** uses the `timeRange` prop for playback bounds.

### Precedent — Flame Graph `initialFrom`/`initialTo`
`FlameGraphCell.tsx:resolveInitialTimeRange` reads `options.initialFrom`/`initialTo`, runs each through `substituteMacros(str, variables, context.timeRange, cellResults, cellSelections)`, then `parseRelativeTime(resolved).getTime()`, collecting errors. This is stored in `state.meta` by `execute` and surfaced via `getRendererProps`. This is the exact resolution recipe reused here — but it sets the **initial view range** of an interactive cell, a distinct concept from a per-cell **query** range, so the two remain separate config fields.

### Editor UI — `analytics-web-app/src/components/CellEditor.tsx`
Renders a shared `DataSourceField` centrally (`CellEditor.tsx:131`), gated by `shouldShowDataSource(cell.type)` (`notebook-utils.ts`), then delegates to the type-specific `meta.EditorComponent`. Edits flow through `onUpdate(updates: Partial<CellConfig>)`, applied by `useCellManager.ts:168` as a **shallow** merge `{ ...cell, ...updates }`.

## Design

Two central integration points cover both consumption paths, plus one shared resolver, one shared config mixin, and one shared editor field. No per-cell `execute` edits.

### 1. Config: shared query-backed mixin
In `notebook-types.ts`, introduce a mixin and have the four query-backed configs extend it (dropping their standalone `dataSource`):

```ts
import { TimeRange } from '@/lib/time-range' // { from: string; to: string }

/** Fields shared by cells that run a query. */
export interface QueryBackedCellConfig {
  dataSource?: string
  /** Optional per-cell query time range override (raw strings; macro-resolved). Defaults to the screen's global range when unset. */
  timeRange?: TimeRange
}

export interface QueryCellConfig extends CellConfigBase, QueryBackedCellConfig {
  type: 'table' | 'chart' | 'log' | 'propertytimeline' | 'swimlane' | 'transposed' | 'flamegraph' | 'map'
  sql: string
  options?: Record<string, unknown>
}

export interface VariableCellConfig extends CellConfigBase, QueryBackedCellConfig {
  type: 'variable'
  variableType: 'combobox' | 'text' | 'expression' | 'datasource'
  sql?: string
  defaultValue?: VariableValue
  expression?: string
}

export interface PerfettoExportCellConfig extends CellConfigBase, QueryBackedCellConfig {
  type: 'perfettoexport'
  processIdVar?: string
  spanType?: 'thread' | 'async' | 'both'
}

export interface ImageCellConfig extends CellConfigBase, QueryBackedCellConfig {
  type: 'image'
  sql: string
}
```

`timeRange` is simply unused by text/expression variable cells. The field is optional and nested, so existing saved screen configs load unchanged (backward compatible).

### 2. Shared resolver
Add `resolveQueryTimeRange` to `notebook-utils.ts` (or a small sibling `cell-time-range.ts` re-exported from `notebook-utils.ts`). It mirrors `resolveInitialTimeRange` but returns a resolved runtime range and falls back to the global range per-bound:

```ts
import { parseRelativeTime } from '@/lib/time-range'
import { substituteMacrosRaw } from './macro-substitution'

interface MacroCtx {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }   // global range = fallback + base for $from/$to
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
}

/** Resolve a cell's optional timeRange override to a runtime { begin, end }.
 *  Each empty/unset bound falls back to the global range. Throws on an
 *  unparseable bound (caller decides whether to error the cell or fall back). */
export function resolveQueryTimeRange(
  config: CellConfig,
  ctx: MacroCtx,
): { begin: string; end: string } {
  const raw = ('timeRange' in config ? (config as QueryBackedCellConfig).timeRange : undefined)
  const fromStr = raw?.from?.trim() || ''
  const toStr = raw?.to?.trim() || ''
  if (!fromStr && !toStr) return ctx.timeRange

  const resolveBound = (s: string, fallback: string): string => {
    if (!s) return fallback
    const substituted = substituteMacrosRaw(s, ctx.variables, ctx.timeRange, ctx.cellResults, ctx.cellSelections)
    return parseRelativeTime(substituted).toISOString() // throws on invalid
  }

  return {
    begin: resolveBound(fromStr, ctx.timeRange.begin),
    end: resolveBound(toStr, ctx.timeRange.end),
  }
}
```

Notes:
- Use `substituteMacrosRaw` (no SQL quote-doubling) since the value is parsed as a time string, not embedded in SQL.
- The base `timeRange` passed in is the **global** range, so `$from`/`$to` *inside* a cell's override refer to the screen range (e.g. `from: "$from", to: "$cell.selected.end"`).
- `parseRelativeTime` already accepts relative (`now-1h`), ISO absolute, and — after substitution — macro-derived timestamps.

### 3. Execute-side integration (covers all cells with an `execute`)

**Blocked-state guard extension.** The existing unresolved-selection guard (`useCellExecution.ts:150-163`, `findUnresolvedSelectionMacro(cellSql, availableCellSelections)`) only scans `cell.sql`. A cell whose SQL has no selection macro but whose `timeRange.from`/`to` uses `$cell.selected.col` with no row selected would otherwise reach `resolveQueryTimeRange` and hit `parseRelativeTime('')`, throwing a raw parse error instead of the friendly blocked placeholder. Extend the guard to also scan the cell's `timeRange.from`/`to` strings with the same `findUnresolvedSelectionMacro` (it matches `$cell.selected.col` in any string, not just SQL) before computing the effective range:

```ts
const cellTimeRange = ('timeRange' in cell ? (cell as QueryBackedCellConfig).timeRange : undefined)
const unresolvedCell =
  (cellSql && findUnresolvedSelectionMacro(cellSql, availableCellSelections)) ||
  (cellTimeRange?.from && findUnresolvedSelectionMacro(cellTimeRange.from, availableCellSelections)) ||
  (cellTimeRange?.to && findUnresolvedSelectionMacro(cellTimeRange.to, availableCellSelections))
if (unresolvedCell) {
  completeCellExecution(cell.name, { status: 'blocked', data: [], error: `Select a row in "${unresolvedCell}" to view results` })
  return false // halt execution — downstream cells should wait for selection
}
```

**Resolving the effective range.** `runQuery`/`runQueryAs`/`executeSql` close over the local `timeRange` const computed at `useCellExecution.ts:177` for their `params: { begin, end }` server query window — they do not read `context.timeRange`. So the effective range must replace that local `timeRange` binding itself, not just `context.timeRange`, or the override would only affect `$from`/`$to` macro substitution while the server still queried the global window. Rename the global range at `:177` to `globalTimeRange`, then — inside the existing `try` that guards `meta.execute`, before the `runQuery`/`runQueryAs` closures are defined — shadow it with the resolved effective range under the original name `timeRange`:

```ts
const globalTimeRange = getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to) // was `timeRange` at :177

try {
  const timeRange = resolveQueryTimeRange(cell, {
    variables: availableVariables,
    timeRange: globalTimeRange,
    cellResults: availableCellResults,
    cellSelections: availableCellSelections,
  })
  // `timeRange` now shadows `globalTimeRange` for the rest of the `try` block, so
  // `runQuery`/`runQueryAs`/`executeSql` (params.begin/end) and `context.timeRange`
  // (substituteMacros $from/$to) both pick up the resolved effective range automatically —
  // no separate reassignment needed.
  ...
}
```

Because `runQuery`/`runQueryAs`/`executeSql` and `context.timeRange` all resolve `timeRange` from the same shadowed binding, this applies the per-cell range to every query-backed cell uniformly — table, chart, log, property timeline, swimlane, transposed, flamegraph, map, image, and SQL-backed variable cells — with **zero changes to any cell's `execute`**. A thrown parse error becomes the cell's `error` state (same treatment as a bad query), consistent with how Flame Graph surfaces `initialTimeRange` errors.

### 4. Renderer-side integration (covers Perfetto and display-axis cells)
In `notebook-cell-view.ts:buildCellRendererProps`, replace `timeRange: context.timeRange` with the resolved effective range so renderers that read the prop directly (Perfetto trace fetch, Swimlane/PropertyTimeline axis, Map playback) honor the override:

```ts
let effectiveTimeRange = context.timeRange
try {
  effectiveTimeRange = resolveQueryTimeRange(cell, {
    variables: context.availableVariables,
    timeRange: context.timeRange,
    cellResults: context.cellResults,
    cellSelections: context.cellSelections,
  })
} catch {
  // render must not throw; fall back to the global range (the cell's execute
  // path, if any, already surfaces the parse error as cell state)
}
// ...timeRange: effectiveTimeRange
```

This is the path that actually powers `PerfettoExportCell` (it has no `execute`). Re-resolution happens naturally on re-render when an upstream variable/selection changes.

### 5. Editor UI: shared time range field
Add `shouldShowTimeRange(cell)` to `notebook-utils.ts` (distinct from `shouldShowDataSource`, which excludes `chart` for per-query reasons that don't apply to the cell-level time window):

```ts
export function shouldShowTimeRange(cell: CellConfig): boolean {
  switch (cell.type) {
    case 'markdown':
    case 'referencetable':
    case 'hg':
      return false
    case 'variable':
      return cell.variableType === 'combobox' || cell.variableType === 'datasource'
    default:
      return true // table, chart, log, propertytimeline, swimlane, transposed, flamegraph, map, perfettoexport, image
  }
}
```

Add a shared presentational component `CellTimeRangeField` (`analytics-web-app/src/components/CellTimeRangeField.tsx`) with two labeled text inputs (From / To), styled like Flame Graph's Initial From/To inputs, with placeholders such as `$from, now-1h, or macro (empty = screen range)` / `$to, now, or macro`. Validation matches that same Flame Graph precedent: no live inline validation in the field itself — a bad value surfaces as a cell error at run/render time (§3/§4), consistent with `resolveInitialTimeRange`. Live validation is possible future work, not part of this change. Render it in `CellEditor.tsx` directly below the `DataSourceField`, gated by `shouldShowTimeRange(cell)`:

```tsx
{shouldShowTimeRange(cell) && (
  <CellTimeRangeField
    value={('timeRange' in cell ? cell.timeRange : undefined)}
    onChange={(tr) => onUpdate({ timeRange: tr } as Partial<CellConfig>)}
    variables={variables}
    cellResults={cellResults}
    cellSelections={cellSelections}
  />
)}
```

Because `updateCell` merges shallowly, `onChange` must emit the **full** `{ from, to }` object on each keystroke (read current bound, set the changed one). Emit `undefined` when both bounds are empty so a cleared override doesn't persist an empty object in the saved config. Flame Graph keeps its own `Initial From`/`Initial To` inside its `EditorComponent` (view-range concept) — it is unaffected and now also gains the shared query time range field.

### Data flow (after change)

```
global raw range (URL) ──> getTimeRangeForApi ──> global {begin,end}
                                                       │
     cell.timeRange {from,to} (raw, may contain macros)│
                    │                                   │
                    └──> resolveQueryTimeRange(cell, ctx: {vars, global range, cellResults, cellSelections})
                                   │
                    ┌──────────────┴───────────────┐
          execute path                       renderer path
   (useCellExecution: override         (buildCellRendererProps:
    context.timeRange)                  override timeRange prop)
          │                                     │
   substituteMacros($from/$to)          Perfetto fetch / axis / playback
   + server query window
```

## Implementation Steps

1. **Config types** (`notebook-types.ts`): add `QueryBackedCellConfig` mixin; extend it from `QueryCellConfig`, `VariableCellConfig`, `PerfettoExportCellConfig`, `ImageCellConfig`; remove their standalone `dataSource`. Import `TimeRange` from `@/lib/time-range`.
2. **Resolver** (`notebook-utils.ts` or new `cell-time-range.ts`): add and export `resolveQueryTimeRange`; add and export `shouldShowTimeRange`.
3. **Execute-side** (`useCellExecution.ts`): rename the global range binding at `:177` from `timeRange` to `globalTimeRange`; inside the existing `try` that guards `meta.execute`, redeclare `const timeRange = resolveQueryTimeRange(cell, { variables: availableVariables, timeRange: globalTimeRange, cellResults: availableCellResults, cellSelections: availableCellSelections })`, shadowing the outer binding so `runQuery`/`runQueryAs`/`executeSql` (`params.begin/end`) and `context.timeRange` both pick up the resolved effective range; parse errors thrown inside the `try` become cell errors.
4. **Renderer-side** (`notebook-cell-view.ts`): resolve effective range in `buildCellRendererProps` with try/catch fallback; set the `timeRange` prop from it.
5. **Editor field** (new `CellTimeRangeField.tsx` + `CellEditor.tsx`): shared From/To component rendered below the data source field, gated by `shouldShowTimeRange`; emit full `{from,to}` or `undefined`.
6. **Tests**: unit-test `resolveQueryTimeRange`; extend cell/execution tests (see Testing Strategy).
7. **Docs**: update `mkdocs/docs/web-app/notebooks/variables.md` and `cell-types.md`; retire or generalize `tasks/1314_per_cell_time_range_mockup.html`.

## Files to Modify
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts` — mixin + config extends.
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` — `resolveQueryTimeRange`, `shouldShowTimeRange` (or a new `cell-time-range.ts` re-exported here).
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — override `context.timeRange`.
- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` — override `timeRange` prop in `buildCellRendererProps`.
- `analytics-web-app/src/components/CellEditor.tsx` — render shared field.
- `analytics-web-app/src/components/CellTimeRangeField.tsx` — **new** shared editor component.
- Tests under `analytics-web-app/src/lib/screen-renderers/__tests__/` and `cells/__tests__/` (see below).
- Docs in `mkdocs/docs/` (query/notebook guide).

## Trade-offs
- **Central resolution vs. per-cell edits.** Resolving in `useCellExecution` (execute) + `buildCellRendererProps` (render) touches two files instead of editing ~9 `execute` methods. It is DRY and guarantees uniform behavior — but only because the execute-side fix shadows the local `timeRange` binding that `runQuery`/`runQueryAs`/`executeSql` close over (see §3), so the *server query window* (`params.begin/end`) is overridden along with `$from`/`$to` macros; overriding `context.timeRange` alone would miss the server window. Cost: two integration points must stay in sync; the shared resolver keeps them consistent.
- **Reuse `TimeRange`/`from`/`to` vs. a new type.** Per the issue, `TimeRange { from, to }` already models raw range strings and avoids the reserved SQL words `begin`/`end`; the resolved `{begin,end}` stays the runtime form. No new type invented.
- **Mixin vs. folding fields into `CellConfigBase`.** A mixin keeps non-query cells (markdown/reference/hg) free of query-only fields, respecting their contracts.
- **Relative-time "now" drift.** Execute resolves at run time; the renderer resolves at render time, so `now-*` bounds can differ by seconds between the two. For Perfetto (no `execute`) only the render path matters; for SQL cells the query uses the execute-time value. Acceptable and matches existing global-range behavior.

## Documentation
- Update `mkdocs/docs/web-app/notebooks/variables.md` (the "SQL Macro Substitution" section that already documents `$from`/`$to`) and `mkdocs/docs/web-app/notebooks/cell-types.md` to document the per-cell time range field: precedence (empty = screen range), that `from`/`to` accept relative strings, absolute timestamps, and macros (`$from`, `$to`, `$variable`, `$cell[N].col`, `$cell.selected.col`), and the contrast with Flame Graph's Initial From/To (view range). (`QUERY_GUIDE_URL` / `mkdocs/docs/query-guide/` is a SQL/DataFusion/Python/Grafana overview and does not cover notebook cells or macros — not the right target.)
- Retire `tasks/1314_per_cell_time_range_mockup.html` or note that the feature is now generalized to all query-backed cells.

## Testing Strategy
- **Unit — `resolveQueryTimeRange`**: unset → returns global; one bound set, other empty → mixes override + global fallback; relative (`now-1h`); absolute ISO; macro (`$variable`, `$cell.selected.col`) resolution; invalid string throws.
- **Execution — `useCellExecution.test.ts`**: a cell with `timeRange` runs its query with the overridden `begin/end` (assert the query params) and its `$from`/`$to` substitute the override; a bad override sets the cell to `error`.
- **Renderer — `notebook-cell-view.test.ts`**: `buildCellRendererProps` returns the overridden `timeRange`; invalid override falls back to the global range without throwing.
- **Perfetto — `PerfettoExportCell.test.tsx`**: renderer receives and fetches with the overridden range; editor shows the From/To field.
- **Editor**: `shouldShowTimeRange` matrix (incl. variable combobox/datasource vs. text/expression); `CellTimeRangeField` emits full `{from,to}` and `undefined` when cleared.
- Run `yarn lint`, `yarn type-check`, `yarn test`, `yarn build` from `analytics-web-app/`.

## Open Questions
1. **Error surfacing for `PerfettoExportCell`** (no `execute`): an invalid override currently only manifests at render, where we fall back to the global range silently. Acceptable, or should the Perfetto renderer show an inline warning (small follow-up) when its configured range fails to resolve? Recommended: ship silent fallback now, add a renderer warning as a follow-up if needed.
