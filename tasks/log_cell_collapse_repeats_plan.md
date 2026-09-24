# Log Cell: Collapse Consecutive Repeated Lines Plan

Issue: https://github.com/madesroches/micromegas/issues/1557

## Overview

The notebook Log cell renders one line per result row. A source that logs the same message in a tight loop (e.g. a client retrying a failing query on every auto-refresh tick) fills the panel with dozens of lines that differ only by timestamp, pushing everything else off screen. This adds a Grafana-style "dedup" mode to `LogCell`: consecutive rows equal on every column except `time` (and an optional per-cell list of ignored columns) render as one line with a `×N` badge. Clicking the badge expands the run to show each row. Rendering-only change: no query, data model, or SQL-surface change.

## Current State

- `LogCell` (`analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx:31`) paginates **raw rows**: `usePagination(numRows, pageSize, …)` (`:47`), then renders `table.get(rowIdx)` for each row in `[startRow, endRow)` (`:263-309`). Each row has an absolutely positioned copy button (`:272-281`) and one `renderLogColumn` span per column with a `LogDivider` between columns.
- Auto column widths come from `computeFlexWidths(table, columns, startRow, endRow)` (`log-utils.tsx:211`), measured over the visible raw-row range only.
- Per-cell persistence goes through `options` / `onOptionsChange` (`pageSize`, `columnWidths`, `wrapText`). The footer holds the pagination bar, a conditional "Reset widths" button and the "Wrap text" toggle (`LogCell.tsx:311-333`). `wrapText` defaults to `true` when unset (`:49`).
- The same "format a log value to its display string" switch appears twice, in `formatRowForCopy` (`log-utils.tsx:190-209`) and `computeFlexWidths` (`:220-236`), with a third copy inside `renderLogColumn`.
- `LogCellEditor` (`LogCell.tsx:342`) only exposes the SQL editor. `CellEditorProps.availableColumns` (`cell-registry.ts:90`) already carries the cell's own result column names (populated in `NotebookRenderer.tsx:846`), but the Log editor doesn't use it yet.
- `usePagination` / `PaginationBar` (`pagination.tsx`) are generic over a `totalRows` count. They know nothing about Arrow and are shared with `TableCell`.
- The deprecated standalone `LogRenderer.tsx` is out of scope, as in previous log-cell plans.

## Design

### Grouping rule

A **group** is a maximal run of consecutive rows `[start, end)` where every *compared* column is equal to the same column in row `start`. Compared columns = all columns minus the **ignore set**:

- every column whose `kind === 'time'` (always ignored, since the timestamp is the thing that differs), plus
- `options.collapseIgnoreColumns: string[]` (default `[]`): user-chosen extra columns such as a request id or `process_id`.

If the ignore set covers every column, grouping is skipped (each row is its own group). Otherwise any two rows would compare equal and the whole result would collapse into one line.

Value equality per column: `a === b` fast path (covers strings, numbers, bigints, booleans, null). Otherwise, if either side is a non-primitive (Arrow struct/list/`Uint8Array`/Date), compare `formatLogValue(col, a) === formatLogValue(col, b)`. "Identical" then means "renders identically", which is what the user sees. Primitive values that differ by `===` are never equal (no formatting pass for them).

Grouping compares against the group's first row, not the previous row. Both give the same result for exact equality; comparing against the first row is simpler to reason about.

### `log-utils.tsx` additions

```ts
export interface LogRowGroup { start: number; end: number }   // raw row indices, end exclusive

export function formatLogValue(col: LogColumn, value: unknown): string
export function groupConsecutiveRows(
  table: Table,
  columns: LogColumn[],
  ignore: ReadonlySet<string>,
): LogRowGroup[]
```

- `formatLogValue` holds the existing `time`/`level`/`target`/default switch. `formatRowForCopy` and `computeFlexWidths` are rewritten to call it (DRY; behavior unchanged).
- `groupConsecutiveRows` reads values through `table.getChild(col.name)?.get(i)` for compared columns only. Going through column vectors avoids building a row proxy per row. It is O(rows × compared columns) with early exit on the first mismatching column. Row counts are capped by the cell's SQL `LIMIT`, so one linear pass per result is cheap, and it is memoized (below).

### Pagination over groups

With collapsing on, pagination counts **display lines** (groups), not raw rows:

```
groups      = useMemo(() => collapse ? groupConsecutiveRows(table, columns, ignoreSet)
                                     : singletonGroups(numRows), [table, columns, ignoreSet, collapse])
pagination  = usePagination(groups.length, pageSize, …)
pageGroups  = groups.slice(pagination.startRow, pagination.endRow)
rawStart    = pageGroups[0]?.start ?? 0
rawEnd      = pageGroups.at(-1)?.end ?? 0
autoWidths  = computeFlexWidths(table, columns, rawStart, rawEnd)   // unchanged signature
```

