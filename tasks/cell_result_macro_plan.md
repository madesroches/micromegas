# Cell Result Row References in Macros

## Issue Reference
- GitHub Issue: [#898](https://github.com/madesroches/micromegas/issues/898)

## Overview

Extend the notebook macro system to support `$cell_name[row_index].column_name` syntax, allowing SQL queries to reference specific values from upstream cell result tables. This eliminates the need for intermediate variable cells when chaining queries.

**Example:**
```sql
-- game_session_info cell returns a table with game_server_micromegas_process_id column
-- A downstream cell can reference that value directly:
SELECT time, level, target, msg
FROM view_instance('log_entries', '$game_session_info[0].game_server_micromegas_process_id')
ORDER BY time DESC
LIMIT 100
```

## Status: IMPLEMENTED

All phases complete. Type-check, lint, and tests (805 total, 14 new) pass.

## Design

### New Syntax

`$cell_name[row_index].column_name`

- `cell_name` — name of any upstream cell that has executed successfully
- `row_index` — zero-based integer index into the result table
- `column_name` — column name from the result schema

The `[N].column` suffix is always required — a bare `$cell_name[0]` (without column) is not supported since a table row has no scalar representation.

### Regex Pattern

```
/\$([a-zA-Z_][a-zA-Z0-9_]*)\[(\d+)\]\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
```

Processed **before** existing patterns to avoid partial matches (e.g., `$game_session_info[0].col` must not be partially consumed by the `$game_session_info` simple variable pattern).

Additionally, the existing simple variable pattern's negative lookahead `(?!\.)` must be extended to `(?![.\[])` so that `$cell_name` is not matched when followed by `[` (i.e., cell result syntax). Without this, an unresolved cell result reference like `$unknown_cell[0].col` would have `$unknown_cell` partially consumed by the simple variable pattern, producing broken output. The same change applies in both `substituteMacros` and `validateMacros`.

### Data Flow

Cell results are passed as a separate parameter alongside variables:

```typescript
export function substituteMacros(
  sql: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string },
  cellResults?: Record<string, Table>,  // NEW
): string
```

Using a separate parameter (rather than merging into `variables`) keeps the type boundaries clean — `VariableValue` is `string | Record<string, string>`, while cell results are Arrow `Table` objects with different access patterns.

### Substitution Logic

Insert a new pass between time range and dotted-variable substitution:

```typescript
// New pass: $cell[row].column — cell result row access
if (cellResults) {
  const cellRefPattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\[(\d+)\]\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
  result = result.replace(cellRefPattern, (match, cellName, rowIdxStr, colName) => {
    const table = cellResults[cellName]
    if (!table) return match  // leave unresolved
    const rowIdx = parseInt(rowIdxStr, 10)
    if (rowIdx >= table.numRows) return match
    const row = table.get(rowIdx)
    if (!row || row[colName] === undefined || row[colName] === null) return match
    return escapeSqlValue(String(row[colName]))
  })
}
```

### Validation

Extend `validateMacros()` with a `cellResults` parameter to check:
- Unknown cell name: `"Unknown cell: {cellName}"`
- Row index out of bounds: `"Row index {N} out of bounds for cell '{cellName}' ({numRows} rows)"`
- Unknown column: `"Column '{colName}' not found in cell '{cellName}'. Available: {columns}"`

### Available Cell Results Map

#### During Execution (`useCellExecution.ts`)

Add a `cellResultsRef` (a `useRef`) storing only the Arrow Tables needed for macro substitution — not the full `CellState`. This mirrors the existing `variableValuesRef` pattern for synchronous access during sequential execution, where React state updates are batched and `useEffect` fires after render (too late for the next cell in a sequential run).

```typescript
const cellResultsRef = useRef<Record<string, Table>>({})
```

Extract a helper that updates both the ref and React state atomically, preventing the ref from getting out of sync:

```typescript
const completeCellExecution = (name: string, state: CellState) => {
  if (state.data.length > 0) {
    cellResultsRef.current = { ...cellResultsRef.current, [name]: state.data[0] }
  }
  setCellStates((prev) => ({ ...prev, [name]: state }))
}
```

Use `completeCellExecution` at every site in `executeCell` that sets a cell's final state (no-execute success, success, and error). The helper ensures the ref always stays in sync without requiring manual duplication at each call site. Also reset the ref when the WASM engine resets in `executeFromCell`: `cellResultsRef.current = {}`, and handle `migrateCellState`/`removeCellState` with trivial key rename/delete on the ref.

Then build `availableCellResults` from the ref inside `executeCell`:

```typescript
const availableCellResults: Record<string, Table> = {}
for (let i = 0; i < cellIndex; i++) {
  const table = cellResultsRef.current[cells[i].name]
  if (table) availableCellResults[cells[i].name] = table
}
```

Pass `availableCellResults` in the `CellExecutionContext` object. Cell `execute()` functions access it as `context.cellResults` and pass it to `substituteMacros`.

#### For Editor Panel (`NotebookRenderer.tsx`)

Add a `getAvailableCellResults(index)` function alongside `getAvailableVariables(index)`:

```typescript
const getAvailableCellResults = (index: number): Record<string, Table> => {
  const results: Record<string, Table> = {}
  for (let i = 0; i < index; i++) {
    const cell = cells[i]
    const state = cellStates[cell.name]
    if (state?.status === 'success' && state.data.length > 0) {
      results[cell.name] = state.data[0]
    }
    if (cell.type === 'hg') {
      for (const child of (cell as HorizontalGroupCellConfig).children) {
        const childState = cellStates[child.name]
        if (childState?.status === 'success' && childState.data.length > 0) {
          results[child.name] = childState.data[0]
        }
      }
    }
  }
  return results
}
```

### AvailableVariablesPanel Update

Add a new section showing upstream cell results with their schemas:

```typescript
interface AvailableVariablesPanelProps {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  additionalVariables?: { name: string; description: string }[]
  cellResults?: Record<string, Table>  // NEW
}
```

Display cell results as expandable entries showing `$cell_name[0].column` for each column in the schema. Only show cells that have data (non-empty results).

### CellExecutionContext Update

Add `cellResults` to the context so cell `execute()` functions can pass it to `substituteMacros`:

```typescript
export interface CellExecutionContext {
  variables: Record<string, VariableValue>
  cellResults: Record<string, Table>  // NEW
  timeRange: { begin: string; end: string }
  runQuery: (sql: string) => Promise<Table>
  runQueryAs?: (sql: string, tableName: string, dataSource?: string) => Promise<Table>
  registerTable?: (ipcBytes: Uint8Array) => void
}
```

All cell type `execute()` functions that call `substituteMacros` will pass `context.cellResults`.

### CellRendererProps Update

Some cells call `substituteMacros` in their **renderer** component rather than (or in addition to) an execute function:
- **MarkdownCell** — has no execute method; calls `substituteMacros` in the renderer to resolve macros in markdown content
- **ChartCell** — calls `substituteMacros` in the renderer for chart option values, per-series units, and chart labels (in addition to SQL substitution in execute)

These renderers receive `CellRendererProps`, not `CellExecutionContext`. To support `$cell[N].col` in these contexts, add `cellResults` to `CellRendererProps`:

```typescript
export interface CellRendererProps {
  // ... existing fields ...
  cellResults?: Record<string, Table>  // NEW
}
```

`cellResults` is threaded from `NotebookRenderer` through `buildCellRendererProps` and `HorizontalGroupCell` into renderer components, the same way `variables` is today.

## Implementation Steps

### Phase 1: Core Substitution — DONE

1. [x] **`notebook-utils.ts`** — Added `cellResults?: Record<string, Table>` parameter to `substituteMacros()` and `validateMacros()`. Added the `$cell[N].col` regex pass before existing patterns. Updated the simple variable negative lookahead from `(?!\.)` to `(?![.\[])` in both functions.

2. [x] **`notebook-utils.test.ts`** — Added 14 tests: basic substitution, out-of-bounds row index, missing column, missing cell, interaction with existing `$variable.column` and `$variable` patterns, SQL escaping, lookahead correctness, and validation errors for unknown cells/rows/columns.

### Phase 2: Execution Wiring — DONE

3. [x] **`cell-registry.ts`** — Added `cellResults: Record<string, Table>` to `CellExecutionContext`.

4. [x] **`useCellExecution.ts`** — Added `cellResultsRef` (`useRef<Record<string, Table>>({})`), `completeCellExecution` helper updating both ref and React state. Used at all final-state sites (no-execute, success, error). Ref reset on engine reset; migrate/remove handled. `availableCellResults` built from ref for upstream cells, passed in `CellExecutionContext`.

5. [x] **Cell execute functions** — Updated `substituteMacros()` calls in execute methods to pass `context.cellResults`: `TableCell`, `TransposedTableCell`, `LogCell`, `ChartCell`, `SwimlaneCell`, `PropertyTimelineCell`, `VariableCell`.

6. [x] **`cell-registry.ts`** — Added `cellResults?: Record<string, Table>` to `CellRendererProps`.

7. [x] **Cell renderer functions** — Updated `substituteMacros()` calls in renderers:
   - `MarkdownCell` — renderer passes `cellResults`
   - `ChartCell` — renderer passes `cellResults` to `substituteOptionsWithMacros`, per-series units, and chart labels

8. [x] **`notebook-cell-view.ts`** — Added `cellResults?: Record<string, Table>` to `CellViewContext`. `buildCellRendererProps` forwards `context.cellResults`.

8b. [x] **`NotebookRenderer.tsx`** — `cellResults` threaded through `CellViewContext` for `buildCellRendererProps` and `HorizontalGroupCell`.

### Phase 3: Editor UI — DONE

9. [x] **`NotebookRenderer.tsx`** — Added `getAvailableCellResults(index)`, passed to `CellEditor` and `HgEditorPanel`.

10. [x] **`CellEditor.tsx`** — Added `cellResults` to local `CellEditorProps`, threaded to `meta.EditorComponent`.

11. [x] **`cell-registry.ts`** — Added `cellResults?: Record<string, Table>` to `CellEditorProps`.

12. [x] **`AvailableVariablesPanel.tsx`** — Added `cellResults` prop. Renders a "Cell Results" section with `$cell[0].column` entries and row count per cell.

13. [x] **Cell editor components** — All editors that use `validateMacros` or `AvailableVariablesPanel` now accept `cellResults` and pass it through: `TableCell`, `ChartCell`, `MarkdownCell`, `VariableCell`, `PropertyTimelineCell`, `SwimlaneCell`, `LogCell`, `TransposedTableCell`. `useMemo` dependency arrays updated accordingly.

### Phase 4: HG Cell Support — DONE

14. [x] **`HorizontalGroupCell.tsx`** — Added `cellResults` to `HorizontalGroupCellProps`, `HorizontalGroupCellEditorProps`, and `ChildEditorViewProps`. Threaded through to `buildCellRendererProps` and `meta.EditorComponent`.

### Phase 5: Documentation — DONE

15. [x] **`mkdocs/docs/web-app/notebooks/variables.md`** — Updated:
    - Syntax table: added `$cellName[N].column`
    - Matching rules: added "Cell result references first"
    - Examples: added upstream cell reference example
    - Variable Scope: noted cell result references follow same top-to-bottom scoping

## Files Modified

| File | Change |
|------|--------|
| `src/lib/screen-renderers/notebook-utils.ts` | Added `cellResults` param to `substituteMacros` and `validateMacros`, new regex pass, updated simple variable lookahead to `(?![.\[])` |
| `src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | 14 new tests for `$cell[N].col` substitution and validation |
| `src/lib/screen-renderers/cell-registry.ts` | Added `cellResults` to `CellExecutionContext`, `CellRendererProps`, and `CellEditorProps` |
| `src/lib/screen-renderers/useCellExecution.ts` | Added `cellResultsRef`, `completeCellExecution` helper, builds `availableCellResults`, passes to context |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Added `getAvailableCellResults()`, passes to editor panels and renderers, updated `HgEditorPanelProps` |
| `src/components/CellEditor.tsx` | Threads `cellResults` prop to editor component |
| `src/components/AvailableVariablesPanel.tsx` | Added `cellResults` prop, renders cell result schemas section |
| `src/lib/screen-renderers/cells/TableCell.tsx` | Passes `cellResults` to `substituteMacros` and `validateMacros` in execute and editor |
| `src/lib/screen-renderers/cells/TransposedTableCell.tsx` | Passes `cellResults` to `substituteMacros` in execute, `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/cells/LogCell.tsx` | Passes `cellResults` to `substituteMacros` in execute, `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/cells/ChartCell.tsx` | Passes `cellResults` to `substituteMacros` in execute and renderer, `validateMacros` and `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/cells/SwimlaneCell.tsx` | Passes `cellResults` to `substituteMacros` in execute, `validateMacros` and `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/cells/PropertyTimelineCell.tsx` | Passes `cellResults` to `substituteMacros` in execute, `validateMacros` and `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/cells/VariableCell.tsx` | Passes `cellResults` to `substituteMacros` in execute, `validateMacros` and `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/cells/MarkdownCell.tsx` | Passes `cellResults` to `substituteMacros` in renderer, `validateMacros` and `AvailableVariablesPanel` in editor |
| `src/lib/screen-renderers/notebook-cell-view.ts` | Added `cellResults` to `CellViewContext`, forwarded through `buildCellRendererProps` |
| `src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | Threads `cellResults` through props, `buildCellRendererProps`, `ChildEditorView`, `meta.EditorComponent` |
| `mkdocs/docs/web-app/notebooks/variables.md` | Documented `$cell[N].column` syntax, matching rule, example, and scope note |

## Trade-offs

### Separate `cellResults` parameter vs. merging into `variables`

**Chosen: Separate parameter.** Cell results are Arrow `Table` objects with row indexing; variables are `string | Record<string, string>` scalars. Merging would require a union type that complicates the existing variable system. The `[N]` indexing syntax naturally distinguishes the two concepts.

### Accessing only `data[0]` vs. supporting multiple tables per cell

**Chosen: `data[0]` only.** Every cell type currently produces at most one result table. Supporting `data[N]` would add complexity without a use case. If needed later, `$cell.1[0].col` or similar syntax could be added.

### Regex-based substitution vs. AST parsing

**Chosen: Regex.** Consistent with the existing macro system. The new pattern `$name[N].col` is unambiguous and doesn't conflict with SQL syntax (`$` is not a standard SQL token). An AST approach would be over-engineering for macro substitution.

## Testing

### Unit Tests (`notebook-utils.test.ts`) — 14 new tests, all passing

**`cell result ref substitution` describe block:**
- [x] `$cell[0].col` substitutes correctly from an Arrow Table
- [x] Out-of-bounds row index leaves macro unresolved
- [x] Missing column leaves macro unresolved
- [x] Unknown cell leaves macro unresolved
- [x] Cell result refs don't interfere with existing `$variable.column` patterns
- [x] Cell result refs don't interfere with `$variable` patterns
- [x] Single quotes in cell result values are escaped
- [x] Simple variable pattern does not partially match `$cell_name` in `$cell_name[0].col` (lookahead)

**`validateMacros with cell results` describe block:**
- [x] Reports error for unknown cell name
- [x] Reports error for out-of-bounds row index
- [x] Reports error for unknown column (shows available columns)
- [x] Passes for valid cell result reference
- [x] Skips cell result validation when `cellResults` is undefined

**`validateMacros` existing block (new test):**
- [x] Does not report "Unknown variable" for valid cell result references

### Manual Testing
1. Create a notebook with a query cell returning multiple columns
2. Add a downstream cell using `$cell[0].column` in its SQL
3. Verify substitution works and query executes
4. Verify the Available Variables panel shows cell result schemas
5. Verify validation errors appear in the editor for bad references

## Open Questions

(none)

## Closed Questions

1. **Should `$cell[0]` without a column be supported?** No. The `.column` suffix is always required. Even for single-column results, write `$cell[0].col_name`.

2. **Should available variables and cell results be scoped per-cell?** Keep upstream-only scoping. The sequential execution model (`executeFromCell` runs cells top-to-bottom; failures block downstream) makes positional scoping the natural fit — you can't use a result that hasn't been computed yet. Exposing all variables regardless of position would require a dependency graph, topological sorting, and cycle detection, fundamentally changing the execution architecture for no demonstrated benefit. The only minor limitation is that HG siblings can't reference each other, but HG children are conceptually independent (displayed side-by-side). No user friction has been reported. The current model is simple, predictable ("top-to-bottom, like a script"), and prevents cycles structurally without runtime checks.
