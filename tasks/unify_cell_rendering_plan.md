# Extract `buildCellRendererProps` to unify cell rendering

## Context

Top-level cells and hg-group children are rendered through two separate code paths that each manually assemble `CellRendererProps`. When new props are added (like `value`, `onValueChange`, `dataSource`, `onTimeRangeSelect`), they must be added in both places — and forgetting one creates bugs (e.g., the missing variable combobox we just fixed). This refactor centralizes prop assembly into a single function.

## Current State

**Top-level path** — `NotebookRenderer.renderCell` (lines 935-1034):
- Builds `commonRendererProps` manually with ~15 fields
- Computes `statusText` with rows, bytes, elapsed time, fetch progress
- Calls `resolveCellDataSource` for per-cell data source
- Resolves `titleBarRenderer` from metadata
- Passes everything to `CellContainer` + `CellRenderer`

**HG child path** — `HorizontalGroupCell` child loop (lines 315-393):
- Builds `commonProps` manually with a subset of the same fields
- Missing: `onContentChange`, `onTimeRangeSelect`, `dataSource`
- No-ops: `onSqlChange: () => {}`, `onOptionsChange: () => {}`
- Computes `statusText` differently (rows + bytes only, no elapsed time)
- Resolves `titleBarRenderer` from metadata (added recently)

Every new `CellRendererProps` field requires changes in both places.

## Design

### New file: `notebook-cell-view.ts`

A pure function `buildCellRendererProps` that:
- Takes `cell`, `state`, a `context` bag (data/state including `variables`), and a `callbacks` bag (event handlers)
- Calls `getCellTypeMetadata` + `getRendererProps` from the registry
- Handles variable-specific branching (`value` from `context.allVariableValues[cell.name]`, `onValueChange` conditional on `cell.type === 'variable'`)
- Maps `context.availableVariables` → `props.variables` (scoped variable lookup for renderers)
- Spreads `rendererProps` from metadata **last** (highest precedence) — preserves current behavior where `getRendererProps` can override base fields like `data`, `status`, `options`
- Returns complete `CellRendererProps`

Also a helper `buildStatusText(cell, state)` that computes the status text string using the full format (rows, bytes, elapsed time, fetch progress). Both top-level and HG children use the same function — HG children currently show a simpler format (rows + bytes only), but the full format is more useful for understanding what's happening inside a collapsed group.

```ts
interface CellViewContext {
  /** Scoped variables visible to this cell (from cells above — used for query substitution) */
  availableVariables: Record<string, VariableValue>
  /** All variable values (used to look up this cell's own value for variable cells) */
  allVariableValues: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  isEditing: boolean
  dataSource?: string
}

interface CellViewCallbacks {
  onRun: () => void
  onSqlChange: (sql: string) => void
  onOptionsChange: (options: Record<string, unknown>) => void
  onContentChange?: (content: string) => void
  onValueChange?: (value: VariableValue) => void
  onTimeRangeSelect?: (from: Date, to: Date) => void
}

function buildCellRendererProps(
  cell: CellConfig,
  state: CellState,
  context: CellViewContext,
  callbacks: CellViewCallbacks,
): CellRendererProps

function buildStatusText(
  cell: CellConfig,
  state: CellState,
): string | undefined

/** Aggregate status for an HG group: total rows, total bytes, sum of elapsed times across all children. */
function buildHgStatusText(
  children: CellConfig[],
  cellStates: Record<string, CellState>,
): string | undefined
```

`buildHgStatusText` iterates over all children, sums up `numRows`, byte sizes, and `elapsedMs` from each child's state, and formats a single line like `"1,234 rows (5.2 MB) in 320ms"`. Returns `undefined` if no child has data.

### Changes to `HorizontalGroupCell.tsx`

1. Add new props to `HorizontalGroupCellProps`:
   - `onTimeRangeSelect?: (from: Date, to: Date) => void`
   - `defaultDataSource?: string` (for `resolveCellDataSource`)

