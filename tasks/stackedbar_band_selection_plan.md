# Bar Chart Band Selection Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1828

## Overview

With `selectionMode: 'single'`, the Bar Chart cell (`stackedbar`) only selects when a click lands
on a drawn segment. A click in the empty part of a bar's row or column, or on its label, counts
as a background click and clears the selection. In a ranked horizontal chart most bars are short,
which makes the lower rows hard to select. This change makes the whole band of a category
clickable: the full row in horizontal orientation, the full column in vertical orientation, and
the category label. The Chart cell already behaves this way (`tasks/completed/chart_bar_column_selection_plan.md`).

## Current State

All in `analytics-web-app/src/lib/screen-renderers/cells/StackedBarCell.tsx`:

- `buildStackedBarLayout` (vertical, ~line 265) computes a per-category `band`
  (`Math.max(naturalBand, MIN_BAND_WIDTH)`) and places bar `c` at
  `yAxisWidth + band * c + (band - barWidth) / 2`. The band itself is not kept in the layout.
- `buildHorizontalBarLayout` (~line 345) gives each category a fixed `ROW_HEIGHT` row starting at
  `rowY = ROWS_VERTICAL_PADDING / 2 + c * ROW_HEIGHT`. Each bar is `BAR_THICKNESS` thick and
  centered in the row. The row spans the full `contentWidth`, label column included.
- `stackSegments` (~line 216) emits only segments with `value > 0`, so a category whose values are
  all zero has no segments.
- `StackedBarView` (~line 521) renders the segments as `<path data-segment>` with
  `onClick → onSegmentClick({ categoryIndex, seriesIndex })`. Each plot `<svg>` has
  `onClick={handleBackgroundClick}`, which calls `onBackgroundClick` unless the target is inside
  `[data-segment]` (~line 570).
- Category labels are `<text>` elements with a `<title>`. They have no click handler, so a click
  on a label clears the selection.
- In horizontal mode the sticky axis `<svg data-testid="bar-axis">` has no click handler, so a
  click there does nothing.
- `StackedBarCell.handleSegmentClick` (~line 885) toggles: clicking the selected hit again
  publishes `null`. `handleBackgroundClick` (~line 894) clears.
- Tests: `analytics-web-app/src/lib/screen-renderers/cells/__tests__/StackedBarCell.test.tsx`,
  `describe('StackedBarCell selection and orientation')` (~line 491).
- Docs: `mkdocs/docs/web-app/notebooks/cell-types.md:24` (option table) and `:45` (Row Selection
  bullet).

## Design

### Layout: keep each category's hit band

Add one field to `StackedBarBarLayout`:

```ts
/** The category's full clickable band in plot-svg coordinates. */
hitBand: { x: number; y: number; width: number; height: number }
```

- Vertical: `{ x: yAxisWidth + band * c, y: 0, width: band, height: contentHeight }`. The column
  runs from the top of the svg through the label area.
- Horizontal: `{ x: 0, y: rowY, width: contentWidth, height: ROW_HEIGHT }`. The row covers the
  label column and the space past the bar's end.

Adjacent bands touch, so the only empty space left inside a plot svg is the y-axis gutter and the
right margin (vertical), or the top and bottom padding (horizontal). Clicking there, on the
horizontal axis svg, or on the selected band again clears the selection.

### Which segment a band click selects

Add a pure, exported helper:

```ts
/** Series index of the segment nearest to `pos` along the value axis; null when the bar has no segments. */
export function nearestSegmentSeries(bar: StackedBarBarLayout, orientation: BarOrientation, pos: number): number | null
```

`pos` is the click's coordinate along the value axis (y for vertical, x for horizontal). A segment
whose value-axis extent (`[y, y + height]` or `[x, x + width]`) contains `pos` wins. Otherwise the
segment at the smallest distance to its extent wins. This gives the expected result in every
case:

- 2-column shape: the single segment, wherever the band is clicked.
- Stacked, click past the bar's end (to the right of the bar, or above it): the end segment.
- Stacked, click in the gap beside a segment (the row padding above or below a horizontal bar, or
  the band space beside a vertical bar): that segment.
- Click on the label, or in the horizontal label column, which sits before the baseline: the
  first (baseline) segment.

A click directly on a segment still goes through the segment's own handler, so that behavior is
unchanged.

### Rendering and events

In `StackedBarView`, only when `onSegmentClick` is present (selection enabled):

- Before the segments, render one `<rect>` per bar that has segments, at `bar.hitBand`, with
  `fill="transparent"`, `data-bar-hit`, and `className="cursor-pointer"`. A transparent fill still
  receives pointer events. Draw these first in both orientations, before the grid lines and labels
  in horizontal mode, so segments and labels stay on top.
