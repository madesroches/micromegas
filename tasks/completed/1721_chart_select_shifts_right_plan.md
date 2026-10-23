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
- uPlot 1.6.32 `redraw(true)` (`uPlot.esm.js:4919`) calls `_setScale(x, scaleX.min, scaleX.max)`;
  `setScales` (`:4040-4065`) then runs `wsc.range(self, wsc.min, wsc.max)` on those already-ranged
  values. The categorical x-scale `range` in `buildXScale` (`xychart-axis.ts:139-147`) is
  `[dataMin - 0.5, dataMax + 0.5]`, which is not idempotent: every `redraw(true)` widens the x
  range by 0.5 per side. The changed scale sets `shouldConvergeSize` (`:4172`), so axis sizing
  (`rotate`/`size`/`rightPadding`) re-runs and the plot rectangle moves. This is the root cause;
  the selection effect is only the trigger. Drag-zoom is disabled (`setScale: false`), so the x
  range is always derived from the data.

The reproducing notebook is a bar chart with `scale_mode: max`, a category axis of usernames
(long labels, so the x-axis is rotated) and a downstream table. The issue reports the shift
happens on every select/deselect, in both directions.

## Design

### Step 1: make the categorical x range idempotent

Change `buildXScale`'s categorical `range` to ignore its `dataMin`/`dataMax` arguments and derive
the bounds from the data: `[u.data[0][0] - 0.5, u.data[0][last] + 0.5]` (the x data is the slot
index). When `u.data[0]` is empty, fall back to the incoming min/max. The output is then identical
on every call, so `redraw(true)` leaves the scale unchanged, no convergence is triggered, and
selection becomes a pure repaint. The half-slot padding and its comment (why, not history) stay.

Alternative considered and rejected: avoid re-ranging on selection redraws (for example by
repainting without `_setScale`). That depends on uPlot internals and leaves any other
`redraw(true)` caller (and `setData`-less re-ranging) with the same drift.

## Implementation Steps

1. Make the categorical `range` in `xychart-axis.ts` idempotent per Design Step 1.
2. Add the unit test described in Testing Strategy.
3. Add a `CHANGELOG.md` entry under the unreleased section.
4. Confirm in the browser (Manual Verification).

## Files to Modify

- `analytics-web-app/src/components/xychart-axis.ts`
- `analytics-web-app/src/components/__tests__/xychart-axis.test.ts`
- `CHANGELOG.md`

## Trade-offs

- **Fix the range vs. pin the plot rectangle with a post-redraw `setSize`/padding override.**
  An override would hide the symptom and leave the drifting scale in place, so it is rejected.
- **Idempotent range vs. skipping the re-range on selection redraws.** The range fix is a
  one-function change inside our code and cannot be reintroduced by another `redraw(true)` caller;
  the alternative relies on uPlot internals.

## Decisions

- AFK: Fixed the cause by making the categorical x-scale `range` ignore its inputs (derive from `u.data[0]`) rather than avoiding re-ranging on selection redraws, as the smaller change that also covers every other `redraw(true)` caller.

## Documentation

No documentation change: selection behavior is already documented in
`mkdocs/docs/web-app/notebooks/cell-types.md` and `variables.md`, and this restores the intended
behavior.

## Testing Strategy

Automated: a no-DB vitest unit test in `xychart-axis.test.ts`, named for the bug (categorical x
range drifts on redraw). It builds `buildXScale('categorical')`, calls `range` with a stub `u`
whose `data[0]` is `[0, 1, 2, 3]`, feeds the output back in as `dataMin`/`dataMax` several times,
and asserts the result stays `[-0.5, 3.5]`. A second case covers empty `data[0]`. The existing
`fn(u, 0, 3)` case needs its stub `u` to carry `data`. No live-DB test is needed.

Existing suites (`ChartCell.test.tsx`, `xychart-selection.test.ts`) and lint/`tsc` must pass.

## Manual Verification

uPlot canvas layout needs a real browser (jsdom has no layout), so the final check is manual:

1. Start the monolith (`python3 local_test_env/ai_scripts/start_services.py --monolith`) and
   `yarn dev` in `analytics-web-app/`; open http://127.0.0.1:3000.
2. Create a notebook with a bar chart cell, `selectionMode: single`, over
   `SELECT CAST(level AS VARCHAR) AS level, COUNT(*) AS n FROM log_entries GROUP BY level`,
   followed by a table cell using `$<chart>.selected.level`.
3. Click a bar, then the same bar again, then another bar. Expected: the plot's left edge and
   width do not move on any click. Repeat with long category labels so the x-axis is rotated. This browser check confirms the
unit-tested fix; jsdom cannot show the layout.

## Open Questions

None.
