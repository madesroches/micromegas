# Restore Log Paging Controls Plan

## Overview

The notebook Log cell's pagination bar (page nav + row-count + page-size selector) disappears entirely whenever the current query result fits on a single page — e.g. a 609-row result with `pageSize` at or above 1000. Once that happens there's no way to reduce the page size back down through the UI, since the control that would let you do that is the very thing that's hidden: a self-locking trap. This was initially suspected to be a regression from #1313 ("Log cell: add wrap-text toggle"), but that commit never touched pagination code. The actual cause is `PaginationBar`'s `totalPages <= 1` hide condition, added in #827 ("Hide pagination bar when data fits on a single page") for the reference-table (inline CSV) cell, and inherited by `LogCell`/`TableCell` since all three share the same component. This plan changes `PaginationBar` so the row-count text and page-size selector always show whenever there is data, while the page nav (prev/next/first/last buttons + page indicator) still hides when there's only one page — restoring the ability to change page size regardless of current row count.

## Current State

- `usePagination`/`PaginationBar` live in `analytics-web-app/src/lib/screen-renderers/pagination.tsx` and are shared verbatim by `LogCell.tsx:306`, `TableCell.tsx:182`, and `ReferenceTableCell.tsx:135` — each just renders `<PaginationBar pagination={pagination} />`.
- `PaginationBar` (`pagination.tsx:98-153`) opens with:
  ```ts
  if (totalRows === 0 || totalPages <= 1) return null
  ```
  (`pagination.tsx:102`). This hides the *entire* bar — both the centered nav (`first/prev/page-indicator/next/last`, lines 111-131) and the right-aligned row-count + page-size `<select>` (lines 134-150) — whenever `totalRows <= pageSize`.
- This condition was introduced in commit `10bab493e` ("Add reference table cell type for inline CSV data (#824) (#827)", specifically its "Hide pagination bar when data fits on a single page" sub-change), predating #1313 by months. It was a reasonable call for the reference-table cell, where pasted CSVs are typically small and a single-page dataset genuinely has nothing to page through.
- For `LogCell`/`TableCell`, though, this creates a trap: if a cell's persisted `options.pageSize` (e.g. `1000`, the largest of `PAGE_SIZE_OPTIONS = [50, 100, 250, 500, 1000]`) happens to be ≥ the current row count, the bar — including the page-size `<select>` — vanishes. There is no other UI to change page size, so the cell is stuck at that page size until the row count grows past it again.
- `usePagination` itself (`pagination.tsx:39-88`) is unaffected — `startRow`/`endRow`/`setPage`/`setPageSize` all compute correctly regardless of `totalPages`; the bug is purely in `PaginationBar`'s render-nothing guard.
- Confirmed via a throwaway RTL test rendering `LogCell` with 150 synthetic rows: `usePagination`/row-slicing/page navigation all work correctly post-#1313, ruling out a logic regression in `LogCell.tsx`/`log-utils.tsx` themselves.

## Design

Split the single hide condition into two independent pieces:

1. **Whole-bar hide** — only when there's truly nothing to show: `totalRows === 0`.
2. **Nav-only hide** — when `totalPages <= 1`, hide just the centered nav controls (buttons + page indicator), not the row-count/page-size section.

```tsx
export function PaginationBar({ pagination }: PaginationBarProps) {
  const { currentPage, totalPages, totalRows, startRow, endRow, setPage, setPageSize, pageSize } =
    pagination

  if (totalRows === 0) return null

  const showNav = totalPages > 1
  const isFirst = currentPage === 0
  const isLast = currentPage >= totalPages - 1

  return (
    <div className="grid grid-cols-[1fr_auto_1fr] items-center py-0.5 px-1 flex-shrink-0" onClick={(e) => e.stopPropagation()}>
      <div />
      {showNav ? (
        <div className="flex items-center gap-0.5">
          {/* existing First/Prev/indicator/Next/Last buttons, unchanged */}
        </div>
      ) : (
        <div />
      )}
      <div className="flex items-center justify-end gap-1 min-w-0">
        {/* existing row-count span + page-size <select>, unchanged */}
      </div>
    </div>
  )
}
```

