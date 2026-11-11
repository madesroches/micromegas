# Chart Bar Column Selection Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1786

## Overview

In a bar chart with `selectionMode: 'single'`, a click only selects a bar when it lands inside
the bar's drawn rectangle. With `scale_mode: 'max'` and a long-tailed distribution, most bars are
a few pixels tall, so they are hard to hit, and a near miss just above a short bar counts as a
click on empty space and clears the selection. This change hit-tests bars on the X axis only:
a click anywhere in a bar's column, from the top of the plot area down to its bottom, selects that bar.

## Current State

- `analytics-web-app/src/components/xychart-selection.ts:22` — `hitTestBars(rects, x, y)` returns
  the last (topmost) rect with `left <= x <= left + width` **and** `top <= y <= top + height`.
- `analytics-web-app/src/components/XYChart.tsx:434-458` — `makeBarPaths` collects one `BarRect`
  per drawn bar from uPlot's `bars({ each })` callback (canvas pixels). uPlot 1.6.32 calls `each`
  for every non-null value, including zero-height bars (`uPlot.esm.js:2617`). Each series gets
  slot width `0.8 / seriesCount` and its own `align` offset, so grouped series sit side by side.
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

Change `hitTestBars` to test the horizontal extent of each rect against the cursor X, and to
test the cursor Y against the plot area's vertical bounds instead of the rect's:

```ts
/** Topmost bar whose column (its horizontal extent, across the full plot height) contains (x, y), in canvas pixels. */
export function hitTestBars(
  rects: BarRect[],
  x: number,
  y: number,
  plot: { top: number; height: number },
): BarRect | null
```

- A hit requires `r.left <= x <= r.left + r.width` and `plot.top <= y <= plot.top + plot.height`.
  The rect's own `top`/`height` are no longer consulted, so a zero-height bar is selectable by
  its column, and so is a bar drawn below the baseline (negative value).
- Iteration order stays reverse (last pushed wins), so shared edges between adjacent grouped
  bars resolve the same way as today.
- Per-series slots are unchanged: each rect keeps its own `left`/`width` from uPlot, so in a
  grouped chart each series' bar keeps its own column, and the gaps between slots and between
  categories still clear the selection on click.
- `hitAt` (`XYChart.tsx:471`) passes `{ top: u.bbox.top, height: u.bbox.height }`. Because
  `hitAt` drives `setCursor` too, the pointer cursor follows the new column hit area with no
  extra change.
- `BarRect` keeps `top`/`height`. Nothing else reads them, but they are what uPlot reports and
  dropping them would buy nothing.

Clearing keeps working through: clicking a gap between columns, clicking outside the plot area
(rejected earlier by `hitAt`), or clicking the selected column again (`nextSelection`).

## Implementation Steps

1. `xychart-selection.ts`: update `hitTestBars` signature, check, and doc comment as above.
2. `XYChart.tsx`: pass the plot bounds from `u.bbox` in `hitAt`'s bar branch.
3. `__tests__/xychart-selection.test.ts`: update existing calls to pass a plot box and adjust
   the "misses outside" case, which today asserts a miss above a bar, and add:
   - click above a short bar (inside its column, well above `top`) → hits that bar;
   - click below a bar's rect but inside the plot → hits;
   - click in the gap between two columns → null;
   - zero-height bar (`height: 0`), click anywhere in its column → hits;
   - click in the column but above `plot.top` or below `plot.top + plot.height` → null;
   - grouped series: clicks in each series' slot select that series.
4. `mkdocs/docs/web-app/notebooks/cell-types.md:68`: say that clicking anywhere in a bar's column
   selects it, and that the gap between columns counts as empty space.
5. `yarn lint`, `yarn type-check`, `yarn test` in `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/components/xychart-selection.ts`
- `analytics-web-app/src/components/XYChart.tsx`
- `analytics-web-app/src/components/__tests__/xychart-selection.test.ts`
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Bar's own slot vs. widening to the full category slot.** The issue offers widening the hit
  column to bar width plus half the gap on each side. That would leave no empty space inside the
  plot in single-series charts, so clearing would rely only on re-clicking the selected column or
  clicking outside the plot. Keeping the drawn slot (80% of the category width split across
  series) gives a large target while keeping in-plot clearing. Widening can be added later if
  very narrow bars prove a problem.
- **Plot bounds as a parameter vs. dropping the vertical check.** `hitAt` already rejects cursors
  outside `u.over`, so the Y check is redundant in practice. Passing the plot box keeps
  `hitTestBars` self-contained and testable on the "stay inside the plot" rule the issue asks
  for, at the cost of one extra argument.
- **Tooltip.** Already tied to the nearest X index, so it already shows on column hover. A
  tooltip limited to the hit column would be a separate change, out of scope.

## Decisions

- Line/point selection (`findNearestPoint`, `POINT_HIT_RADIUS`) is out of scope, per the issue.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` — Chart section, click selection bullet.

## Testing Strategy

The hit-test rule is a pure function and is covered by the `hitTestBars` unit tests listed in
step 3, including the cases the issue names: a click above a short bar, a click in the gap, and
a click on a zero-height bar.

## Manual Verification

The wiring from `hitAt` (the `u.bbox` conversion and the cursor style) runs inside a live uPlot
canvas that jsdom cannot drive. Breakage would be obvious on first use:

1. `yarn dev` in `analytics-web-app/`, open a notebook with a bar chart cell set to
   `selectionMode: 'single'` and `scale_mode: 'max'` over a long-tailed result.
2. Hover above a short bar: the pointer cursor appears. Click: that bar stays at full color and
   the others dim.
3. Click the gap between two columns: the selection clears. Click a column, then click the same
   column again: the selection clears.
4. With two queries (grouped bars), click above each series' bar in one category: each click
   selects that series' bar.