2. Add `updateChildConfig` helper:
   ```ts
   const updateChildConfig = (childName, updates) => {
     const newChildren = config.children.map(c =>
       c.name === childName ? { ...c, ...updates } : c
     )
     onConfigChange({ ...config, children: newChildren })
   }
   ```
   This replaces the no-op `onSqlChange`/`onOptionsChange` callbacks — children can now be edited by their renderers (e.g., chart interactions that update options).

3. Replace manual prop assembly with `buildCellRendererProps`:
   ```ts
   const props = buildCellRendererProps(child, state,
     {
       availableVariables: variables,
       allVariableValues: variableValues,
       timeRange,
       isEditing: false,
       dataSource: resolveCellDataSource(child, variables, defaultDataSource),
     },
     {
       onRun: () => onChildRun(child.name),
       onSqlChange: (sql) => updateChildConfig(child.name, { sql }),
       onOptionsChange: (options) => updateChildConfig(child.name, { options }),
       onContentChange: (content) => updateChildConfig(child.name, { content }),
       onValueChange: (value) => onVariableValueChange(child.name, value),
       onTimeRangeSelect,
     },
   )
   ```

4. Replace manual statusText computation with `buildStatusText(child, state)`.

5. Keep `onVariableValueChange` as an HG prop — it dispatches runtime state changes (`setVariableValue`) that live in NotebookRenderer, not config mutations. The per-child `onValueChange` closure in step 3 delegates to it: `(value) => onVariableValueChange(child.name, value)`. Similarly, keep `variableValues` as a prop (passed as `context.allVariableValues`) and `variables` (passed as `context.availableVariables`).

### Changes to `NotebookRenderer.tsx`

1. Replace manual `commonRendererProps` assembly (lines 961-987) with:
   ```ts
   const commonRendererProps = buildCellRendererProps(cell, state,
     {
       availableVariables: availableVariables,
       allVariableValues: variableValues,
       timeRange: getTimeRangeForApi(rawTimeRange.from, rawTimeRange.to),
       isEditing: selectedCellIndex === index,
       dataSource: cellDataSource,
     },
     {
       onRun: () => executeCellByName(cell.name),
       onSqlChange: (sql) => updateCell(index, { sql }),
       onOptionsChange: (options) => updateCell(index, { options }),
       onContentChange: (content) => updateCell(index, { content }),
       onValueChange: (value) => { setVariableValue(cell.name, value); /* auto-run logic */ },
       onTimeRangeSelect: handleTimeRangeSelect,
     },
   )
   ```

2. Replace manual statusText computation (lines 942-955) with `buildStatusText(cell, state)`.

3. Pass `onTimeRangeSelect` and `defaultDataSource` to `HorizontalGroupCell`.

4. Compute and pass `statusText` to the HG group's `CellContainer` (currently missing — line 872 has no `statusText`):
   ```ts
   const hgStatusText = buildHgStatusText(hgConfig.children, cellStates)
   // ...
   <CellContainer
     statusText={hgStatusText}
     ...
   >
   ```
   This gives the HG cell a summary like `"1,234 rows (5.2 MB) in 320ms"` aggregated across all children, visible in the collapsed header.

## What stays separate

- **CellContainer** vs **ChildCellHeader** — legitimately different (resize, collapse, full dropdown vs compact). No change.
- **Error/blocked rendering** — CellContainer handles it for top-level (with padding/height), HG handles it inline (compact text-xs). The visual difference is intentional.
- **Auto-run logic** in `onValueChange` — stays in NotebookRenderer's callback, not in the shared function.
- **titleBarRenderer resolution** — both paths still do `meta.titleBarRenderer ? <TitleBarRenderer {...props} />` since the result goes to different containers (`CellContainer.titleBarContent` vs `ChildCellHeader.titleBarContent`).

