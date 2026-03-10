# Row Selection in Table Cells Plan

## Issue Reference
- GitHub Issue: [#915](https://github.com/madesroches/micromegas/issues/915)

## Overview

Add the ability for users to select a row in a table cell. A radio-button column appears when selection is enabled. The selected row is accessible to downstream cells via `$cell.selected.column` macros, enabling interactive drill-down workflows within notebooks.

## Current State

Table cells render query results with sortable columns, pagination, column hiding, and column format overrides. There is no row selection or click interaction on rows.

**Key files:**
- `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` — renderer, editor, metadata
- `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` — `TableBody`, `SortHeader`, `useColumnManagement`, `ColumnOverride`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` — `substituteMacros`, `validateMacros`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — `cellResultsRef`, `completeCellExecution`, cell execution orchestration
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` — `CellExecutionContext`, `CellRendererProps`, `CellEditorProps`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` — `getAvailableCellResults`, wiring
- `analytics-web-app/src/components/AvailableVariablesPanel.tsx` — editor sidebar showing available macros
- `analytics-web-app/src/components/OverrideEditor.tsx` — collapsible section pattern reference

**Existing macro system:** `$cell[N].column` accesses a fixed row by index. `$variable.column` accesses multi-column variables. Both are regex-based substitution passes in `substituteMacros()`.

**Options pattern:** Cell options are stored in `config.options: Record<string, unknown>`, persisted in the notebook config JSON. `onOptionsChange` flows through `useCellManager` → `onConfigChange` → save. Pagination follows the pattern: `pageSize` is persisted in options, `currentPage` is ephemeral component state.

## Design Decisions

- **Row visual style:** Radio button column (leftmost column). More discoverable than highlight-only.
- **Macro syntax:** `$cell.selected.column` (e.g. `$processes.selected.process_id`).
- **Editor UI:** Collapsible "Row Selection" section below Overrides, collapsed by default. Contains None/Single radio buttons. Badge in collapsed header shows current mode.
- **Selection mode stored in config:** `options.selectionMode: 'none' | 'single'` — persisted, defaults to `'none'`.
- **Selected row index:** Ephemeral state in the renderer component (like `currentPage`). Resets on re-execution. Not persisted.
- **No selection state:** Downstream cells using `$cell.selected.column` show a "waiting for selection" placeholder with the cell name and macro reference.

## Design

### New Macro Pattern

```
$cell.selected.column
```

Regex: `/\$([a-zA-Z_][a-zA-Z0-9_]*)\.selected\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g`

This pattern is processed **after** `$cell[N].column` (cell result refs) and **before** `$variable.column` (dotted variables). The keyword `selected` disambiguates from dotted variable access.

### Substitution Logic

New pass in `substituteMacros()`, inserted between the cell result ref pass and the dotted variable pass:

```typescript
// $cell.selected.column — selected row access
if (cellSelections) {
  const selectedPattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\.selected\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
  result = result.replace(selectedPattern, (match, cellName, colName) => {
    const selection = cellSelections[cellName]
    if (!selection) return match  // leave unresolved
    const value = selection[colName]
    if (value === undefined || value === null) return match
    const table = cellResults?.[cellName]
    const field = table?.schema.fields.find((f) => f.name === colName)
    return escapeSqlValue(formatArrowValue(value, field?.type))
  })
}
```

### Selection State Storage

A new `cellSelectionsRef` in `useCellExecution.ts` (mirrors `cellResultsRef` pattern):

```typescript
const cellSelectionsRef = useRef<Record<string, Record<string, unknown>>>({})
```

Each entry maps a cell name to the selected row object (`table.get(selectedIndex)`). Updated when a table cell reports a selection via a new callback.

### Selection State Flow

```
TableCell renderer (radio click)
  → onSelectionChange(selectedRow: Record<string, unknown> | null)
    → useCellExecution updates cellSelectionsRef
      → triggers re-execution of downstream cells
        → substituteMacros receives cellSelections
```

### CellRendererProps / CellExecutionContext Changes

```typescript
// cell-registry.ts
interface CellRendererProps {
  // ... existing ...
  selectionMode?: 'none' | 'single'
  onSelectionChange?: (selectedRow: Record<string, unknown> | null) => void
}

interface CellExecutionContext {
  // ... existing ...
  cellSelections?: Record<string, Record<string, unknown>>
}
```

### Table Cell Config Option

```typescript
// In options:
{
  selectionMode: 'none' | 'single'  // default: 'none'
}
```

### TableBody Radio Column

When `selectionMode === 'single'`, `TableBody` renders an extra first column with radio indicators. The selected row gets `background: rgba(21,101,192,0.08)` and a filled radio dot.

### Selection Badge

Below the table (above pagination), a selection badge shows:
```
Selected: row N — first_col_value / second_col_value    × clear
```
Only visible when a row is selected.

### No-Selection Downstream Behavior

When a downstream cell references `$cell.selected.column` but no row is selected:
- The macro is left unresolved (same as unknown cell reference)
- The cell shows status "waiting for selection" with a placeholder:
  "Select a row in **cellName** to view results"
- This uses a new status or the existing blocked mechanism with a descriptive message

### Validation

Extend `validateMacros()` with a `cellSelections` parameter:
- Unknown cell name (no upstream cell with selection enabled): `"Unknown cell: {cellName}"`
- Cell has no selection enabled: `"Cell '{cellName}' does not have row selection enabled"`
- Unknown column: `"Column '{colName}' not found in cell '{cellName}'. Available: {columns}"`

### Available Variables Panel

When a cell has `selectionMode: 'single'`, the panel shows entries like:
```
$processes.selected.exe           flight-sql-srv
$processes.selected.process_id    f7e8d9c0-b1a2
```
When no row is selected, values show "no selection" in amber.

### Editor Section

A collapsible "Row Selection" section (matching the OverrideEditor pattern):
- Collapsed by default
- Collapsed header shows badge: "None" (muted) or "Single" (accent)
- Expanded shows two radio buttons: None, Single
- Help text shows macro syntax with dynamic cell name

## Implementation Steps

### Phase 1: Macro Substitution

1. **`notebook-utils.ts`** — Add `cellSelections?: Record<string, Record<string, unknown>>` parameter to `substituteMacros()` and `validateMacros()`. Add the `$cell.selected.column` regex pass between cell result refs and dotted variable passes. Update the dotted variable negative lookahead to not match `$cell.selected.column`.

2. **`notebook-utils.test.ts`** — Tests for: basic `$cell.selected.col` substitution, missing cell, missing column, no selection, SQL escaping, non-interference with `$cell[N].col` and `$variable.column`, timestamp formatting, validation errors.

### Phase 2: Selection State Plumbing

3. **`cell-registry.ts`** — Add `selectionMode`, `onSelectionChange` to `CellRendererProps`. Add `cellSelections` to `CellExecutionContext`.

4. **`useCellExecution.ts`** — Add `cellSelectionsRef`. Build `availableCellSelections` from ref during `executeCell`. Pass to `CellExecutionContext`. Add `updateCellSelection(cellName, row)` callback that updates the ref and triggers re-execution of downstream cells. Handle reset, migrate, remove.

5. **`NotebookRenderer.tsx`** — Thread `selectionMode` and `onSelectionChange` through `CellViewContext` → `buildCellRendererProps`. Thread `cellSelections` to editor panels and renderers.

6. **`notebook-cell-view.ts`** — Add `cellSelections` to `CellViewContext`, forward through `buildCellRendererProps`.

### Phase 3: Table Rendering

7. **`table-utils.tsx`** — Add `selectedRowIndex`, `onRowSelect`, `selectionMode` props to `TableBody`. Render radio column when `selectionMode === 'single'`. Handle row click → `onRowSelect(index)`. Style selected row. Add selection badge component.

8. **`TableCell.tsx` renderer** — Read `selectionMode` from options. Track `selectedRowIndex` as local state. Wire `onRowSelect` to update local state + call `onSelectionChange` with the row object. Render selection badge. Clear selection on re-execution (data change).

### Phase 4: Cell Execute Functions

9. **Cell execute functions** — Update `substituteMacros` calls in all cell `execute()` methods to pass `context.cellSelections`: TableCell, TransposedTableCell, LogCell, ChartCell, SwimlaneCell, PropertyTimelineCell, VariableCell.

10. **Cell renderer functions** — Update `substituteMacros` calls in renderers (MarkdownCell, ChartCell) to pass `cellSelections`.

### Phase 5: Editor UI

11. **`RowSelectionEditor.tsx`** (new component) — Collapsible section with None/Single radio buttons, matching OverrideEditor pattern. Help text shows `$cellName.selected.column` syntax.

12. **`TableCell.tsx` editor** — Add `RowSelectionEditor` below `OverrideEditor`. Handle `selectionMode` option change.

13. **`AvailableVariablesPanel.tsx`** — Add `cellSelections` prop. Show `$cell.selected.column` entries for cells with selection enabled.

14. **Cell editor components** — Thread `cellSelections` to `validateMacros` and `AvailableVariablesPanel` in all editors that use them.

### Phase 6: No-Selection Placeholder

15. **Downstream cell rendering** — When `substituteMacros` leaves `$cell.selected.column` unresolved and the cell has selection enabled but no row selected, show a "waiting for selection" placeholder instead of executing.

### Phase 7: HG Cell Support

16. **`HorizontalGroupCell.tsx`** — Thread `selectionMode`, `onSelectionChange`, and `cellSelections` through HG props.

### Phase 8: Documentation

17. **`mkdocs/docs/web-app/notebooks/variables.md`** — Add `$cell.selected.column` to syntax table, matching rules, examples section. Document the Row Selection editor option.

## Files to Modify

| File | Change |
|------|--------|
| `src/lib/screen-renderers/notebook-utils.ts` | Add `cellSelections` param, new regex pass, updated lookahead |
| `src/lib/screen-renderers/__tests__/notebook-utils.test.ts` | Tests for `$cell.selected.col` |
| `src/lib/screen-renderers/cell-registry.ts` | Add `selectionMode`, `onSelectionChange`, `cellSelections` to interfaces |
| `src/lib/screen-renderers/useCellExecution.ts` | Add `cellSelectionsRef`, `updateCellSelection`, re-execution trigger |
| `src/lib/screen-renderers/NotebookRenderer.tsx` | Thread selection state through context |
| `src/lib/screen-renderers/notebook-cell-view.ts` | Add `cellSelections` to `CellViewContext` |
| `src/lib/screen-renderers/table-utils.tsx` | Radio column, selected row styling, selection badge, `TableBody` props |
| `src/lib/screen-renderers/cells/TableCell.tsx` | Selection state, `RowSelectionEditor`, wiring |
| `src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | Thread selection props |
| `src/lib/screen-renderers/cells/LogCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/lib/screen-renderers/cells/ChartCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/lib/screen-renderers/cells/SwimlaneCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/lib/screen-renderers/cells/PropertyTimelineCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/lib/screen-renderers/cells/VariableCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/lib/screen-renderers/cells/TransposedTableCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/lib/screen-renderers/cells/MarkdownCell.tsx` | Pass `cellSelections` to `substituteMacros` |
| `src/components/AvailableVariablesPanel.tsx` | Add `cellSelections` prop, show `.selected.` entries |
| `src/components/RowSelectionEditor.tsx` | **New** — collapsible None/Single radio editor |
| `src/components/CellEditor.tsx` | Thread `cellSelections` to editor component |
| `mkdocs/docs/web-app/notebooks/variables.md` | Document `$cell.selected.column` syntax and Row Selection option |

## Trade-offs

### `$cell.selected.column` vs `$cell[selected].column`
**Chosen: `$cell.selected.column`.** Reads naturally, self-documenting. The keyword `selected` clearly communicates interactive dependency. Doesn't conflict with existing `$cell[N].column` numeric pattern. Slightly longer, but clearer.

### Selection state in options (persisted) vs ephemeral
**Chosen: Ephemeral.** The selected row index is interaction state, like `currentPage` in pagination. It resets when the query re-executes (data may change). The selection *mode* (`none`/`single`) is config and persists in options.

### Radio column vs click-anywhere highlight
**Chosen: Radio column.** More discoverable — users immediately see the table is interactive. Uglier but more usable. Click-anywhere could conflict with future cell interactions (text selection, links in overrides).

### Separate `cellSelections` parameter vs merging into `cellResults`
**Chosen: Separate parameter.** `cellResults` contains Arrow `Table` objects (full result sets). `cellSelections` contains single row objects (`Record<string, unknown>`). Different semantics — one is data, the other is UI state. Keeping them separate avoids conflating concerns.

## Documentation

- **`mkdocs/docs/web-app/notebooks/variables.md`** — Add `$cell.selected.column` to syntax table, matching rules, and examples. Document row selection configuration.

## Testing Strategy

### Unit Tests (`notebook-utils.test.ts`)
- `$cell.selected.col` substitutes correctly from a row object
- Missing cell leaves macro unresolved
- Missing column leaves macro unresolved
- No selection (cell not in cellSelections) leaves macro unresolved
- Single quotes in values are escaped
- Non-interference with `$cell[N].col` pattern
- Non-interference with `$variable.column` pattern
- Timestamp formatting
- Validation: unknown cell, unknown column, cell without selection enabled

### Manual Testing
1. Create a table cell, enable single selection in editor
2. Verify radio column appears, click row to select
3. Add downstream SQL cell using `$cell.selected.process_id`
4. Verify query executes with selected value
5. Clear selection, verify downstream shows placeholder
6. Change selected row, verify downstream re-executes
7. Re-execute table cell query, verify selection clears
8. Verify Available Variables panel shows `.selected.` entries

## Open Questions

None — all design decisions have been made.

## Mockups

- `tasks/row_select/mockup_selection_styles.html` — Visual style options (radio chosen)
- `tasks/row_select/mockup_notebook_workflow.html` — End-to-end notebook workflow
- `tasks/row_select/mockup_macro_syntax.html` — Macro syntax comparison (option 1 chosen)
- `tasks/row_select/mockup_table_editor.html` — Editor collapsible section with None/Single radio
