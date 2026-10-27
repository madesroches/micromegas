# Log Cell: Collapse Repeats by Message, Not Adjacency Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1738

## Overview

The Log cell's "collapse repeats" only merges rows that are adjacent and identical. When two messages alternate (A,B,A,B,A,B) nothing is adjacent to an identical row, so nothing collapses. This changes the grouping rule to key rows on their compared columns and merge rows sharing a key within a bounded window, so A,B,A,B,A,B renders as two `×3` lines. Rendering-only change: no query, data model, or SQL-surface change.

## Current State

Introduced by `tasks/completed/log_cell_collapse_repeats_plan.md`.

- `groupConsecutiveRows(table, columns, ignore)` (`analytics-web-app/src/lib/screen-renderers/log-utils.tsx:296`) returns `LogRowGroup { start, end }` — a contiguous `[start, end)` raw-row range. It compares every row with the group's first row and closes the group on the first mismatch.
- `LogCell.tsx` assumes contiguity throughout:
  - `displayedRows` uses `range(g.start, g.end)` (`:125`) and expanded members render with `range(g.start + 1, g.end)` (`:~420`).
  - `expandedGroups` is keyed by `g.start`.
  - `buildBadgeTitle` reads only `group.start` and `group.end - 1` (`:322`) for the timestamps.
  - `count = g.end - g.start`.
- Equality (`logValuesEqual`), the ignore set (time columns plus `collapseIgnoreColumns`), the "everything ignored → singletons" guard, pagination over groups, and the badge/toggle UI are all reusable as-is.

## Design

### Group shape

Replace the range with an explicit member list:

```ts
export interface LogRowGroup {
  rows: number[]    // raw row indices in ascending order, length >= 1
  anchor: number    // the member that represents the group on screen
}
```

`count` becomes `rows.length`. `range` stays only if other callers still need it (`LogRenderer.tsx`, `computeFlexWidths`).

### Grouping rule

Two rows are in the same group when they are equal on every compared column (same equality as today, `logValuesEqual`) **and** the later row is within `COLLAPSE_WINDOW_ROWS` raw rows of the group's most recent member.

Algorithm (single pass, O(rows × compared columns) expected):

1. For each row `i`, compute a string key from the compared columns' formatted values (`formatLogValue`, joined with an unambiguous separator such as `JSON.stringify` of the array). A hash map `key → open group` is probed.
2. If an open group exists and `i - lastMember(group) <= COLLAPSE_WINDOW_ROWS`, append `i`. Otherwise start a new group and replace the map entry (the stale group stays closed in the output).
3. The group's `anchor` is its most recent occurrence: the member with the greatest `time` value when a `time`-kind column exists, else the last member. Ordering by value, not position, because the result may be `ORDER BY time ASC` or `DESC`.
4. Output groups sorted by `anchor` row index, so the list stays roughly in the query's time order.

Keying on formatted strings rather than pairwise `logValuesEqual` is what makes a map lookup possible. It does change equality for primitives: `null` vs `''` must still differ, so the key must encode null distinctly (e.g. `null` in the JSON array vs `""`), and numbers/bigints must not collide with their string form (`JSON.stringify` of the raw primitive, formatted string only for object-valued columns — mirroring the current rule). Bigints need a `String(v) + 'n'`-style replacer since `JSON.stringify` throws on them.

### Window

A fixed row-count window, `COLLAPSE_WINDOW_ROWS = 200`, as a module constant in `log-utils.tsx`. Row count rather than time because the cell has no guaranteed time column and row distance is cheap and deterministic. It is not user-configurable until someone reports needing it.

### Rendering (`LogCell.tsx`)

- Representative row = `anchor` (not first member). Stripe index per group on the page, as today.
- Collapsed line shows `×N` badge; the badge title already lists earlier → later timestamps — compute min/max over **all** `rows` rather than the two endpoints, since members are no longer bracketed by `start`/`end - 1`. This satisfies "first and last timestamps for each group".
- Expanded: members other than the anchor render directly after the anchor in ascending row order with the existing tint.
- `expandedGroups` keyed by `anchor`. The reset-on-`groups`-change pattern is unchanged.
- `displayedRows`: `expandedGroups.has(g.anchor) ? g.rows : [g.anchor]`.
- Pagination still counts groups; a group still never straddles a page.
- Footer toggle, editor ignore-list, and option names are unchanged. The toggle label "Collapse repeats" stays accurate.

## Implementation Steps

1. `log-utils.tsx`
   - Change `LogRowGroup` to `{ rows, anchor }`; update `singletonGroups`.
   - Rename `groupConsecutiveRows` to `groupRepeatedRows` and rewrite it per the rules above; add `COLLAPSE_WINDOW_ROWS`. Drop `logValuesEqual` if the key builder fully replaces it.
   - Update the section header comment ("Grouping consecutive repeated rows").
2. `LogCell.tsx`: adapt count, `displayedRows`, expanded rendering, `expandedGroups` key, and `buildBadgeTitle` (min/max over `rows`) to the new shape.
3. Update existing tests, add new ones (below).
4. Docs and `CHANGELOG.md`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/log-utils.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/__tests__/log-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/LogCell.test.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `CHANGELOG.md`

## Trade-offs

- **Row-count window vs time window**: row count works without a time column and costs nothing; a time window would need a parsed timestamp per row and a decision for unsorted data.
- **Key map vs pairwise comparison against open groups**: pairwise is O(rows × open groups); a map keeps it linear.
- **Anchor by time vs by row position**: position would anchor at the oldest occurrence for `ORDER BY time DESC` results, contradicting "most recent occurrence".
- **Strict chronological order between groups is lost** (accepted in the issue); anchoring keeps it approximately ordered.

## Decisions

- Window is a fixed 200-row constant, not an option; revisit if users report needing control.
- The `collapseRepeats` / `collapseIgnoreColumns` option names and the UI are unchanged, so saved notebooks keep working; the behavior change (non-adjacent rows now merge) applies to every existing notebook with collapse on (the default).

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` (Log section): replace "consecutive identical rows" wording with the keyed-within-a-window description, mention the window size, the anchor at the most recent occurrence, and that the `×N` tooltip shows first/last timestamps.
- `CHANGELOG.md` Unreleased: "Log cell collapse repeats groups by message within a window instead of by adjacency" (#1738).

## Testing Strategy

No-DB Vitest unit tests; no live-DB tests (no witnessed bug beyond the issue's behavior, which is pure logic).

`log-utils.test.ts` (replace the adjacency cases):
- A,B,A,B,A,B → two groups of 3; group order follows anchors.
- A,B,A: A has `rows [0,2]`, B `rows [1]`.
- Same key with a gap larger than `COLLAPSE_WINDOW_ROWS` → two groups; gap exactly at the limit → one group.
- Anchor is the latest-by-time member for both ASC and DESC fixtures; with no time column it is the last row.
- Ignored column (`request_id`) merges rows only when ignored; all columns ignored → singletons.
- `null` vs `''` differ; bigint column values don't throw and group correctly; object-valued column groups by formatted value; a value containing the key separator can't collide with a different tuple.
- Empty table → `[]`; single row → one group.

`LogCell.test.tsx`:
- Interleaved A,B,A,B → 2 lines with `×2` badges; expanding shows the members in row order.
- Badge title lists earliest → latest timestamps across non-adjacent members.
- Pagination counts groups; existing toggle/editor tests keep passing.

`yarn lint`, `yarn type-check`, `yarn test` from `analytics-web-app/`.

## Open Questions

- Is 200 rows the right default window? (A guess against the default 100-line page; easy to change.)
