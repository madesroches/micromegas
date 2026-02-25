# Hide Rows in Transposed Table Plan

## Overview

Add row hiding to the transposed table cell, mirroring how column hiding works in the regular table. In a transposed table, each "row" corresponds to a field (column) from the original data, so hiding a row means hiding a field from the transposed view. Right-click a row header (field name) to get a context menu with "Hide Row". Hidden rows appear as restoration pills above the table.

## Current State

The transposed table cell (`TransposedTableCell.tsx`) renders each schema field as a row with no interactivity beyond display. It ignores `options` and `onOptionsChange` from `CellRendererProps` even though they're available.

The regular table already has a complete column-hiding system:
- `useColumnManagement` hook in `table-utils.tsx` manages `hiddenColumns` state
- `HiddenColumnsBar` component renders restoration pills
- `SortHeader` wraps column headers in a Radix context menu with "Hide Column"
- State persisted in `options.hiddenColumns: string[]`

## Design

### Reuse Existing Infrastructure

Since hiding a "row" in the transposed view is conceptually hiding a field, we can store `hiddenRows: string[]` in `options` and reuse the `HiddenColumnsBar` component (with different label) for restoration.

### New Component: `RowContextMenu`

A lightweight wrapper around Radix `ContextMenu` for row headers. Simpler than `SortHeader`'s context menu since it only needs one action (Hide Row) — no sort options.

```tsx
// in table-utils.tsx
function RowContextMenu({
  rowName,
  onHide,
  children
}: {
  rowName: string
  onHide: (name: string) => void
  children: React.ReactNode
})
```

### New Hook: `useRowManagement`

Simplified version of `useColumnManagement` — only manages hide/restore, no sort:

```tsx
// in table-utils.tsx
function useRowManagement(
  config: { hiddenRows?: string[]; [key: string]: unknown },
  onChange: (config: Record<string, unknown>) => void
) → { hiddenRows, handleHideRow, handleRestoreRow, handleRestoreAll }
```

### Config Shape

```ts
// options on QueryCellConfig for transposed cells
{
  hiddenRows?: string[]   // field names to hide
}
```

Persisted automatically through the existing notebook cell options mechanism.

### HiddenColumnsBar Generalization

Rename the `"Hidden:"` label parameter or add a `label` prop to `HiddenColumnsBar` so it can display `"Hidden:"` for both use cases. Alternatively, create a `HiddenItemsBar` alias or just pass `hiddenRows` as `hiddenColumns` — the component doesn't care about the semantics, just the strings. The simplest approach: add an optional `label` prop to `HiddenColumnsBar` (defaults to `"Hidden:"`).

## Implementation Steps

### Step 1: Add `useRowManagement` hook to `table-utils.tsx`

- Add hook that manages `hiddenRows` in config
- `handleHideRow(name)`: adds name to `hiddenRows` array
- `handleRestoreRow(name)`: removes name from `hiddenRows`
- `handleRestoreAll()`: clears `hiddenRows`
- Export the hook

### Step 2: Add `RowContextMenu` component to `table-utils.tsx`

- Radix `ContextMenu` wrapping a `<td>` element
- Single menu item: eye-off icon + "Hide Row"
- Reuse existing context menu styling from `SortHeader`

### Step 3: Update `TransposedTableCell` renderer

- Destructure `options` and `onOptionsChange` from props
- Call `useRowManagement(options || {}, onOptionsChange)`
- Filter `rows` through `hiddenRows` set
- Wrap each row header `<td>` in `RowContextMenu`
- Render `HiddenColumnsBar` above the table (compact, passing `hiddenRows` as `hiddenColumns`)

### Step 4: Tests

- `useRowManagement`: hide, restore, restore-all behavior
- `RowContextMenu`: renders context menu on right-click
- `TransposedTableCell`: rows hidden when in `hiddenRows`, restoration works

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` — add `useRowManagement` hook and `RowContextMenu` component
- `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` — integrate row hiding
- `analytics-web-app/src/lib/screen-renderers/__tests__/table-utils.test.tsx` — add tests for new hook and component

## Trade-offs

**Why not reuse `useColumnManagement` directly?**
It bundles sort logic (handleSort, handleSortAsc, handleSortDesc, sort-column clearing on hide) that doesn't apply to the transposed table. A dedicated `useRowManagement` is simpler and avoids dead code paths. The two hooks share the same pattern but different concerns.

**Why `hiddenRows` instead of `hiddenColumns`?**
Even though rows in the transposed view map to fields/columns in the data, using `hiddenRows` makes the config self-documenting for the transposed context and avoids confusion if both table types share config patterns.

**Why not add a generic label prop to `HiddenColumnsBar`?**
The component name and "Hidden:" label are already generic enough. We can pass `hiddenRows` as the `hiddenColumns` prop — the component just renders string pills. No rename needed for this first iteration.

## Testing Strategy

- Unit tests for `useRowManagement` hook (hide/restore/restoreAll)
- Unit tests for `RowContextMenu` (renders menu, triggers callback)
- Integration test: `TransposedTableCell` with `options.hiddenRows` set correctly hides rows and shows restoration bar

## Mockup

See `tasks/transposed_table_hide_rows_mockup.html` for an interactive HTML mockup demonstrating the behavior.
