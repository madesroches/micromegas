# Admin List Pages Table Scrolling Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1618

## Overview

Several admin list pages in the analytics web app clip their table when it holds more rows than
fit in the viewport. Neither the page nor the table scrolls, so the rows below the fold (and the
API-key pagination footer) can't be reached. This plan adds a small shared `TableFrame` component
that makes the table region its own scroll container and switches every affected page to it.

## Current State

`PageLayout` (`analytics-web-app/src/components/layout/PageLayout.tsx:38`) renders
`<main className="flex-1 overflow-auto flex flex-col">`, so `<main>` is intended to be the page's
scroll container. Each admin page puts a fixed-height column inside it:

```tsx
<div className="p-6 flex flex-col h-full">
  ...breadcrumb, header, banners...
  <div className="border border-theme-border rounded-lg overflow-hidden">
    <table>...</table>
  </div>
</div>
```

A flex item's automatic minimum size is `min-height: auto` (content height) **unless it has a
non-`visible` overflow, in which case the automatic minimum becomes 0**. The `overflow-hidden`
table wrapper (there to clip the table to the rounded corners) can therefore shrink to whatever
height the `h-full` column has left over. It clips its rows, and the column never grows past
`<main>`, so `<main>` has nothing to scroll.

Affected wrappers (direct children of the `h-full` column, no inner scroller):

| File | Line | Notes |
|---|---|---|
| `src/components/ApiKeysAdminPage.tsx` | 306 | Shared by `/admin/ingestion-keys` and `/admin/analytics-keys`; the Previous/Next pagination footer sits *inside* the wrapper (lines 375-391) and is clipped too |
| `src/routes/MapsPage.tsx` | 264 | |
| `src/routes/DataSourcesPage.tsx` | 283 | |
| `src/routes/QueryDenyListPage.tsx` | 367 | `overflow-hidden overflow-x-auto`: horizontal scroll works, but vertical clipping is identical |
| `src/routes/ImportScreensPage.tsx` | 288 | `renderStep2()` returns a fragment, so the wrapper is a direct child of the page column; the Back/Import footer after it stays visible, but the table rows are clipped |

Pages that already work, for reference:
- `ExportScreensPage.tsx:178`: `border ... rounded-lg overflow-hidden flex-1 overflow-y-auto` with
  `<thead className="bg-app-panel sticky top-0">`. The working precedent for an in-page table
  scroller.
- `ProcessesPage.tsx:345`: `flex-1 overflow-auto` with a sticky thead.
- `ScreensPage.tsx:330/388`, `GroupsPage.tsx:518`: `flex-1 min-h-0` / `overflow-auto` regions.
- `AudienceAccessPage.tsx:736`: the list wrapper is `space-y-4` with no overflow, so it keeps its
  content height and `<main>` scrolls. The per-group `overflow-hidden` cards sit inside that
  non-flex wrapper, so they aren't subject to flex shrinking.

## Design

### `TableFrame` component

New file `src/components/TableFrame.tsx`:

```tsx
interface TableFrameProps {
  children: React.ReactNode   // the <table>
  footer?: React.ReactNode    // pinned below the scroll area (e.g. pagination)
}

export function TableFrame({ children, footer }: TableFrameProps) {
  return (
    <div className="min-h-0 flex flex-col border border-theme-border rounded-lg overflow-hidden">
      <div className="min-h-0 overflow-auto">{children}</div>
      {footer}
    </div>
  )
}
```

Layout behaviour:
- **No `flex-1`**, on purpose: a short table keeps its content height, so the bordered box doesn't
  stretch to the bottom of the page (unlike `ProcessesPage`). When the rows exceed the space left
  in the `h-full` column, the frame shrinks (`min-h-0`) and the inner div scrolls.
- The inner scroller handles both axes (`overflow-auto`), which covers
  `QueryDenyListPage`'s existing horizontal-scroll need.
- `footer` is a sibling of the scroller. Its min-height stays `auto`, so it never shrinks and
  stays pinned at the bottom of the frame, always reachable. The outer `overflow-hidden` still
  clips corners for both.

### Sticky header

Each migrated table gets `sticky top-0` on its `<thead>` (all of them already use the opaque
`bg-app-panel` background), following `ExportScreensPage`. This keeps the column names visible
while the rows scroll.

### Page changes

