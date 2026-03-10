# Row Selection in Table Cells Plan

## Issue Reference
- GitHub Issue: [#915](https://github.com/madesroches/micromegas/issues/915)

## Overview

Add the ability for users to select a row in a table cell. A radio-button column appears when selection is enabled. The selected row is accessible to downstream cells via `$cell.selected.column` macros, enabling interactive drill-down workflows within notebooks.

## Current State

**Status: Fully implemented.** All 8 phases complete. All 828 tests pass (including 13 new tests for selection macros/validation). ESLint, type-check, and production build all clean.

**Implementation summary:**
- `$cell.selected.column` macro substitution added to `substituteMacros()` / `validateMacros()`
- `cellSelectionsRef` added to `useCellExecution.ts` (mirrors `cellResultsRef` pattern)
- Radio-button column in `TableBody` when `selectionMode === 'single'`
- `SelectionBadge` component shows selected row preview with clear button
- `RowSelectionEditor` component (collapsible None/Single radio buttons)
- "Waiting for selection" blocking in `useCellExecution` via `findUnresolvedSelectionMacro()`
- `AvailableVariablesPanel` shows `$cell.selected.column` entries with live values or "no selection" placeholder
- All cell execute/render functions pass `cellSelections` through
- HorizontalGroupCell threads selection props to children
- Documentation updated in `mkdocs/docs/web-app/notebooks/variables.md`

**Key files:**
- `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` — renderer, editor, metadata
- `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` — `TableBody`, `SortHeader`, `useColumnManagement`, `ColumnOverride`, `SelectionBadge`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` — `substituteMacros`, `validateMacros`, `findUnresolvedSelectionMacro`
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` — `cellSelectionsRef`, `updateCellSelection`, cell execution orchestration
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` — `CellExecutionContext`, `CellRendererProps`, `CellEditorProps`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` — `getAvailableCellSelections`, wiring
- `analytics-web-app/src/lib/screen-renderers/notebook-cell-view.ts` — `CellViewContext` with `cellSelections`
- `analytics-web-app/src/components/AvailableVariablesPanel.tsx` — `cellSelections` prop, `CellSelectionEntry`, `CellSelectionPlaceholder`
- `analytics-web-app/src/components/RowSelectionEditor.tsx` — collapsible None/Single radio editor
- `analytics-web-app/src/components/CellEditor.tsx` — threads `cellSelections` to editor component

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

### Phase 1: Macro Substitution — DONE

1. **`notebook-utils.ts`** — Added `cellSelections` parameter to `substituteMacros()` and `validateMacros()`. Added `$cell.selected.column` regex pass (step 2b) between cell result refs and dotted variable passes. Added `findUnresolvedSelectionMacro()` helper. Updated dotted variable validation to skip `$cell.selected` patterns using a `selectedRefCellNames` set.

2. **`notebook-utils.test.ts`** — Added 2 describe blocks: `selected row ref substitution` (8 tests) and `validateMacros with cell selections` (5 tests).

### Phase 2: Selection State Plumbing — DONE

3. **`cell-registry.ts`** — Added `selectionMode`, `onSelectionChange` to `CellRendererProps`. Added `cellSelections` to `CellExecutionContext` and `CellEditorProps`.

4. **`useCellExecution.ts`** — Added `cellSelectionsRef`, `updateCellSelection` callback. Builds `availableCellSelections` during execution. Resets selections on full re-execution. Handles migrate/remove. Added "waiting for selection" check via `findUnresolvedSelectionMacro`.

5. **`NotebookRenderer.tsx`** — Added `getAvailableCellSelections()`. Threads `selectionMode`, `onSelectionChange`, and `cellSelections` through to renderers, HG cells, and editor panels.

6. **`notebook-cell-view.ts`** — Added `cellSelections` to `CellViewContext`, forwarded through `buildCellRendererProps`.

### Phase 3: Table Rendering — DONE

7. **`table-utils.tsx`** — Added `selectedRowIndex`, `onRowSelect`, `selectionMode` props to `TableBody`. Radio column with filled/empty dot. Selected row highlight `bg-[rgba(21,101,192,0.08)]`. Added `SelectionBadge` component.

8. **`TableCell.tsx` renderer** — Ephemeral `selectedRowIndex` state. `handleRowSelect` converts Arrow StructRow to plain object. Pagination-aware `handlePageRelativeRowSelect`. Renders radio column header and `SelectionBadge`. Clears selection on data change.

### Phase 4: Cell Execute Functions — DONE

9. **Cell execute functions** — All cell `execute()` methods pass `context.cellSelections` to `substituteMacros`: TableCell, TransposedTableCell, LogCell, ChartCell (5 call sites + `substituteOptionsWithMacros`), SwimlaneCell, PropertyTimelineCell, VariableCell.

10. **Cell renderer functions** — MarkdownCell and ChartCell renderers pass `cellSelections` to `substituteMacros`.

### Phase 5: Editor UI — DONE

11. **`RowSelectionEditor.tsx`** (new component) — Collapsible section with None/Single radio buttons. Badge in collapsed header. Help text shows `$cellName.selected.column` syntax.

12. **`TableCell.tsx` editor** — Added `RowSelectionEditor` below `OverrideEditor`. Passes `cellSelections` to `validateMacros` and `AvailableVariablesPanel`.

13. **`AvailableVariablesPanel.tsx`** — Added `cellSelections` prop. `CellSelectionEntry` (expandable, shows column values) and `CellSelectionPlaceholder` ("no selection" in amber).

14. **Cell editor components** — `CellEditor.tsx` threads `cellSelections` to editor components.

### Phase 6: No-Selection Placeholder — DONE

15. **`useCellExecution.ts`** — Before executing a cell, checks for unresolved `$cell.selected.column` macros via `findUnresolvedSelectionMacro()`. If found, sets cell status to `blocked` with message "Select a row in **cellName** to view results".

### Phase 7: HG Cell Support — DONE

16. **`HorizontalGroupCell.tsx`** — Added `cellSelections` and `onSelectionChange` to `HorizontalGroupCellProps`. Threads to child `buildCellRendererProps`. Attaches `selectionMode`/`onSelectionChange` for table children.

### Phase 8: Documentation — DONE

17. **`mkdocs/docs/web-app/notebooks/variables.md`** — Added `$cellName.selected.column` to syntax table, matching rules, example SQL, and new "Row Selection" section.

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

### Unit Tests (`notebook-utils.test.ts`) — ALL PASSING

13 new tests across 2 describe blocks:

**`selected row ref substitution`** (8 tests):
- `$cell.selected.col` substitutes correctly from a row object
- Missing cell leaves macro unresolved
- Missing column leaves macro unresolved
- No selection (cell not in cellSelections) leaves macro unresolved
- Single quotes in values are escaped
- Non-interference with `$cell[N].col` pattern
- Non-interference with `$variable.column` pattern
- Timestamp formatting

**`validateMacros with cell selections`** (5 tests):
- Unknown cell name
- Unknown column
- Cell without selection enabled
- Valid references pass
- Mixed valid and invalid references

Total test suite: 828 tests passing.

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
