# Hide Columns via Context Menu — Implementation Plan

**Issue:** [#788](https://github.com/madesroches/micromegas/issues/788)
**Design:** Option C — Column header context menu with restore pills

## Overview

Right-click any column header → "Hide Column" menu item. Hidden columns shown as clickable pill chips above the table for easy restoration. Works in both standalone TableRenderer screens and notebook TableCell cells.

## Config Shape

```ts
// TableRenderer (screen-level)
interface TableConfig {
  sql: string
  hiddenColumns?: string[]   // ← new
  overrides?: ColumnOverride[]
  sortColumn?: string
  sortDirection?: 'asc' | 'desc'
}

// TableCell (notebook cell)
// options.hiddenColumns: string[]   ← stored alongside options.overrides
```

No new API — `hiddenColumns` is persisted automatically through the existing `ScreenConfig` index signature / notebook cell options.

## Files to Change

### 1. `table-utils.tsx` — New shared components

**New: `ColumnHeaderMenu` component**
- Wraps each `<th>` with a Radix `DropdownMenu` triggered by `onContextMenu`
- Left-click still cycles sort (existing behavior unchanged)
- Right-click opens menu with:
  - Sort Ascending
  - Sort Descending
  - separator
  - Hide Column
- Props: `columnName`, `onSort`, `onHide`, existing sort state
- Uses `@radix-ui/react-dropdown-menu` (already a dependency)

**New: `HiddenColumnsBar` component**
- Renders a bar above the table when `hiddenColumns.length > 0`
- Shows eye-off icon + "Hidden:" label + pill chip per column
- Clicking ✕ on a pill calls `onRestore(columnName)`
- Compact variant prop for notebook cells

**Update `SortHeader`**
- Add optional `onHide?: (columnName: string) => void` prop
- When `onHide` is provided, attach `onContextMenu` handler that opens the dropdown
- When `onHide` is not provided, existing click-only behavior is unchanged (backward compatible)

**Update `TableBody`**
- No changes needed — filtering happens upstream before columns are passed

### 2. `TableRenderer.tsx` — Screen-level integration

- Read `tableConfig.hiddenColumns` (default `[]`)
- Filter `columns` array before passing to `SortHeader` loop and `TableBody`
- Keep `availableColumns` unfiltered (overrides editor still sees all columns)
- Add `handleHideColumn(name)` — appends to `hiddenColumns` via `onConfigChange`
- Add `handleRestoreColumn(name)` — removes from `hiddenColumns` via `onConfigChange`
- Render `HiddenColumnsBar` above the table when columns are hidden
- Pass `onHide` to `SortHeader`

### 3. `cells/TableCell.tsx` — Notebook cell integration

- Read `options?.hiddenColumns` (default `[]`)
- Same filtering pattern as TableRenderer
- `handleHideColumn` / `handleRestoreColumn` use `onOptionsChange`
- Render compact `HiddenColumnsBar` above the table
- Pass `onHide` to `SortHeader`

### 4. `OverrideEditor.tsx` — No changes

Overrides stay as format-only. Hidden columns are a separate concern. The override editor continues to show all columns (including hidden ones) in its dropdown since a column can have both a format override and be hidden.

## Implementation Steps

### Step 1: Add `ColumnHeaderMenu` and `HiddenColumnsBar` to `table-utils.tsx`

- Import `DropdownMenu` from `@radix-ui/react-dropdown-menu`
- Import `EyeOff`, `ArrowUp`, `ArrowDown` from `lucide-react`
- Build `ColumnHeaderMenu` wrapping the existing `<th>` content
- Build `HiddenColumnsBar` with pill chips
- Add `onHide` optional prop to `SortHeader`
- When `onHide` is set, `SortHeader` renders `ColumnHeaderMenu` internally

### Step 2: Wire up `TableRenderer.tsx`

- Add `hiddenColumns` to `TableConfig` interface
- Filter columns: `columns.filter(c => !hiddenSet.has(c.name))`
- Add hide/restore handlers that call `onConfigChange`
- Render `HiddenColumnsBar` inside `renderContent()` above the `<table>`
- Pass `onHide={handleHideColumn}` to each `SortHeader`

### Step 3: Wire up `cells/TableCell.tsx`

- Same pattern using `options.hiddenColumns` and `onOptionsChange`
- Use `compact` prop on `HiddenColumnsBar`
- Pass `onHide` to `SortHeader`

### Step 4: Tests

- Unit test `HiddenColumnsBar` renders pills, calls `onRestore`
- Unit test column filtering logic in both renderers
- Verify hidden columns are excluded from `<thead>` and `<tbody>`
- Verify overrides on hidden columns don't render
- Verify restoring a column brings it back

## Edge Cases

- **Column in overrides but hidden**: Column is hidden (not rendered). Override is preserved in config but has no visual effect until the column is restored.
- **Sort on hidden column**: Clear `sortColumn`/`sortDirection` if the sorted column gets hidden.
- **All columns hidden**: Show the `HiddenColumnsBar` with all pills + an empty state message in the table area.
- **Query changes columns**: On re-query, remove any `hiddenColumns` entries that no longer exist in the new schema (stale cleanup).