- Give each category label `<text>` of a bar that has segments the same click handler and
  `data-bar-hit`, plus `cursor-pointer`. Rotated vertical labels hang into the neighboring band, so
  the label needs its own handler bound to its own category, not just the band rect under it.
- Both call a shared `handleBandClick(bi, e)`. It converts the click to plot-svg coordinates with
  `e.currentTarget.ownerSVGElement.getBoundingClientRect()` (each svg renders at its viewBox size,
  1:1). It takes y in vertical mode or x in horizontal mode, resolves `nearestSegmentSeries`, and
  calls `onSegmentClick({ categoryIndex: bi, seriesIndex })` when the result is non-null.
- Rename the segment marker `data-segment` to `data-bar-hit` on segment paths as well.
  `handleBackgroundClick` ignores clicks inside `[data-bar-hit]`, so a band or label click doesn't
  bubble up into a clear.
- Add `onClick={handleBackgroundClick}` to the horizontal axis svg, so the axis area clears as the
  issue expects.

Toggling needs no change: a band click resolves to a `BarHit`, and `handleSegmentClick` already
clears when it equals the current selection. In the 2-column shape, a second click anywhere in the
selected band clears it. In the stacked shape, it clears when the click resolves to the selected
segment.

The hover tooltip stays on segments only.

## Implementation Steps

1. `StackedBarCell.tsx`:
   - Add `hitBand` to `StackedBarBarLayout` and fill it in `buildStackedBarLayout` and
     `buildHorizontalBarLayout` as above.
   - Add `nearestSegmentSeries` next to `buildBarSelection` in the Selection section.
   - In `StackedBarView`, add the hit rects, label click handlers, `handleBandClick`, the
     `data-segment` → `data-bar-hit` rename, and the axis-svg background click.
2. `__tests__/StackedBarCell.test.tsx`:
   - Layout: `hitBand` for vertical (adjacent bands tile from `yAxisWidth`, full content height)
     and horizontal (`x = 0`, `width = contentWidth`, `y = rowY`, `height = ROW_HEIGHT`).
   - `nearestSegmentSeries`: inside a segment, past the end (end segment), before the baseline
     (first segment), in a segment's gap, and no segments (`null`), for both orientations.
   - Component (jsdom's `getBoundingClientRect` returns zeros, so `clientX`/`clientY` are plot-svg
     coordinates):
     - 2-column horizontal: clicking a row's `[data-bar-hit]` rect publishes that row, and
       clicking it again clears.
     - Clicking a category label selects that category.
     - 3-column: a band click past the bar's end selects the end segment.
     - A click on the horizontal axis svg clears.
     - With `selectionMode` unset, no hit rects render.
   - Existing tests select segments with `svg path` and clear by clicking the `svg`, so they keep
     passing.
3. `mkdocs/docs/web-app/notebooks/cell-types.md`: update line 24 ("Click a bar or segment …") and
   the Row Selection bullet at line 45. A click anywhere in a category's row or column, or on its
   label, selects it. In the stacked shape, the nearest segment along the value axis is selected.
   The axis gutter, the margins, and clicking the selected bar again clear it.
4. `yarn lint`, `yarn type-check`, `yarn test` in `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/StackedBarCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/StackedBarCell.test.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Hit rects vs. a hit-test on the svg's click.** One handler on the svg could map the click
  position to a band. Transparent per-band rects (the issue's suggestion) reuse the existing event
  shape, give a free pointer cursor over the band, and keep the geometry in the pure layout, where
  it can be unit-tested.
- **Nearest segment vs. always the end segment vs. nothing.** Always picking the end segment is
  simpler. But it would make a click on the label or beside a middle segment select a segment far
  from the click. Nearest-along-the-value-axis costs one small pure function and matches the
  issue's "nearest/last segment" wording.
- **Full band vs. the Chart cell's bar-width column.** The Chart cell kept gaps between columns as
  in-plot empty space. Here the issue explicitly asks for the full band, label included, and the
  gutter, the margins, and the axis remain as clearing targets.

## Decisions

- Categories with no non-zero segments get no hit rect and no label handler, so clicking their
  band clears, as before.
- The hover tooltip is not extended to the band.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` — Bar Chart option table (`selectionMode`) and
  the Row Selection bullet.

## Testing Strategy

The band geometry and the segment resolver are pure functions covered by the unit tests in step
2. The click wiring (hit rects, label handlers, marker-based background suppression, toggling) is
covered by jsdom component tests. The tests rely on jsdom's zeroed `getBoundingClientRect`, so
client coordinates equal plot-svg coordinates.

## Manual Verification

Real-browser coordinate conversion inside a scrolled plot can't be reproduced in jsdom. A broken
conversion would be obvious on first use. In a notebook with a horizontal `stackedbar`, set
`selectionMode: 'single'`, scroll the plot, then click to the right of a short bar and on its
label: both select that row, and clicking the row again clears.
