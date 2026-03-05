# Download CSV Menu Option for Notebook Cells

## Issue Reference
- GitHub Issue: [#900](https://github.com/madesroches/micromegas/issues/900)

## Overview

Add a "Download CSV" option to the existing context menu (three-dot dropdown) on notebook cells. When clicked, it converts the cell's Arrow Table result data to CSV and triggers a browser download.

## Current State

### Cell Context Menu
`CellContainer.tsx` (lines 186-252) renders a Radix UI `DropdownMenu` with items: Edit, Run from here, Auto-run, Duplicate, Delete. The menu appears on hover via the `MoreVertical` icon.

`CellContainer` does **not** currently receive cell result data — it only gets `name`, `type`, `status`, and callback props. The actual `data: Table[]` lives in `CellRendererProps` and is passed to the cell renderer component inside CellContainer's `children` slot.

### Data Flow
1. `NotebookRenderer.tsx` builds `CellRendererProps` via `buildCellRendererProps()` (notebook-cell-view.ts:186)
2. Cell state holds `data: Table[]` (notebook-types.ts:155)
3. Props are passed to `<CellRenderer>` inside `<CellContainer>` children
4. CellContainer itself never sees the data

### Existing Download Pattern
`perfetto-trace.ts` (lines 3-13) uses the standard Blob + createObjectURL + anchor click pattern for triggering browser downloads.

### CSV Dependencies
- `d3-dsv` v3.0.1 is already a dependency (used in `csv-to-arrow.ts` for CSV parsing)
- `d3-dsv` exports `csvFormat()` and `csvFormatRows()` for CSV generation

## Design

### Approach: Callback-based

Rather than passing the full `Table[]` data down into `CellContainer`, pass an `onDownloadCsv` callback. This keeps CellContainer lean — it doesn't need to know about Arrow tables. The callback is only provided when the cell has data, making the menu item conditional.

### Arrow Table to CSV Conversion

Create a utility function `arrowTableToCsv(table: Table): string` in a new file `arrow-to-csv.ts` alongside the existing `csv-to-arrow.ts`. This uses `d3-dsv`'s `csvFormatRows` for proper RFC 4180 quoting/escaping:

```typescript
import { csvFormatRows } from 'd3-dsv'
import type { Table } from 'apache-arrow'

export function arrowTableToCsv(table: Table): string {
  const fields = table.schema.fields
  const header = fields.map((f) => f.name)
  const rows: string[][] = []
  for (let i = 0; i < table.numRows; i++) {
    const row = table.get(i)
    rows.push(fields.map((f) => {
      const val = row?.[f.name]
      return val == null ? '' : String(val)
    }))
  }
  return csvFormatRows([header, ...rows])
}
```

### Download Trigger

Reuse the Blob + anchor pattern from `perfetto-trace.ts`:

```typescript
export function triggerCsvDownload(csvContent: string, filename: string): void {
  const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}
```

### CellContainer Changes

Add an optional `onDownloadCsv` callback prop:

```typescript
interface CellContainerProps {
  // ... existing props ...
  /** Download cell data as CSV */
  onDownloadCsv?: () => void
}
```

Add a menu item in the dropdown (after "Run from here", before "Duplicate"):

```typescript
{onDownloadCsv && (
  <DropdownMenu.Item onSelect={() => onDownloadCsv()}>
    <Download className="w-4 h-4" />
    Download CSV
  </DropdownMenu.Item>
)}
```

### NotebookRenderer Wiring

In `NotebookRenderer.tsx`, where `<CellContainer>` is rendered (around line 601), add the `onDownloadCsv` prop:

```typescript
onDownloadCsv={
  state.data.length > 0 && state.data[0].numRows > 0
    ? () => {
        const csv = arrowTableToCsv(state.data[0])
        triggerCsvDownload(csv, `${cell.name}.csv`)
      }
    : undefined
}
```

The same pattern applies for HG child cells rendered inside `HorizontalGroupCell.tsx`.

### HorizontalGroupCell Support

HG children do **not** use `CellContainer`. They render via `HgChildPane` (HorizontalGroupCell.tsx lines 113-271), which has its own `DropdownMenu` (lines 210-242) with "Edit cell" and "Remove from group" items. Add an `onDownloadCsv` callback to `HgChildPaneProps` and a "Download CSV" menu item to `HgChildPane`'s dropdown. The child cell state is already available in the parent render loop at line 376 as `cellStates[child.name]`.

## Implementation Steps

1. **Create `analytics-web-app/src/lib/screen-renderers/cells/arrow-to-csv.ts`** — `arrowTableToCsv()` function using `d3-dsv` csvFormatRows, and `triggerCsvDownload()` helper
2. **Update `CellContainer.tsx`** — Add `onDownloadCsv?: () => void` prop, add `Download` icon import from lucide-react, add menu item in dropdown
3. **Update `NotebookRenderer.tsx`** — Wire `onDownloadCsv` on both the regular cell `<CellContainer>` and the HG group `<CellContainer>` (HG parent doesn't have data, so skip it — only wire for non-HG cells)
4. **Update `HorizontalGroupCell.tsx`** — Add `onDownloadCsv` prop to `HgChildPaneProps`, add `Download` icon import, add "Download CSV" menu item to `HgChildPane`'s dropdown (lines 226-239), and wire the callback from the parent render loop using `cellStates[child.name].data[0]`

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/lib/screen-renderers/cells/arrow-to-csv.ts` | **NEW** — `arrowTableToCsv()` and `triggerCsvDownload()` |
| `analytics-web-app/src/components/CellContainer.tsx` | Add `onDownloadCsv` prop and menu item |
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | Wire `onDownloadCsv` callback for regular cells |
| `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | Add `onDownloadCsv` to `HgChildPaneProps`, add menu item to `HgChildPane`'s dropdown, wire from parent render loop |

## Trade-offs

### Callback prop vs. passing Table data to CellContainer
**Chosen: Callback.** CellContainer is a pure layout/UI component that doesn't import Arrow. Passing a callback keeps the dependency boundary clean and the component simple. The callback is only defined when data exists, naturally hiding the menu item for empty cells.

### New utility file vs. inline conversion
**Chosen: New file** (`arrow-to-csv.ts`) next to the existing `csv-to-arrow.ts`. They form a natural pair. The conversion logic is non-trivial enough to warrant its own file and unit tests.

### d3-dsv csvFormatRows vs. manual CSV generation
**Chosen: d3-dsv.** Already a dependency. Handles RFC 4180 edge cases (quoting, escaping commas/newlines in values) correctly.

### Download `data[0]` only vs. all tables
**Chosen: `data[0]` only.** Every cell type produces at most one result table. Consistent with how all cell renderers access data (`const table = data[0]`).

## Testing Strategy

### Unit Tests
- Test `arrowTableToCsv()` with a simple Arrow table — verify headers and rows are correct
- Test with empty table (0 rows) — verify only header row
- Test with values containing commas, quotes, newlines — verify proper CSV escaping

### Test Mock Updates
- **`analytics-web-app/src/components/__tests__/CellContainer.test.tsx`** — Add `Download` to the lucide-react mock (lines 9-20) since CellContainer will now import it
- **`analytics-web-app/src/lib/screen-renderers/cells/__tests__/HorizontalGroupCell.test.tsx`** — Add `Download` to the lucide-react mock since HgChildPane will now import it

### Integration
- `yarn build` — verify no type errors
- `yarn lint` — verify lint passes
- Existing CellContainer and HorizontalGroupCell tests still pass (after mock updates above)

### Manual
- Open a notebook with table/log cells that have data
- Click the three-dot menu — verify "Download CSV" appears
- Click "Download CSV" — verify browser downloads a .csv file with correct content
- Verify menu item does NOT appear on cells with no data (markdown, empty results)
- Verify it works for cells inside HG groups
