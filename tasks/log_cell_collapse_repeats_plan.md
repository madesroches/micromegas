# Log Cell: Collapse Consecutive Repeated Lines Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1557

## Overview

The notebook Log cell renders one line per result row. A source that logs the same message in a tight loop (e.g. a client retrying a failing query on every auto-refresh tick) fills the panel with dozens of lines that differ only by timestamp, pushing everything else off screen. This adds a Grafana-style "dedup" mode to `LogCell`: consecutive rows equal on every column except `time` (and an optional per-cell list of ignored columns) render as one line with a `×N` badge. Clicking the badge expands the run to show each row. Rendering-only change: no query, data model, or SQL-surface change.

## Current State

- `LogCell` (`analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx:31`) paginates **raw rows**: `usePagination(numRows, pageSize, …)` (`:47`), then renders `table.get(rowIdx)` for each row in `[startRow, endRow)` (`:263-309`). Each row has an absolutely positioned copy button (`:272-281`) and one `renderLogColumn` span per column with a `LogDivider` between columns.
- Auto column widths come from `computeFlexWidths(table, columns, startRow, endRow)` (`log-utils.tsx:211`), measured over the visible raw-row range only.
- Per-cell persistence goes through `options` / `onOptionsChange` (`pageSize`, `columnWidths`, `wrapText`). The footer holds the pagination bar, a conditional "Reset widths" button and the "Wrap text" toggle (`LogCell.tsx:311-333`). `wrapText` defaults to `true` when unset (`:49`).
- The same "format a log value to its display string" switch appears twice, in `formatRowForCopy` (`log-utils.tsx:190-209`) and `computeFlexWidths` (`:220-236`), with a third copy inside `renderLogColumn`.
- `LogCellEditor` (`LogCell.tsx:342`) only exposes the SQL editor. `CellEditorProps.availableColumns` (`cell-registry.ts:90`) already carries the cell's own result column names (populated in `NotebookRenderer.tsx:846`), but the Log editor doesn't use it yet. For a Log cell nested in a horizontal group, `HgEditorPanel` (`NotebookRenderer.tsx:145-179`, render at ~249) doesn't take or forward `availableColumns` to `HorizontalGroupCellEditor`, even though that editor already accepts and forwards the prop to `ChildEditorView` / `meta.EditorComponent` (`HorizontalGroupCell.tsx:425-471`, 393).
- `usePagination` / `PaginationBar` (`pagination.tsx`) are generic over a `totalRows` count. They know nothing about Arrow and are shared with `TableCell`.
- The deprecated standalone `LogRenderer.tsx` is out of scope except for the `computeFlexWidths` call-site update, as in previous log-cell plans.

## Design

### Grouping rule

A **group** is a maximal run of consecutive rows `[start, end)` where every *compared* column is equal to the same column in row `start`. Compared columns = all columns minus the **ignore set**:

- every column whose `kind === 'time'` (always ignored, since the timestamp is the thing that differs), plus
- `options.collapseIgnoreColumns: string[]` (default `[]`): user-chosen extra columns such as a request id or `process_id`.

If the ignore set covers every column, grouping is skipped (each row is its own group). Otherwise any two rows would compare equal and the whole result would collapse into one line.

Value equality per column: `a === b` fast path (covers strings, numbers, bigints, booleans, null). Otherwise, if either side is a non-primitive (Arrow struct/list/`Uint8Array`/Date), compare `formatLogValue(col, a) === formatLogValue(col, b)`. "Identical" then means "renders identically", which is what the user sees. Primitive values that differ by `===` are never equal (no formatting pass for them).

### `log-utils.tsx` additions

```ts
export interface LogRowGroup { start: number; end: number }   // raw row indices, end exclusive

export function formatLogValue(col: LogColumn, value: unknown): string
export function groupConsecutiveRows(
  table: Table,
  columns: LogColumn[],
  ignore: ReadonlySet<string>,
): LogRowGroup[]
export function range(start: number, end: number): number[]
export function singletonGroups(n: number): LogRowGroup[]
```

- `formatLogValue` holds the existing `time`/`level`/`target`/default switch. `formatRowForCopy` and `computeFlexWidths` are rewritten to call it (DRY; behavior unchanged). `renderLogColumn`'s third copy of the switch (`log-utils.tsx:142-187`) also collapses to a call to `formatLogValue` for the display string, keeping its per-kind JSX styling (className/title/color) around that string.
- `groupConsecutiveRows` reads values through `table.getChild(col.name)?.get(i)` for compared columns only. Going through column vectors avoids building a row proxy per row. It is O(rows × compared columns) with early exit on the first mismatching column. Row counts are capped by the cell's SQL `LIMIT`, so one linear pass per result is cheap, and it is memoized (below).

### Pagination over groups

With collapsing on, pagination counts **display lines** (groups), not raw rows:

