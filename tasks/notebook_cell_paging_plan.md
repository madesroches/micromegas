# Notebook Cell Paging Plan

## Overview

Add client-side pagination controls to table and log cells in notebooks. Currently both cell types render all rows from the query result at once, which degrades performance for large result sets. Pagination slices the already-fetched Arrow Table data into pages and provides navigation controls.

## Status: Implemented

All steps are complete. The pagination hook, component, and cell integrations are in place.

## Design

### Approach: Client-side slicing of fetched data

The data is already fully fetched as an Arrow Table. Pagination controls which slice of rows gets rendered, without re-executing queries. This avoids changes to the execution pipeline, WASM engine, or data streaming.

Page state (`currentPage`) is stored in component state (not in cell options), since page position is ephemeral UI state that shouldn't persist across sessions or trigger re-execution. Page size is stored in cell options so it persists with the notebook config.

### Shared pagination hook

`usePagination` hook encapsulates page logic:

```typescript
interface PaginationState {
  currentPage: number       // 0-indexed
  pageSize: number
  totalRows: number
  totalPages: number
  startRow: number          // inclusive, 0-indexed
  endRow: number            // exclusive
  setPage: (page: number) => void
  setPageSize: (size: number) => void
}

function usePagination(totalRows: number, pageSize: number, onPageSizeChange: (size: number) => void): PaginationState
```

The hook clamps `currentPage` to the valid range when `totalRows` or `pageSize` changes (e.g., if you're on page 5 and re-execute a query that returns fewer rows, the page adjusts to the last valid page rather than always resetting to 0).

### Shared pagination controls component

`PaginationBar` renders:

```
[|<] [<] Page 1 of 42 [>] [>|]   Rows 1-100 of 4,200   [100 v] rows/page
```

- First/Prev/Next/Last buttons using lucide-react chevron icons (disabled at boundaries)
- Current page / total pages display
- Row range display (e.g., "Rows 1–100 of 4,200")
- Page size dropdown with options: 50, 100, 250, 500, 1000
- Hidden when totalRows === 0

Styled with Tailwind to match existing compact cell UI (text-xs/text-[11px], theme colors, border, bg-app-card).

### Default page size

100 rows. Stored as `pageSize` in cell options so users can adjust per-cell and it persists.

## Files

| File | Action | Status |
|------|--------|--------|
| `analytics-web-app/src/lib/screen-renderers/pagination.tsx` | **Created** — shared hook and component | Done |
| `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` | **Modified** — integrated pagination | Done |
| `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx` | **Modified** — integrated pagination | Done |
| `tasks/mockups/notebook_paging_mockup.html` | **Created** — HTML/CSS mockup of all pagination states | Done |

## Implementation Details

### pagination.tsx

- `usePagination` hook: manages `currentPage` in local state, computes `startRow`/`endRow`, clamps page on data/size changes
- `PaginationBar` component: renders nav buttons (First/Prev/Next/Last with lucide-react icons), page info, row range, page size dropdown
- `NavButton` internal component: styled 28px square button with disabled state
- Constants: `DEFAULT_PAGE_SIZE = 100`, `PAGE_SIZE_OPTIONS = [50, 100, 250, 500, 1000]`

### TableCell integration

- Reads `pageSize` from `options` (defaults to `DEFAULT_PAGE_SIZE`)
- Creates a sliced `TableData` wrapper that maps indices through `pagination.startRow`:
  ```typescript
  const slicedData = {
    numRows: pagination.endRow - pagination.startRow,
    get: (index: number) => data.get(pagination.startRow + index),
  }
  ```
- Passes `slicedData` to `<TableBody>` instead of raw `data`
- Renders `<PaginationBar>` after the scroll container, inside the flex column wrapper
- Layout: flex column with `h-full`, scroll area is `flex-1 overflow-auto min-h-0`, pagination bar is `flex-shrink-0`

### LogCell integration

- Reads `pageSize` from `options` (defaults to `DEFAULT_PAGE_SIZE`)
- Accepts `options` and `onOptionsChange` props (passed via `getRendererProps`)
- Slices the materialized `rows` array: `rows.slice(pagination.startRow, pagination.endRow)`
- Renders `<PaginationBar>` after the scroll container
- `logMetadata.getRendererProps` passes `options: (config as QueryCellConfig).options`

## Trade-offs

### Client-side vs. server-side pagination

**Chosen: Client-side.** The data is already fully fetched and stored as an Arrow Table in memory. Client-side slicing is trivial (O(1) per page via index access), requires no backend changes, and keeps the WASM cross-cell reference system working (cells downstream can still reference the full result set). Server-side pagination (LIMIT/OFFSET in SQL) would require re-executing queries on every page change, add latency, and break the WASM engine's table registration (which expects complete results).

### Page state in options vs. component state

**Chosen: Component state for currentPage, options for pageSize.** Page position is ephemeral — you don't want to reopen a notebook and be on page 37. Page size is a user preference that should persist.

### Pagination bar position

**Chosen: flex-shrink-0 inside a flex column container.** The cell uses `flex flex-col h-full`, the scroll area is `flex-1 overflow-auto min-h-0`, and the pagination bar sits below it with `flex-shrink-0`. This keeps the bar always visible at the bottom without needing sticky positioning.

### Page clamping vs. reset on data change

**Chosen: Clamp to valid range.** When `totalRows` or `pageSize` changes, the hook clamps `currentPage` to `[0, maxPage]` rather than always resetting to page 0. This preserves the user's approximate position when possible (e.g., changing page size from 100 to 50 keeps you near the same data).

## Mockup

`tasks/mockups/notebook_paging_mockup.html` — open in a browser to see all pagination states:
1. Table cell, middle page (page 3 of 42)
2. Table cell, first page (first/prev disabled)
3. Log cell with pagination (page 1 of 85)
4. Table cell, few rows (single page, all nav disabled)
5. Log cell, last page (next/last disabled)
6. Isolated pagination bar close-up

## Testing Strategy

1. **Manual testing**:
   - Create a table cell with a query returning >100 rows — verify pagination appears
   - Navigate through pages — verify correct rows displayed
   - Change page size — verify page resets to 0 and new size persists
   - Verify sort still works with pagination (re-execution resets data, page clamps)
   - Verify log cell pagination with same scenarios
   - Verify cells with <100 rows show pagination bar but all on one page
   - Verify empty result sets show "No data" message (no pagination bar)

2. **Edge cases**:
   - Exactly `pageSize` rows (1 page, no next button)
   - 0 rows (no pagination bar shown)
   - Last page with fewer rows than `pageSize`
   - Re-executing query clamps page to valid range
