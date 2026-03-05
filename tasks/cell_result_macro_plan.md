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

## Current State

### Macro Substitution
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts:263-311`

`substituteMacros()` handles three patterns in order:
1. `$begin` / `$end` — time range (line 270-272)
2. `$variable.column` — dotted multi-column variable access (line 274-292)
3. `$variable` — simple variable reference (line 294-308)

All patterns only reference `variables: Record<string, VariableValue>`, which is populated exclusively from `variable` type cells.

### Variable Collection During Execution
**File:** `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts:132-139`

```typescript
const availableVariables: Record<string, VariableValue> = {}
for (let i = 0; i < cellIndex; i++) {
  const prevCell = cells[i]
  if (prevCell.type === 'variable' && variableValuesRef.current[prevCell.name] !== undefined) {
    availableVariables[prevCell.name] = variableValuesRef.current[prevCell.name]
  }
}
```

Only `variable` type cells contribute. Cell result data (`cellStates[name].data`) is available but not exposed to macro substitution. Note: `cellStates` is React state (`useState`), not a ref — state updates from a previous cell's execution are batched and not visible synchronously in the next cell's callback closure.

### Variable Collection for Editor Panel
**File:** `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx:422-440`

`getAvailableVariables(index)` mirrors the execution logic — only `variable` cells above the current index, plus variable children inside HG groups.

### Cell Result Storage
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-types.ts:155-167`

`CellState.data` is `Table[]` (Apache Arrow). During execution, results are also registered in the WASM engine by cell name (line 179 of `useCellExecution.ts`). The Arrow `Table` supports `table.get(rowIndex)` which returns a row as `Record<string, unknown>`.