- `usePagination` / `PaginationBar` need no change: they just receive a smaller total. The "1–100 of N" label then reads in lines. That is accurate for what the page shows. The raw row count is not surfaced in v1 (see Trade-offs).
- A group never straddles a page boundary, so a run always shows its full count on one line.
- `computeFlexWidths` over the raw range `[rawStart, rawEnd)` covers representative rows *and* members of any expanded group. Hidden members are identical on every compared column, so measuring them does not change the width of those columns.
- `ignoreSet` is built with `useMemo` from `columns` + `collapseIgnoreColumns`, keyed on a joined string of the configured names so a fresh array from `options` does not invalidate the grouping memo on every render.

### Rendering

- Pull the row JSX (copy button + columns + dividers) out of the page map into a local `renderRow(rowIdx, stripe, trailing?)` closure, so representative rows and expanded member rows share one path.
- Representative row of a group with `count > 1` gets a trailing `RepeatBadge` (Option A mockup): a small `button` pill `×{count}` placed after the last column, `flex-none ml-2 self-start`, styled with `accent-link` tones.
  - `title`: `"{count} identical rows, {time of last row} → {time of first row}"` when a `time`-kind column exists (using `formatLocalTime`), else `"{count} identical rows"`. The two times are ordered by row position, not value, so the text reads correctly for both `ORDER BY time DESC` and `ASC`. For DESC, the first row of the run is the most recent.
  - `aria-expanded`, `aria-label="Show {count} repeated rows"`.
- **Expand/collapse:** `expandedGroups: ReadonlySet<number>` state keyed by the group's `start` row index. When expanded, rows `start+1 … end-1` render directly after the representative row with a subtle tint (`bg-accent-link/5`), no badge, and the same stripe as their representative. Every raw row keeps its own copy button, so a copy or pin works on any member. The expanded set is cleared when `groups` changes (new result, ignore-list edit, toggle), using the same render-time "previous value" reset pattern already used for `copiedRowIdx` (`LogCell.tsx:209-213`). Expanded state is ephemeral and not persisted.
- Stripe alternation (`i % 2`) is computed per **group** on the page, not per raw row.
- Copy on a collapsed line copies the representative row (unchanged `formatRowForCopy`). The count is not appended, so the clipboard text stays a faithful log line.

### Toggle and ignore-list UI

- **Footer toggle** "Collapse repeats", placed before "Wrap text" and styled the same way (`aria-pressed`, accent when on). Icon: `ChevronsDownUp` from `lucide-react`. Persisted as `options.collapseRepeats: boolean`; unset means **on** (same default convention as `wrapText`).
- **Editor section** in `LogCellEditor`, below the SQL editor: "Ignore when collapsing repeats". It renders one toggle chip per name in `availableColumns`. `time`-kind columns are shown checked and disabled with the hint "always ignored". Toggling a chip writes `onChange({ ...logConfig, options: { ...logConfig.options, collapseIgnoreColumns } })`. Names in `collapseIgnoreColumns` that no longer exist in the result are kept as-is and simply don't match. If `availableColumns` is empty (cell not run yet), the section shows "Run the query to choose columns".

## Mockups

- `tasks/log_cell_collapse_repeats_mockups/option-a-trailing-badge.html`: **chosen**. `×N` pill at the end of the collapsed line; rows without repeats are pixel-identical to today. Interactive: click a pill to expand, toggle "Collapse repeats" in the footer.
- `tasks/log_cell_collapse_repeats_mockups/option-b-leading-gutter.html`: Grafana-style count gutter before the timestamp. Counts line up for scanning, but it costs ~44px on every row and shifts every column when the toggle flips.

Option A is chosen because it adds nothing to rows that don't repeat.

## Implementation Steps

1. **`log-utils.tsx`**
   - Add `formatLogValue(col, value)`; rewrite `formatRowForCopy` and `computeFlexWidths` to use it.
   - Add `LogRowGroup` and `groupConsecutiveRows(table, columns, ignore)`, including the "everything ignored → singleton groups" guard.
2. **`LogCell.tsx` renderer**
   - Read `collapseRepeats` (default `true`) and `collapseIgnoreColumns` (default `[]`) from `options`; build `ignoreSet` and `groups`.
   - Paginate over `groups.length`; derive `rawStart`/`rawEnd` for `computeFlexWidths`.
   - Extract `renderRow`; render the page as groups plus expanded members; add `expandedGroups` state with its reset-on-`groups`-change.
   - Add `RepeatBadge` (local component in `LogCell.tsx`) and the "Collapse repeats" footer toggle.
3. **`LogCell.tsx` editor**
   - Destructure `availableColumns` in `LogCellEditor`; add the ignore-columns chip section writing `options.collapseIgnoreColumns`.
