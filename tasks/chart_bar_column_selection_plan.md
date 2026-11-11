# Chart Bar Column Selection Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1786

## Overview

In a bar chart with `selectionMode: 'single'`, a click only selects a bar when it lands inside
the bar's drawn rectangle. With `scale_mode: 'max'` and a long-tailed distribution, most bars are
a few pixels tall, so they are hard to hit, and a near miss just above a short bar counts as a
click on empty space and clears the selection. This change adds an X-only fallback to the bar hit test:
a click inside a drawn bar still selects that bar, and a click anywhere else in a bar's column
selects that bar (`hitAt` already guarantees the click is inside the plot).

## Current State

- `analytics-web-app/src/components/xychart-selection.ts:22` — `hitTestBars(rects, x, y)` returns
  the last (topmost) rect with `left <= x <= left + width` **and** `top <= y <= top + height`.
- `analytics-web-app/src/components/XYChart.tsx:434-460` — `makeBarPaths` collects one `BarRect`
  per drawn bar from uPlot's `bars({ each })` callback (canvas pixels). uPlot 1.6.32 calls `each`
  for every non-null value, including zero-height bars (`uPlot.esm.js:2631`). Each series gets
  bar width `0.8 / seriesCount` of the category, but multi-series charts pass `align: i`
  (`XYChart.tsx:1002-1008`), and uPlot only distinguishes `0`, `1`, and anything else
  (`uPlot.esm.js:2567`): series 0 is centered on the category x, series 1 spans `[x, x + bw]`, and
  every series ≥ 2 spans about `[x - bw, x]`. Grouped bars therefore overlap rather than sit side
  by side; with 3+ series, series 0 is horizontally covered by series 1 and 2.
- `XYChart.tsx:463-475` — `hitAt` in `createSelectionPlugin` rejects cursors outside the plot
  (`left/top < 0`), filters rects of hidden series, converts the cursor to canvas px, and calls
  `hitTestBars`. The same `hitAt` drives both the click (`mouseup`) and the pointer cursor
  (`setCursor`, `XYChart.tsx:520-526`).
- Tooltips (`createMultiSeriesTooltipPlugin` / `createTooltipPlugin`) already follow uPlot's
  `cursor.idx`, the nearest X index, so hovering anywhere in a column already shows that
  category's tooltip. No change is needed there.
- `analytics-web-app/src/components/__tests__/xychart-selection.test.ts:15-39` — `hitTestBars`
  unit tests (inside, edges, outside, overlap).
- `mkdocs/docs/web-app/notebooks/cell-types.md:68` — Chart click-selection description.

## Design

Change `hitTestBars` to a two-tier rule, keeping its signature:

```ts
/**
 * Topmost bar containing (x, y); failing that, topmost bar whose horizontal extent contains x.
 * Canvas pixels.
 */
export function hitTestBars(rects: BarRect[], x: number, y: number): BarRect | null
```

- Tier 1 is today's check (`left <= x <= left + width` and `top <= y <= top + height`). Because
  grouped bars overlap (see Current State), keeping it first means a click on a bar's visible
  drawn area still selects that bar, exactly as today.
- Tier 2 only runs when tier 1 misses: it requires `r.left <= x <= r.left + r.width`, ignoring
  `top`/`height`, so a click above a short bar, on a zero-height bar's column, or on a bar drawn
  below the baseline (negative value) selects it.
- Both tiers iterate in reverse (last pushed wins), matching the drawing order, so where columns
  overlap the bar drawn on top wins.
- The grouped layout (`align: i`) is not changed. Each rect keeps its own `left`/`width` from
  uPlot, and the gaps between categories still clear the selection on click.
- `hitAt` (`XYChart.tsx:471`) is unchanged: it already passes both converted coordinates. Because
  `hitAt` drives `setCursor` too, the pointer cursor follows the new column hit area with no
  extra change.

Clearing keeps working through: clicking a gap between columns, clicking outside the plot area
(rejected earlier by `hitAt`), or clicking the selected column again (`nextSelection`).

## Implementation Steps

1. `xychart-selection.ts`: implement the two-tier rule and update the doc comment as above.
2. `__tests__/xychart-selection.test.ts`: adjust the "misses outside" case, which today asserts a
   miss above a bar, and add:
   - a click above a short bar, and any y in the column of a `height: 0` rect, hit that rect;
   - click in the gap between two columns → null;
   - overlapping rects (as grouped bars are drawn): a click inside the earlier rect's drawn area,
     outside the later one, selects the earlier rect; a click above both selects the later rect.
3. `mkdocs/docs/web-app/notebooks/cell-types.md:68`: say that clicking anywhere in a bar's column
   selects it, and that the gap between columns counts as empty space.
4. `yarn lint`, `yarn type-check`, `yarn test` in `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/components/xychart-selection.ts`
- `analytics-web-app/src/components/__tests__/xychart-selection.test.ts`
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Bar's own slot vs. widening to the full category slot.** The issue offers widening the hit
  column to bar width plus half the gap on each side. That would leave no empty space inside the
  plot in single-series charts, so clearing would rely only on re-clicking the selected column or
  clicking outside the plot. Keeping the drawn slot (80% of the category width split across
  series) gives a large target while keeping in-plot clearing. Widening can be added later if
  very narrow bars prove a problem.
- **Two-tier hit test vs. fixing the grouped layout.** Grouped bars overlap because `align: i` is
  not a real per-series offset. Computing non-overlapping per-series offsets would give each
  series a distinct column, but it changes how every grouped bar chart is drawn. The two-tier
  rule meets the issue's goal (short bars easy to click) without any visual change: visible bars
  stay selectable as today, and clicks above them pick the topmost column. A series fully hidden
  by its neighbors stays unselectable, as it is today.

## Decisions

- Line/point selection (`findNearestPoint`, `POINT_HIT_RADIUS`) is out of scope, per the issue.
- A tooltip limited to the hit column is out of scope.
- AFK: Grouped bars overlap (`align: i` is not a per-series offset), so use a two-tier hit test (topmost rect containing (x, y), else topmost rect whose X extent contains x) and leave the grouped bar layout unchanged.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` — Chart section, click selection bullet.

## Testing Strategy

The hit-test rule is a pure function and is covered by the `hitTestBars` unit tests listed in
step 3, including the issue's cases (a click above a short bar, a click on a zero-height bar), which are
covered by the X-only fallback, a click in the gap, and overlapping rects as grouped bars draw them.

## Manual Verification

The wiring from `hitAt` (the `u.bbox` conversion and the cursor style) runs inside a live uPlot
canvas that jsdom cannot drive. Breakage would be obvious on first use:

1. `yarn dev` in `analytics-web-app/`, open a notebook with a bar chart cell set to
   `selectionMode: 'single'` and `scale_mode: 'max'` over a long-tailed result.
2. Hover above a short bar: the pointer cursor appears. Click: that bar stays at full color and
   the others dim.
3. Click the gap between two columns: the selection clears. Click a column, then click the same
   column again: the selection clears.
4. With two queries (grouped bars), click on each series' visible bar area in one category: each
   click selects that series' bar. Click above both bars: the bar drawn on top (series 1) is selected.
