# Chart Selection Shifts Chart Right Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1721

## Overview

Click selection on notebook chart cells (`selectionMode: 'single'`) makes the bar chart's plot
area jump to the right on every select and every deselect. Selection must be a pure repaint: the
plot rectangle (left edge, width, axis bands) must be identical before and after a selection
change. This plan finds why the layout changes and fixes it so selection never alters layout.

## Current State

Selection state flows `ChartCell` → `XYChart` without rebuilding the uPlot instance:

- `ChartCell.tsx:216-238` keeps `selected` in local state and passes `selectedPoint` to `XYChart`
  (`ChartCell.tsx:296-297`, `350-351`).
- `XYChart.tsx:422-425` reacts to `selectedPoint` by writing `selectedRef` and calling
  `chartRef.current?.redraw(true)` so the bar `disp` color callbacks and point styles re-run.
- The create/update effect (`XYChart.tsx:810-1270`) destroys and rebuilds the chart when `data`,
  `normalizedSeries`, `activeSelection`, and the other listed deps change. `dimensions` is
  deliberately excluded; a separate effect (`XYChart.tsx:1273-1277`) calls `setSize`.
- Layout is computed by uPlot's `convergeSize()` (axis bands from `yAxisSize` and, for
  categorical bar charts, the x-axis `rotate`/`size`/`space` callbacks and the `rightPadding`
  function in `xychart-axis.ts:60-150`). The categorical x-axis keeps shared mutable state
  (`rotated`, `maxWidth`) that `rotate()` writes and `size()`/`rightPadding()` read.
- uPlot 1.6.32 `redraw(rebuildPaths, recalcAxes)` (`uPlot.esm.js:4919`) sets
  `shouldConvergeSize = recalcAxes || false`, then `_setScale(x, ...)`. Reading the source, a
  plain `redraw(true)` should not re-run axis sizing, so the shift is not explained by that call
  alone; the root cause is not yet established.

The reproducing notebook is a bar chart with `scale_mode: max`, a category axis of usernames
(long labels, so the x-axis is rotated) and a downstream table. The issue reports the shift
happens on every select/deselect, in both directions.

## Design

### Step 0: reproduce and isolate (decides the fix)

The cause must be confirmed in a browser before changing code; candidates, in rough likelihood:

1. **Re-measurement after the selection round trip.** `onSelectionChange` publishes
   `$cell.selected.*`; downstream cells re-render/re-execute, which can change the container
   width (scrollbar appearing/disappearing, or the cell wrapper) and `measureContainer`
   (`XYChart.tsx:367-380`, `rect.width - 32`) feeds `setSize`. A drift in what is measured would
   explain a shift that accumulates per click.
2. **Category-axis shared state.** `rotated`/`maxWidth` in `buildXAxisConfig` persist across
   `redraw`; if `rotate()` is re-invoked with a different `foundSpace` during the selection
   redraw, the `rightPadding`/`size` readers change the plot's left/right insets.
3. **Chart rebuild on selection.** `activeSelection` or another dep in the effect list at
   `XYChart.tsx:1270` changing identity would destroy/recreate the chart on each click and
   re-run axis sizing from scratch (the effect comment at `XYChart.tsx:1264-1266` assumes it
   does not).
4. **Selection-dependent drawing outside the plot.** The bar `disp` colors / stroke width change
   the bar paths' extents, but uPlot sizes the plot independently of path content, so this is
   unlikely to move the plot.

Instrument by logging `u.bbox`, `u.over` left/width, and `dimensions` before/after a click in the
reproducing notebook (a bar chart over `log_entries` grouped by level with a long-label category
is enough) and identify which of the above changes. Whichever field moves names the cause.

### Step 1: fix at the cause, and make layout independent of selection

Fix per the confirmed cause; the invariant to enforce is "a selection change never changes plot
layout". Expected shapes, to be chosen after Step 0:

- Cause 1: make the measured container size independent of the downstream cells (stable
  scrollbar gutter on the cell scroll container, or measure the content box rather than a
  value that includes the selection-dependent area).
- Cause 2: recompute/pin the categorical axis state only when data or width change, not on
  selection redraws (for example, compute `rotated`/`maxWidth` once per chart build).
- Cause 3: remove the changing dep from the effect list (route it through a ref, as
  `selectedRef` already does).

## Implementation Steps

1. Reproduce in the browser (monolith + `yarn dev`, see CLAUDE.md "Service Management") with a
   bar chart cell, `selectionMode: single`, a long-label category and a downstream table;
   instrument as in Step 0 and record which value changes. Remove the instrumentation afterward.
2. Apply the fix at the identified cause in `analytics-web-app/src/components/XYChart.tsx`,
   `xychart-axis.ts`, or the cell/notebook container, per Design Step 1.
3. Add a unit test that pins the layout invariant at the lowest tier that reproduces the cause
   (see Testing Strategy).
4. Add a `CHANGELOG.md` entry under the unreleased section.

## Files to Modify

- `analytics-web-app/src/components/XYChart.tsx` (likely)
- `analytics-web-app/src/components/xychart-axis.ts` (if cause 2)
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` or the notebook cell wrapper
  (if cause 1)
- A test under `analytics-web-app/src/components/__tests__/` or the cell `__tests__` folder
- `CHANGELOG.md`

## Trade-offs

- **Fix at the cause vs. pin the plot rectangle with a post-redraw `setSize`/padding override.**
  An override would hide the symptom for any cause and leave the real layout instability in
  place, so it is rejected.
- **Diagnose first vs. guess a fix.** Static reading of uPlot and `XYChart` does not explain the
  shift (a plain `redraw(true)` skips axis convergence), so a guessed fix risks shipping a no-op.

## Decisions

## Documentation

No documentation change: selection behavior is already documented in
`mkdocs/docs/web-app/notebooks/cell-types.md` and `variables.md`, and this restores the intended
behavior.

## Testing Strategy

Automated: a no-DB vitest unit test that pins the confirmed cause, named for the bug it covers
(chart plot rectangle shifts on selection change), at whichever tier reproduces it. If the cause
is in `buildXAxisConfig` state or an effect dependency, the test calls the pure axis builder or
renders `XYChart` with a mocked uPlot and asserts the layout inputs (`size`/`padding`
callbacks, chart construction count) are unchanged across a `selectedPoint` change. If the cause is
container measurement, the test asserts the measured width is unaffected by selection state.
No live-DB test is needed: the bug needs only a client-side chart and constructed data.

Existing suites (`ChartCell.test.tsx`, `xychart-selection.test.ts`) and lint/`tsc` must pass.

## Manual Verification

uPlot canvas layout needs a real browser (jsdom has no layout), so the final check is manual:

1. Start the monolith (`python3 local_test_env/ai_scripts/start_services.py --monolith`) and
   `yarn dev` in `analytics-web-app/`; open http://127.0.0.1:3000.
2. Create a notebook with a bar chart cell, `selectionMode: single`, over
   `SELECT CAST(level AS VARCHAR) AS level, COUNT(*) AS n FROM log_entries GROUP BY level`,
   followed by a table cell using `$<chart>.selected.level`.
3. Click a bar, then the same bar again, then another bar. Expected: the plot's left edge and
   width do not move on any click. Repeat with long category labels so the x-axis is rotated.

## Open Questions

None; the root cause is determined in Step 0.
