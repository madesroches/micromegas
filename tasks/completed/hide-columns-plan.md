# Hide Columns via Context Menu — Implementation Plan (Completed)

**Issue:** [#788](https://github.com/madesroches/micromegas/issues/788)
**Design:** Option C — Column header context menu with restore pills
**Status:** Completed

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

## Files Changed

### `table-utils.tsx` — Shared components and hook
- Added `@radix-ui/react-context-menu` dependency (context menu, not dropdown — better UX for right-click)
- `SortHeader`: added optional `onHide`, `onSortAsc`, `onSortDesc` props. When `onHide` is provided, wraps `<th>` in a Radix `ContextMenu` with Sort Ascending / Sort Descending / Hide Column items. Left-click sort cycling unchanged.
- `HiddenColumnsBar`: bar above table showing eye-off icon + "Hidden:" label + pill chips per column. Compact variant for notebook cells. "Show all" button when >1 column hidden.
- `useColumnManagement` hook: extracts all sort/hide/restore handler logic shared between `TableRenderer` and `TableCell`.

### `TableRenderer.tsx` — Screen-level integration
- Added `hiddenColumns` to `TableConfig` interface
- Uses `useColumnManagement` hook for all sort/hide/restore handlers
- Filters columns via `hiddenSet` before rendering headers and body
- Renders `HiddenColumnsBar` above the table

### `cells/TableCell.tsx` — Notebook cell integration
- Uses `useColumnManagement` hook (same as TableRenderer)
- Renders compact `HiddenColumnsBar`
- Passes `onHide` to `SortHeader`

### `OverrideEditor.tsx` — No changes
Override editor still shows all columns (including hidden ones).

## Tests
- `getNextSortState`: sort cycling logic (new column, ASC→DESC, DESC→none)
- `SortHeader`: rendering, left-click sort, context menu not opening on left-click
- `HiddenColumnsBar`: empty state, pill rendering, restore callbacks, "Show all" visibility

## Edge Cases

- **Column in overrides but hidden**: Column is hidden (not rendered). Override is preserved in config but has no visual effect until the column is restored.
- **Sort on hidden column**: Clear `sortColumn`/`sortDirection` if the sorted column gets hidden.
- **All columns hidden**: The `HiddenColumnsBar` shows all pills, making it obvious how to restore them. No special empty state needed.
- **Query changes columns**: Hidden column entries are intentionally preserved across query changes so users don't lose their config when temporarily switching queries. Stale entries are harmless (they simply don't match any column).