- The grid is already `grid-cols-[1fr_auto_1fr]` with the nav as the centered `auto` column — swapping its content for an empty `<div />` when `!showNav` keeps the row-count/page-size column's right alignment unchanged (no layout shift beyond the centered slot going blank).
- No changes to `usePagination`, `DEFAULT_PAGE_SIZE`, or `PAGE_SIZE_OPTIONS`.
- No changes needed in `LogCell.tsx`, `TableCell.tsx`, or `ReferenceTableCell.tsx` — they only render `<PaginationBar pagination={pagination} />` and don't inspect its internals.
- Reference-table cells (the original motivation for hiding on a single page) keep the same "no nav clutter for a small pasted CSV" behavior; they additionally gain a visible page-size selector, which is a strict improvement (lets a user shrink the page size for a large pasted CSV without editing the notebook JSON) rather than a regression for that use case.

## Implementation Steps

1. **`analytics-web-app/src/lib/screen-renderers/pagination.tsx`**
   - Change the early-return guard at line 102 from `if (totalRows === 0 || totalPages <= 1) return null` to `if (totalRows === 0) return null`.
   - Add `const showNav = totalPages > 1` and wrap the existing nav `<div className="flex items-center gap-0.5">...</div>` block in `{showNav ? (...) : <div />}` so the grid's centered column always renders something (keeping the 3-column grid balanced whether or not nav is shown).
   - Leave the row-count/page-size `<div className="flex items-center justify-end gap-1 min-w-0">...</div>` block untouched.

2. Update the doc comment at the top of the file if it references "hidden when a single page" behavior (check for one before editing — none currently present per read).

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/pagination.tsx`

## Trade-offs

**Always show page-size selector vs. keep the original single-page hide**: keeping the original behavior avoids the self-locking trap only if users never let `pageSize` grow to meet or exceed their row count — not a safe assumption, since `1000` is a valid, easily-selected option and row counts naturally cross that threshold as data ages in/out of a time range. Always showing the row-count + page-size selector (dropping the nav only) directly fixes the trap with no downside beyond a few extra pixels of always-visible UI.

**Scope: shared `PaginationBar` fix vs. Log-only special case**: a Log-only fix (e.g. a bespoke always-visible footer just for `LogCell`) would avoid touching `TableCell`/`ReferenceTableCell` behavior, but duplicates UI that's otherwise identical across all three cells and leaves the same trap live for `TableCell`. Fixing the shared component is simpler and removes the trap everywhere it can occur, and per investigation the "hide entirely" behavior wasn't load-bearing for reference tables beyond avoiding nav clutter — which this plan preserves.

## Testing Strategy

- `yarn type-check` and `yarn lint` — no new errors.
- `yarn test` — no existing test asserts the old `totalPages <= 1` full-hide behavior (searched; none found), so no test updates expected, but re-run to confirm.
- Add a unit test in `pagination.tsx`'s test suite (create one if none exists — none found today) asserting:
  - `totalRows === 0` → `PaginationBar` renders nothing.
  - `totalRows > 0 && totalPages === 1` → row-count text and page-size `<select>` render; nav buttons (`First page`/`Previous page`/`Next page`/`Last page` titles) do not.
  - `totalPages > 1` → both nav and page-size selector render.
- Manual, in the running app (`./start_analytics_web.py`):
  - Add a Log cell whose query returns fewer rows than the current page size (e.g. set page size to 1000 for a ~600-row result) — confirm the row-count text and page-size dropdown are still visible, with no nav buttons.
  - Use the now-visible page-size selector to drop it to 100 — confirm nav buttons appear and pagination works normally.
  - Repeat for a Table cell and a Reference Table cell with a small dataset, confirming the same behavior.

## Open Questions

- None — scope and root cause confirmed via code inspection, a throwaway RTL test on `LogCell`, and the user's screenshots/row-count report.