4. **Tests** (see Testing Strategy).
5. **Docs** (see Documentation), plus a `CHANGELOG.md` Unreleased entry.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/__tests__/log-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/LogCell.test.tsx` (new)
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `CHANGELOG.md`

## Trade-offs

- **Group the whole result, then paginate groups, vs. group within the current raw-row page.** Grouping within the page keeps the scan page-bounded. But a page of 100 raw rows could shrink to 3 lines, so the user would page through almost-empty pages to find distinct lines, and a run crossing a page boundary would split into two badges with wrong counts. Grouping once over the whole result costs one linear pass with column-vector reads, memoized per result. Rendering and width measurement stay page-bounded, which is what pagination is for (bounding DOM rows).
- **Default on vs. off.** Grafana defaults dedup off. Here it defaults on because the issue is about the default view being flooded, and the badge makes the collapse visible and reversible in one click. This matches the `wrapText` precedent. The accepted cost: existing notebooks will show fewer lines on next open.
- **Time columns always ignored vs. part of the configurable list.** Comparing `time` would defeat the feature, since the timestamp is exactly what differs. Keeping it out of the list removes one way to misconfigure it. Users who want every row can turn the toggle off.
- **Equality on raw values with a formatted fallback vs. always formatted strings.** Formatting every compared cell of the full result would be the costliest part of the pass. The `===` fast path handles the common primitive columns, and formatting only runs for object-valued cells, where raw identity is meaningless.
- **Ignore list in the editor panel vs. the `LogDivider` context menu.** The divider menu is attached to the divider *after* a column, so the last column (usually `msg`) has no menu, and the list would be spread across per-column menus. One chip list in the editor shows the whole set in one place.
- **Pagination label in lines only.** Showing "N lines · M rows" would need a new optional prop on the shared `PaginationBar`. That is left out until someone asks. The badges already show where rows were folded.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` (Log section, ~line 243):
  - Options table: add `collapseRepeats` (boolean, default on) and `collapseIgnoreColumns` (string[]). While there, add the undocumented existing `wrapText` and `columnWidths` options.
  - Short paragraph describing collapsing: which columns are compared, the `×N` badge, click-to-expand, and that pagination counts collapsed lines.
  - Fix the stale "copy the full row as JSON" sentence (the copy is tab-delimited, per `formatRowForCopy`).
- `CHANGELOG.md`: Unreleased entry, "Log cell collapses consecutive repeated lines" (#1557).

## Testing Strategy

All no-DB unit tests with Vitest + React Testing Library. Arrow tables are built in-test with `tableFromArrays`/`makeTable`, following the existing cell tests (`cells/__tests__/TransposedTableCell.test.tsx` mock-props pattern).

`__tests__/log-utils.test.ts`:
- `formatLogValue`: one case per kind (time/level numeric and string/target/generic). Existing `formatRowForCopy` and `computeFlexWidths` tests keep passing, which guards the refactor.
- `groupConsecutiveRows`:
  - rows differing only in `time` form one group; a differing `msg` breaks the run
  - non-consecutive duplicates (A, B, A) stay three groups
  - a column in `ignore` is skipped (rows differing only in `request_id` group when it's ignored, and don't when it isn't)
  - every column ignored → singleton groups
  - null vs. null equal; null vs. `''` not equal
  - object-valued column (e.g. struct/list) equal by formatted value
  - empty table → `[]`; a single row → one group

`cells/__tests__/LogCell.test.tsx` (new):
- 5 identical rows + 1 distinct → 2 rendered lines, badge `×5` present on the first
- clicking the badge renders all 5 rows (`aria-expanded` flips); clicking again collapses
- `options.collapseRepeats: false` → 6 lines, no badge; clicking the footer toggle calls `onOptionsChange` with `collapseRepeats` flipped
- `collapseIgnoreColumns: ['request_id']` groups rows that differ only in `request_id`
- pagination counts groups: with `pageSize: 50` and 60 rows forming 2 groups, no pagination bar is shown
- badge `title` contains the formatted first/last timestamps
- `LogCellEditor`: toggling a chip calls `onChange` with the updated `options.collapseIgnoreColumns`; a `time` chip is disabled

`yarn lint`, `yarn type-check`, `yarn test` from `analytics-web-app/`.

## Manual Verification

Visual placement and feel only. The logic is covered above, and a misplaced badge is obvious on sight.

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`, open http://127.0.0.1:3000, and create a notebook with a Log cell over `log_entries` for a window containing a repeated warning (or `SELECT` a `VALUES` list with repeated rows).
2. Expected: repeated lines collapse with a trailing `×N` pill aligned with the first text line in both wrap on and wrap off modes. Expanding a run shows tinted member rows, and column dividers stay aligned.
3. Flip "Collapse repeats" off, reload the page, and expect it to stay off.

## Decisions

- Badge placement: Option A (trailing `×N` pill), chosen by the user over Option B (leading gutter).