```
groups        = useMemo(() => collapse ? groupConsecutiveRows(table, columns, ignoreSet)
                                       : singletonGroups(numRows), [table, columns, ignoreSet, collapse])
pagination    = usePagination(groups.length, pageSize, …)
pageGroups    = groups.slice(pagination.startRow, pagination.endRow)
displayedRows = pageGroups.flatMap((g) => expandedGroups.has(g.start)
                  ? range(g.start, g.end)   // expanded: every member
                  : [g.start])              // collapsed: representative row only
autoWidths    = computeFlexWidths(table, columns, displayedRows)   // new signature: explicit row indices
```

- `usePagination` / `PaginationBar` need no change: they just receive a smaller total. The "1–100 of N" label then reads in lines.
- A group never straddles a page boundary, so a run always shows its full count on one line.
- `computeFlexWidths` takes the explicit list of raw row indices actually rendered (representative rows plus members of any expanded group on the page), not a `[rawStart, rawEnd)` range. The deprecated `LogRenderer.tsx` call site (`:353`, which calls `computeFlexWidths(resultTable, columns, 0, numRows)` over the whole result) is updated to pass `range(0, numRows)`.
- `ignoreSet` is built with `useMemo` from `columns` + `collapseIgnoreColumns`, keyed on a joined string of the configured names so a fresh array from `options` does not invalidate the grouping memo on every render.

### Rendering

- Pull the row JSX (copy button + columns + dividers) out of the page map into a local `renderRow(rowIdx, stripe, trailing?)` closure, so representative rows and expanded member rows share one path.
- Representative row of a group with `count > 1` gets a trailing `RepeatBadge` (Option A mockup): a small `button` pill `×{count}` placed after the last column, `flex-none ml-2 self-start`, styled with `accent-link` tones.
  - `title`: `"{count} identical rows, {earlier timestamp} → {later timestamp}"` when a `time`-kind column exists (using `formatLocalTime`), else `"{count} identical rows"`. The two timestamps are ordered by value, not row position, since row order (and therefore which of `start`/`end-1` is earlier) flips between `ORDER BY time ASC` and `DESC`.
  - `aria-expanded`, `aria-label="Show {count} repeated rows"`.
- **Expand/collapse:** `expandedGroups: ReadonlySet<number>` state keyed by the group's `start` row index. When expanded, rows `start+1 … end-1` render directly after the representative row with a subtle tint (`bg-accent-link/5`), no badge, and the same stripe as their representative. Every raw row keeps its own copy button, so a copy or pin works on any member. The expanded set is cleared when `groups` changes (new result, ignore-list edit, toggle), using the same render-time "previous value" reset pattern already used for `copiedRowIdx` (`LogCell.tsx:209-213`). Expanded state is ephemeral and not persisted.
- Stripe alternation (`i % 2`) is computed per **group** on the page, not per raw row.
- Copy on a collapsed line copies the representative row (unchanged `formatRowForCopy`). The count is not appended, so the clipboard text stays a faithful log line.

### Toggle and ignore-list UI

- **Footer toggle** "Collapse repeats", placed before "Wrap text" and styled the same way (`aria-pressed`, accent when on). Icon: `ChevronsDownUp` from `lucide-react`. Persisted as `options.collapseRepeats: boolean`; unset means **on** (same default convention as `wrapText`).
- **Editor section** in `LogCellEditor`, below the SQL editor: "Ignore when collapsing repeats". It renders one toggle chip per name in `availableColumns`. `time`-kind columns are shown checked and disabled with the hint "always ignored". Toggling a chip writes `onChange({ ...logConfig, options: { ...logConfig.options, collapseIgnoreColumns } })`. Names in `collapseIgnoreColumns` that no longer exist in the result are kept as-is and simply don't match. If `availableColumns` is empty (cell not run yet), the section shows "Run the query to choose columns".
- `HgEditorPanel` gains an `availableColumns` prop, sourced the same way as the top-level case: `selectedChildName ? cellStates[selectedChildName]?.data[0]?.schema.fields.map((f) => f.name) : undefined`, and passes it through to `HorizontalGroupCellEditor` so a Log cell inside a horizontal group gets a working ignore-columns editor too.

## Mockups

- `tasks/log_cell_collapse_repeats_mockups/option-a-trailing-badge.html`: **chosen**. `×N` pill at the end of the collapsed line; rows without repeats are pixel-identical to today. Interactive: click a pill to expand, toggle "Collapse repeats" in the footer.
- `tasks/log_cell_collapse_repeats_mockups/option-b-leading-gutter.html`: Grafana-style count gutter before the timestamp.

## Implementation Steps