Replace each affected wrapper `<div className="border border-theme-border rounded-lg overflow-hidden ...">`
with `<TableFrame>`. For `ApiKeysAdminPage`, move the existing pagination `<div>` (still behind the
same `(offset > 0 || keys.length === pageSize)` condition) into the `footer` prop. Its markup stays
the same, including the `border-t` separator.

## Implementation Steps

1. Create `src/components/TableFrame.tsx` as above.
2. `src/components/ApiKeysAdminPage.tsx`: wrap the table in `<TableFrame footer={...}>`, passing
   the pagination block as `footer`. Add `sticky top-0` to the `<thead>`.
3. `src/routes/MapsPage.tsx`, `src/routes/DataSourcesPage.tsx`, `src/routes/QueryDenyListPage.tsx`,
   `src/routes/ImportScreensPage.tsx`: replace the wrapper with `<TableFrame>` and add
   `sticky top-0` to each `<thead>`. For `QueryDenyListPage`, drop the now-redundant
   `overflow-x-auto`, since the frame's inner scroller covers it.
4. Add `src/components/__tests__/TableFrame.test.tsx` (see Testing Strategy).
5. Run `yarn test`, `yarn lint`, and `yarn type-check` (or the project's equivalents) in
   `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/components/TableFrame.tsx` (new)
- `analytics-web-app/src/components/__tests__/TableFrame.test.tsx` (new)
- `analytics-web-app/src/components/ApiKeysAdminPage.tsx`
- `analytics-web-app/src/routes/MapsPage.tsx`
- `analytics-web-app/src/routes/DataSourcesPage.tsx`
- `analytics-web-app/src/routes/QueryDenyListPage.tsx`
- `analytics-web-app/src/routes/ImportScreensPage.tsx`

## Trade-offs

- **Whole-page scroll (`shrink-0` on each wrapper)** vs **in-frame scroll (chosen)**. `shrink-0`
  is a one-class fix, but the page header scrolls away and the table header scrolls off with it.
  In-frame scrolling with a sticky thead matches the list pages that already work
  (`ExportScreensPage`, `ProcessesPage`, `ScreensPage`) and keeps pagination pinned.
- **Shared component vs. inline classes per page.** The same broken wrapper was copy-pasted into
  five places, which is how the bug spread. A single `TableFrame` fixes them in one place and
  gives future admin tables a correct default. The working pages (`ExportScreensPage`,
  `ProcessesPage`) aren't migrated. They already scroll correctly, and `ExportScreensPage`'s
  `flex-1` stretch is a deliberate layout choice for its selection list.
- **`flex-1` on the frame** was rejected: it would stretch a two-row table's border to the bottom
  of the viewport.

## Documentation

None. This is internal UI layout, with no user-facing docs in `mkdocs/` that describe it.

## Testing Strategy

jsdom does no layout, so a unit test can't observe whether rows are clipped. Automated coverage
pins the structure that produces correct layout:

- `TableFrame.test.tsx`:
  - The `children` render inside an element carrying `overflow-auto` and `min-h-0`, and the outer
    frame carries `min-h-0` and does **not** carry `flex-1`.
  - When `footer` is given, it renders as a sibling of the scroll container, not inside it. This
    guards against the footer scrolling away. When omitted, nothing extra renders.
- The existing `ApiKeysAdminPage.test.tsx` pagination tests (Previous/Next buttons found by role)
  keep passing, which confirms the footer still renders under its condition after the move.

## Manual Verification

Actual scrolling depends on the browser's flex layout, which jsdom can't compute. A regression
would also be obvious the next time anyone opens one of these pages with a long list.

1. Start the monolith: `python3 local_test_env/ai_scripts/start_services.py --monolith`, then open
   http://127.0.0.1:3000.
2. As an admin, mint enough ingestion keys (or shrink the browser window) that the table exceeds
   the viewport. On `/admin/ingestion-keys`, expect the table body to scroll with the header row
   staying visible, and the last row plus the pagination footer to be reachable.
3. Repeat with a short window on `/admin/analytics-keys`, `/admin/maps`, `/admin/data-sources`,
   `/admin/query-deny-list`, and Import Screens step 2. Expect the same behaviour, with horizontal
   scroll still working on the deny-list page at a narrow width.
4. With only one or two rows, expect the frame to hug its content rather than stretch to the
   bottom of the page.
