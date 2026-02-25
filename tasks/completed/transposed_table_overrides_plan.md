# Add Override Support to Transposed Table Cells

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/868

## Overview

Transposed table cells (used for key-value detail views like process info) should support column overrides, just like regular table cells already do. This enables inline markdown rendering with `$row.field` macro expansion — e.g., linking to a process's log/metrics views — without needing separate variable + markdown cells.

## Current State

**Regular TableCell** (`cells/TableCell.tsx`) already has full override support:
- Renderer extracts `overrides` from `options` (line 33) and passes them to `TableBody`
- Editor includes `OverrideEditor` component (lines 131-145, 180-187)
- `TableBody` in `table-utils.tsx` (line 384) builds an override lookup map and renders `OverrideCell` for overridden columns

**TransposedTableCell** (`cells/TransposedTableCell.tsx`) has no override support:
- Renderer always uses `formatCell()` for all values (line 83)
- Editor only has SQL editor + variables panel (lines 99-120)
- Does not import `OverrideCell`, `OverrideEditor`, or `ColumnOverride`

**Key architectural difference**: In a regular table, each row is a `Record<string, unknown>` with column values. In a transposed table, the original columns become rows, and original rows become columns. The data is stored as `{ name, type, values[] }` per transposed row. However, the original row data is reconstructable from `table.get(colIdx)`, which returns a full `Record<string, unknown>`.

## Design

The override system maps directly: each transposed row corresponds to an original column, so an override on a transposed row (e.g., `exe`) replaces that row's cell with markdown. The `$row` context for macro expansion is the full original data row — all fields are available. This is the key insight: `table.get(colIdx)` gives us the exact object shape that `OverrideCell` expects.

### Renderer

Build a precomputed override lookup map and original-row records. For each transposed row, check if its `name` (the column name) has an override. If so, render `OverrideCell` with the original row as context; otherwise use `formatCell` as before.

```
override on "exe" column, original row = {process_id: "abc", exe: "myapp", start_time: ...}
  → OverrideCell gets format="[View](/process/$row.process_id)", row={process_id, exe, start_time, ...}
  → renders: [View](/process/abc)
```

### Editor

Add `OverrideEditor` below the existing panels, matching TableCell's pattern. Wire up overrides in `options.overrides`.

## Implementation Steps

All changes are in `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx`.

### Step 1: Add imports

Add to the import from `table-utils`:
- `ColumnOverride`
- `OverrideCell`
- `TableColumn` (type for building columns array)

Add import:
- `OverrideEditor` from `@/components/OverrideEditor`

Add from React:
- `useCallback` (for editor handler)

### Step 2: Update renderer

In `TransposedTableCell`:

1. Add `variables` to the destructured props (matching `TableCell`'s pattern): `{ data, status, options, onOptionsChange, variables }`
2. Extract overrides from options: `const overrides = (options?.overrides as ColumnOverride[] | undefined) || []`
3. Build override lookup map (memoized): `Map<string, string>` from override column name to format string
4. Build columns array (memoized) from `table.schema.fields` as `TableColumn[]` — needed by `OverrideCell` for type-aware formatting
5. Build original row records (memoized): for each `colIdx`, `table.get(colIdx)` gives the full row object
6. In the render loop, for each visible transposed row and each value column:
   - Check if `overrideMap.has(row.name)`
   - If yes: render `<OverrideCell format={override} row={originalRows[colIdx]} columns={columns} variables={variables} />`
   - If no: render `formatCell(value, row.type)` as before

### Step 3: Update editor

In `TransposedTableCellEditor`:

1. Add `availableColumns` to the destructured props (matching `TableCellEditor`'s pattern): `{ config, onChange, variables, timeRange, availableColumns }`
2. Extract overrides from config options (memoized)
3. Add `handleOverridesChange` callback that updates `config.options.overrides`
4. Add `<OverrideEditor>` below `DocumentationLink`, passing:
   - `overrides`
   - `availableColumns={availableColumns || []}`
   - `availableVariables={Object.keys(variables)}`
   - `onChange={handleOverridesChange}`

## Files to Modify

| File | Changes |
|------|---------|
| `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` | Add override imports, renderer override logic, editor OverrideEditor integration |

## Testing Strategy

1. Open a notebook with a transposed table cell (e.g., process detail view)
2. Edit the cell, verify the Overrides panel appears below the query guide link
3. Add an override on a column (e.g., `exe`), set format to `**$row.exe**` — verify bold rendering
4. Add a link override: `[Logs](/logs?process=$row.process_id&begin=$row.start_time)` — verify clickable link with correct macro expansion
5. Verify non-overridden rows still render with default `formatCell` formatting
6. Verify `$variable` macros expand correctly in override formats
7. Test with hidden rows + overrides together (both features use `options`)
8. Run `yarn lint` and `yarn type-check` from `analytics-web-app/`