1. **`log-utils.tsx`**
   - Add `formatLogValue(col, value)`; rewrite `formatRowForCopy` and `computeFlexWidths` to use it. `renderLogColumn`'s per-kind switch also collapses to a call to `formatLogValue` for the display string, keeping its per-kind styling (className/title/color).
   - Add `LogRowGroup` and `groupConsecutiveRows(table, columns, ignore)`, including the "everything ignored → singleton groups" guard.
   - Add `range(start, end): number[]` and `singletonGroups(n): LogRowGroup[]` helpers, used by the pagination logic below.
   - **`LogRenderer.tsx`**: update the `computeFlexWidths(resultTable, columns, 0, numRows)` call site (`:353`) to pass `range(0, numRows)` instead of the raw start/end pair.
2. **`LogCell.tsx` renderer**
   - Read `collapseRepeats` (default `true`) and `collapseIgnoreColumns` (default `[]`) from `options`; build `ignoreSet` and `groups`.
   - Paginate over `groups.length`; derive `displayedRows` (representative rows plus expanded members) for `computeFlexWidths`.
   - Extract `renderRow`; render the page as groups plus expanded members; add `expandedGroups` state with its reset-on-`groups`-change.
   - Add `RepeatBadge` (local component in `LogCell.tsx`) and the "Collapse repeats" footer toggle.
3. **`LogCell.tsx` editor**
   - Destructure `availableColumns` in `LogCellEditor`; add the ignore-columns chip section writing `options.collapseIgnoreColumns`.
   - `NotebookRenderer.tsx`: add `availableColumns` to `HgEditorPanel`, sourced from `selectedChildName ? cellStates[selectedChildName]?.data[0]?.schema.fields.map((f) => f.name) : undefined`, and pass it through to `HorizontalGroupCellEditor` so Log cells nested in a horizontal group get it too.
4. **Tests** (see Testing Strategy).
5. **Docs** (see Documentation), plus a `CHANGELOG.md` Unreleased entry.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/LogRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`
- `analytics-web-app/src/lib/screen-renderers/__tests__/log-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/LogCell.test.tsx` (new)
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `CHANGELOG.md`

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` (Log section, ~line 243):
  - Options table: add `collapseRepeats` (boolean, default on) and `collapseIgnoreColumns` (string[]). While there, add the undocumented existing `wrapText` and `columnWidths` options.
  - Short paragraph describing collapsing: which columns are compared, the `×N` badge, click-to-expand, and that pagination counts collapsed lines.
  - Fix the stale "copy the full row as JSON" sentence (the copy is tab-delimited, per `formatRowForCopy`).
- `CHANGELOG.md`: Unreleased entry, "Log cell collapses consecutive repeated lines" (#1557).

## Testing Strategy

All no-DB unit tests with Vitest + React Testing Library. Arrow tables are built in-test with `tableFromArrays`/`makeTable`, following the existing cell tests (`cells/__tests__/TransposedTableCell.test.tsx` mock-props pattern).

`__tests__/log-utils.test.ts`:
- `formatLogValue`: one case per kind (time/level numeric and string/target/generic).
- New characterization tests written before the refactor, so they guard `formatRowForCopy` and `computeFlexWidths` against behavior changes when both are rewritten to call `formatLogValue`:
  - `formatRowForCopy`: one case per column kind, plus a case confirming tabs and newlines in a value are replaced (not left raw, since the copy is tab-delimited).
  - `computeFlexWidths`: one case per kind confirming its min/max width clamping.
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
- badge `title` contains the formatted timestamps ordered earlier → later, for both `ORDER BY time ASC` and `DESC` fixtures
- `LogCellEditor`: toggling a chip calls `onChange` with the updated `options.collapseIgnoreColumns`; a `time` chip is disabled

`yarn lint`, `yarn type-check`, `yarn test` from `analytics-web-app/`.

## Manual Verification

Visual placement and feel only. The logic is covered above, and a misplaced badge is obvious on sight.

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`, open http://127.0.0.1:3000, and create a notebook with a Log cell over `log_entries` for a window containing a repeated warning (or `SELECT` a `VALUES` list with repeated rows).
2. Expected: repeated lines collapse with a trailing `×N` pill aligned with the first text line in both wrap on and wrap off modes. Expanding a run shows tinted member rows, and column dividers stay aligned.
3. Flip "Collapse repeats" off, reload the page, and expect it to stay off.

## Decisions

- Badge placement: Option A (trailing `×N` pill), chosen by the user over Option B (leading gutter).
- Group the whole result, then paginate groups, rather than grouping within the current raw-row page (avoids almost-empty pages and runs splitting across a page boundary with wrong counts).
- Expanded runs render every member with no cap (accepted risk; bounded by the SQL `LIMIT`).
- `collapseRepeats` defaults on, not off (accepted cost: existing notebooks show fewer lines on next open).
- Time-kind columns are always ignored when grouping, not part of the configurable ignore list.
- Value equality uses `===` with a formatted-string fallback only for object-valued columns.
- The ignore list lives in the `LogCellEditor` panel, not the `LogDivider` context menu.
- Pagination's "1–100 of N" label counts lines (groups), not raw rows.
- Auto widths measure only rows on screen, not hidden members of collapsed runs.