## Tests

New file: `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-cell-view.test.ts`

All exported functions are pure (no React, no DOM, no async). Only mock needed: `getCellTypeMetadata` from the cell registry.

### `formatBytes`

- `formatBytes(0)` → `"0 B"`
- `formatBytes(512)` → `"512 B"`
- `formatBytes(1024)` → `"1.0 KB"`
- `formatBytes(1536)` → `"1.5 KB"`
- `formatBytes(1048576)` → `"1.0 MB"`

### `formatElapsedMs`

- `formatElapsedMs(0)` → `"0ms"`
- `formatElapsedMs(500)` → `"500ms"`
- `formatElapsedMs(999)` → `"999ms"`
- `formatElapsedMs(1000)` → `"1.00s"`
- `formatElapsedMs(5500)` → `"5.50s"`

### `buildStatusText`

- Non-combobox variable cell (`variableType: 'text'`) → `undefined`
- Combobox variable cell with data → returns row/byte string (not suppressed)
- `status: 'loading'` with `fetchProgress: { rows: 100, bytes: 2048 }` → `"100 rows (2.0 KB)"`
- `status: 'loading'` without `fetchProgress` → `undefined`
- Data present with `elapsedMs: 320` → `"N rows (X KB) in 320ms"`
- Data present without `elapsedMs` → `"N rows (X KB)"`
- Empty data array → `undefined`

### `buildHgStatusText`

- No children → `undefined`
- All children idle (empty data) → `undefined`
- Single child with data + elapsed → same format as `buildStatusText`
- Two children: sums rows, bytes, elapsed across both
- Mixed: one child has data, one idle → sums only the child with data
- Children with `elapsedMs` on some but not all → omits elapsed portion

### `buildCellRendererProps`

Mock `getCellTypeMetadata` to return `{ getRendererProps: () => ({}) }` by default.

- **Base mapping**: table cell → result has `name`, `data`, `status`, `error`, `timeRange`, `variables`, `isEditing`, `dataSource` from context; `onRun`, `onSqlChange`, `onOptionsChange`, `onContentChange`, `onTimeRangeSelect` from callbacks; `value` and `onValueChange` are `undefined`
- **Variable cell**: `value` set from `context.allVariableValues[cell.name]`, `onValueChange` set from callbacks
- **Variable cell with no value in map**: `value` is `undefined`
- **Metadata override**: mock `getRendererProps` returning `{ options: { custom: true } }` → result `options` is `{ custom: true }` (overrides base)
- **Metadata override of `data`**: mock returning `{ data: customData }` → result `data` is `customData` (not `state.data`)
- **Context passthrough**: `availableVariables` → `props.variables`, `timeRange` → `props.timeRange`, `isEditing` → `props.isEditing`, `dataSource` → `props.dataSource`

## Implementation Steps

1. Create `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` with `buildCellRendererProps`, `buildStatusText`, `buildHgStatusText`, `formatBytes`, `formatElapsedMs`
2. Add tests in `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-cell-view.test.ts`
3. Update `HorizontalGroupCell.tsx`: add props, add `updateChildConfig`, replace manual prop/status assembly with helpers, import `resolveCellDataSource`, remove local `formatBytes`
4. Update `NotebookRenderer.tsx`: replace manual prop/status assembly with helpers, pass new props to HorizontalGroupCell, remove local `formatBytes`/`formatElapsedMs`

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` (NEW)
- `analytics-web-app/src/lib/screen-renderers/__tests__/notebook-cell-view.test.ts` (NEW)
- `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`

## Verification

- `yarn type-check` — no type errors
- `yarn lint` — no lint errors
- `yarn test` — 716 tests pass
- Manual: variable combobox works in hg children, chart drag-to-zoom works in hg children, SQL/options changes from renderer interactions propagate correctly
- Manual: collapsed HG cell header shows aggregated rows/bytes/elapsed across all children