### Macro Validation
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts:325-365`

`validateMacros()` checks dotted and simple variable references against known variables. No awareness of cell results.

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

### Phase 1: Core Substitution

1. **`notebook-utils.ts`** — Add `cellResults?: Record<string, Table>` parameter to `substituteMacros()` and `validateMacros()`. Add the `$cell[N].col` regex pass before existing patterns. Update the simple variable negative lookahead from `(?!\.)` to `(?![.\[])` in both functions to prevent partial matches on cell result syntax. Update existing callers to pass `undefined` (no behavior change).

2. **`notebook-utils.test.ts`** — Add tests for the new pattern: basic substitution, out-of-bounds row index, missing column, missing cell, interaction with existing `$variable.column` patterns.

### Phase 2: Execution Wiring

3. **`cell-registry.ts`** — Add `cellResults: Record<string, Table>` to `CellExecutionContext`.

4. **`useCellExecution.ts`** — Add a `cellResultsRef` (`useRef<Record<string, Table>>({})`). Extract a `completeCellExecution` helper that updates both the ref and React state. Use it at all final-state `setCellStates` call sites in `executeCell`. Reset the ref when the engine resets; handle migrate/remove. Build `availableCellResults` map from the ref for upstream cells. Pass it in `CellExecutionContext`.

5. **Cell execute functions** — Update `substituteMacros()` calls in execute methods to pass `context.cellResults`: `TableCell`, `TransposedTableCell`, `LogCell`, `ChartCell` (execute only), `SwimlaneCell`, `PropertyTimelineCell`, `VariableCell` (combobox). Each just passes the new `context.cellResults` parameter.

6. **`cell-registry.ts`** — Add `cellResults?: Record<string, Table>` to `CellRendererProps`.

7. **Cell renderer functions** — Update `substituteMacros()` calls in renderers to pass `cellResults` from props:
   - `MarkdownCell` — has no execute method; its renderer calls `substituteMacros(content, variables, timeRange)` → add `cellResults`
   - `ChartCell` — renderer calls `substituteMacros` for option values (`substituteOptionsWithMacros`), per-series units, and chart labels → add `cellResults` to each call

8. **`notebook-cell-view.ts`** — Add `cellResults?: Record<string, Table>` to `CellViewContext`. Update `buildCellRendererProps` to forward `context.cellResults` into the returned `CellRendererProps`.

8b. **`NotebookRenderer.tsx`** — Pass `cellResults` through `CellViewContext` when calling `buildCellRendererProps`, so renderers receive it. Same for `HorizontalGroupCell` renderer props.

### Phase 3: Editor UI

9. **`NotebookRenderer.tsx`** — Add `getAvailableCellResults(index)` and pass it to `CellEditor` and `HgEditorPanel`.

10. **`CellEditor.tsx`** — Add `cellResults` to the component's local `CellEditorProps` interface (defined in `CellEditor.tsx`, separate from the one in `cell-registry.ts`). Thread `cellResults` prop to `meta.EditorComponent`.

11. **`cell-registry.ts`** — Add `cellResults?: Record<string, Table>` to `CellEditorProps`.

12. **`AvailableVariablesPanel.tsx`** — Add `cellResults` prop. Render a "Cell Results" section showing `$cell[0].column` entries with schema info.

13. **Cell editor components** — Pass `cellResults` to `AvailableVariablesPanel` in all editor components that use it.

### Phase 4: HG Cell Support

14. **`HorizontalGroupCell.tsx`** — Thread `cellResults` through to child cell renderers and editors, same as `variables`.

### Phase 5: Documentation

15. **`mkdocs/docs/web-app/notebooks/variables.md`** — Update the "SQL Macro Substitution" section:
    - Add `$cellName[N].column` to the syntax table with description: "Replaced with a value from an upstream cell's result table (row N, named column)"
    - Add a matching rule: "Cell result references first: `$cell[N].column` references are resolved before dotted variable and simple variable patterns."
    - Add an example showing a downstream cell referencing an upstream query result:
      ```sql
      -- Upstream cell "game_session" returns a table with process_id column
      SELECT time, level, target, msg
      FROM view_instance('log_entries', '$game_session[0].process_id')
      ORDER BY time DESC
      LIMIT 100
      ```
    - Add a brief note in the "Variable Scope" section that cell result references follow the same top-to-bottom scoping: only cells above can be referenced.

## Files to Modify

| File | Change |
|------|--------|
| `src/lib/screen-renderers/notebook-utils.ts` | Add `cellResults` param to `substituteMacros` and `validateMacros`, new regex pass, update simple variable lookahead to `(?![.\[])` |
| `src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Tests for `$cell[N].col` substitution and validation |
| `src/lib/screen-renderers/cell-registry.ts` | Add `cellResults` to `CellExecutionContext`, `CellRendererProps`, and `CellEditorProps` |
| `src/lib/screen-renderers/useCellExecution.ts` | Add `cellResultsRef` (`Record<string, Table>`), `completeCellExecution` helper, build `availableCellResults`, pass to context |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Add `getAvailableCellResults()`, pass to editor panels |
| `src/components/CellEditor.tsx` | Thread `cellResults` prop to editor component |
| `src/components/AvailableVariablesPanel.tsx` | Add `cellResults` prop, render cell result schemas |
| `src/lib/screen-renderers/cells/TableCell.tsx` | Pass `cellResults` to `substituteMacros` |
| `src/lib/screen-renderers/cells/TransposedTableCell.tsx` | Pass `cellResults` to `substituteMacros` |
| `src/lib/screen-renderers/cells/LogCell.tsx` | Pass `cellResults` to `substituteMacros` |
| `src/lib/screen-renderers/cells/ChartCell.tsx` | Pass `cellResults` to `substituteMacros` in execute and renderer |
| `src/lib/screen-renderers/cells/SwimlaneCell.tsx` | Pass `cellResults` to `substituteMacros` |
| `src/lib/screen-renderers/cells/PropertyTimelineCell.tsx` | Pass `cellResults` to `substituteMacros` |
| `src/lib/screen-renderers/cells/VariableCell.tsx` | Pass `cellResults` to `substituteMacros` |
| `src/lib/screen-renderers/cells/MarkdownCell.tsx` | Pass `cellResults` to `substituteMacros` in renderer (no execute method) |
| `src/lib/screen-renderers/notebook-cell-view.ts` | Add `cellResults` to `CellViewContext`, forward it through `buildCellRendererProps` |
| `src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | Thread `cellResults` through to children |
| `mkdocs/docs/web-app/notebooks/variables.md` | Document `$cell[N].column` syntax in macro substitution section |

## Trade-offs

### Separate `cellResults` parameter vs. merging into `variables`

**Chosen: Separate parameter.** Cell results are Arrow `Table` objects with row indexing; variables are `string | Record<string, string>` scalars. Merging would require a union type that complicates the existing variable system. The `[N]` indexing syntax naturally distinguishes the two concepts.

### Accessing only `data[0]` vs. supporting multiple tables per cell

**Chosen: `data[0]` only.** Every cell type currently produces at most one result table. Supporting `data[N]` would add complexity without a use case. If needed later, `$cell.1[0].col` or similar syntax could be added.

### Regex-based substitution vs. AST parsing

**Chosen: Regex.** Consistent with the existing macro system. The new pattern `$name[N].col` is unambiguous and doesn't conflict with SQL syntax (`$` is not a standard SQL token). An AST approach would be over-engineering for macro substitution.

## Testing Strategy

### Unit Tests (`notebook-utils.test.ts`)
- `$cell[0].col` substitutes correctly from an Arrow Table
- `$cell[0].col` with out-of-bounds row index leaves macro unresolved
- `$cell[0].missing_col` leaves macro unresolved
- `$unknown_cell[0].col` leaves macro unresolved
- Cell result refs don't interfere with existing `$variable.column` patterns
- Cell result refs don't interfere with `$variable` patterns
- `validateMacros` reports errors for unknown cells, out-of-bounds rows, missing columns
- Simple variable pattern does not partially match `$cell_name` in `$cell_name[0].col` (lookahead `(?![.\[])`)
- `validateMacros` does not report "Unknown variable" for valid cell result references

### Manual Testing
1. Create a notebook with a query cell returning multiple columns
2. Add a downstream cell using `$cell[0].column` in its SQL
3. Verify substitution works and query executes
4. Verify the Available Variables panel shows cell result schemas
5. Verify validation errors appear in the editor for bad references

## Open Questions

1. **Should `$cell[0]` without a column be supported?** Current design requires `.column`. A possible use case is single-column results, but that can be written as `$cell[0].col_name`.
