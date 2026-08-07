# Categorical Chart X-Axis: Rotate Overlapping Labels Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1425

## Overview

Chart cells in categorical X-axis mode render tick labels horizontally at a
fixed 65px axis height. When labels are long strings (e.g. version/build
identifiers) and/or there are many categories, labels overlap and become
unreadable. This adds adaptive rotation to `buildXAxisConfig`: labels tilt to
-45° and the axis grows to fit them, but only when the available per-tick
space is too narrow to fit the labels horizontally — short labels or few
categories keep rendering flat, as today.

## Current State

**File:** `analytics-web-app/src/components/xychart-axis.ts`, `buildXAxisConfig()` (lines 12-45)

```ts
export function buildXAxisConfig(xAxisMode: XAxisMode, xLabels?: string[]): uPlot.Axis {
  const xAxisConfig: uPlot.Axis = {
    stroke: '#6a6a7a',
    grid: { stroke: '#2a2a35', width: 1 },
    ticks: { stroke: '#2a2a35', width: 1 },
    font: '11px -apple-system, BlinkMacSystemFont, sans-serif',
    size: 65,
  }

  if (xAxisMode === 'categorical' && xLabels) {
    xAxisConfig.incrs = [1]
    xAxisConfig.space = 60
    xAxisConfig.values = (_u, vals) => vals.map(/* index -> xLabels[idx] */)
  }
  ...
}
```

`size` is a fixed `65` regardless of mode or label length. No `rotate` is set
anywhere in the axis config (confirmed via `grep -r rotate analytics-web-app/src`
— no hits outside uPlot's own type declarations). This function is pure and
already unit-tested in `analytics-web-app/src/components/__tests__/xychart-axis.test.ts`,
extracted from `XYChart.tsx` in #1089 specifically so axis formatting logic
stays isolated and testable — this fix stays inside that same module.

`buildXAxisConfig`'s return value is used verbatim as `axes[0]` in both the
multi-series and single-series uPlot option objects built in `XYChart.tsx`
(`xychart-axis.ts` is called once at `XYChart.tsx:618`, result reused at
`XYChart.tsx:705` and `XYChart.tsx:998`).

### How uPlot resolves axis rotation and size

uPlot's `Axis.Rotate` and `Axis.Size` types (`node_modules/uplot/dist/uPlot.d.ts:1054,1111`)
each accept either a static number or a function:

```ts
type Rotate = number | ((self: uPlot, values: (string|number)[], axisIdx: number, foundSpace: number) => number)
type Size   = number | ((self: uPlot, values: string[], axisIdx: number, cycleNum: number) => number)
```

Tracing uPlot's internal `axesCalc(cycleNum)` (`uPlot.iife.js:4469-4524`) shows
the exact per-axis call sequence, once per layout convergence cycle:

```js
let values = axis._values = axis.values(self, ..., i, _space, incr)   // our tick-label formatter
axis._rotate = side == 2 ? axis.rotate(self, values, i, _space) : 0   // rotate() called with the REAL per-tick pixel space
axis._size = ceil(axis.size(self, values, i, cycleNum))               // size() called right after, same values, but NO foundSpace
```

Two things matter for the design:
1. `rotate()` receives `foundSpace` — the actual CSS-pixel space available
   per tick at the current layout — so it can decide whether labels fit.
2. `size()` does **not** receive `foundSpace`, only `values` and `cycleNum`.
   It runs immediately after `rotate()` for the same axis in the same cycle,
   so the only way for `size()` to know whether we rotated is a value shared
   between the two closures (uPlot itself does this internally via `axis._rotate`,
   a private field we should not depend on).

## Design

Extend the categorical branch of `buildXAxisConfig` with `rotate` and `size`
as functions instead of the current static `size: 65`. Both close over a
small piece of mutable state (`rotated: boolean`) that `rotate()` sets and
`size()` reads — safe because uPlot always calls `rotate()` immediately
before `size()` for the same axis in the same cycle (see trace above), and a
fresh `xAxisConfig` object (and thus fresh closure) is built per `XYChart`
render via `buildXAxisConfig()`, so no state leaks across chart instances.

### Overlap heuristic

Rather than measuring text with `ctx.measureText()` (would require a canvas
context inside a pure, unit-tested module, and the test suite has no canvas
mock — see Trade-offs), estimate label pixel width from character count:

```ts
const AVG_CHAR_WIDTH_PX = 6 // approx. glyph advance for the 11px sans-serif axis font
function estimateLabelWidth(label: string): number {
  return label.length * AVG_CHAR_WIDTH_PX
}
```

This is intentionally coarse — the only decision it drives is a binary
rotate/don't-rotate threshold, not pixel-perfect layout, so an average-width
approximation is sufficient (correct to within a character or two never
matters visually: the transition between horizontal and rotated is only ever
crossed near the threshold, and the failure mode is "rotated one label group
sooner or later than strictly necessary," not incorrect rendering).

### `rotate`

```ts
xAxisConfig.rotate = (_u, values, _axisIdx, foundSpace) => {
  const maxWidth = Math.max(0, ...values.map((v) => estimateLabelWidth(String(v))))
  rotated = maxWidth + TICK_LABEL_PADDING_PX > foundSpace
  return rotated ? ROTATE_DEG : 0
}
```

- `TICK_LABEL_PADDING_PX` (e.g. `8`): small buffer so labels rotate slightly
  before they'd visually touch, not exactly at the pixel where they'd overlap.
- `ROTATE_DEG = -45`: matches the issue's suggestion and common bar-chart
  convention (uPlot only honors `rotate` on the bottom axis, which is the only
  axis this applies to here).

### `size`

```ts
xAxisConfig.size = (_u, values) => {
  if (!rotated) return BASE_SIZE // 65, unchanged from today
  const maxWidth = Math.max(0, ...values.map((v) => estimateLabelWidth(String(v))))
  const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
  const rotatedExtent = maxWidth * Math.sin(angleRad) + LABEL_LINE_HEIGHT_PX * Math.cos(angleRad)
  return Math.min(MAX_ROTATED_SIZE, Math.ceil(rotatedExtent) + AXIS_CHROME_PX)
}
```

- `LABEL_LINE_HEIGHT_PX` (e.g. `14`): approximate single-line text height for
  the 11px axis font.
- `AXIS_CHROME_PX` (e.g. `20`): tick length + label gap, matching the existing
  visual spacing (`ticks`/`gap` are otherwise untouched).
- `MAX_ROTATED_SIZE` (e.g. `160`): hard ceiling so one pathologically long
  label can't consume most of the chart's vertical space — the label itself
  will simply run past the axis box height in that rare case, same failure
  mode uPlot already has for any axis, rather than a chart that's all axis.

### Why not vary the angle continuously

The issue's "ideally" suggestion floats scaling rotation/size with label
length continuously. A fixed two-state design (0° or -45°) is simpler,
matches the issue's own concrete suggestion ("set `rotate` (e.g. -45°)"), and
is what every common charting library defaults to for this exact problem.
Variable-angle rotation would need a second heuristic (choosing the "best"
angle) for no legibility benefit over a fixed -45°, since -45° already
roughly maximizes labels-per-pixel-width for typical label lengths.

## Implementation Steps

1. **`analytics-web-app/src/components/xychart-axis.ts`**
   - Add module-level constants: `ROTATE_DEG`, `AVG_CHAR_WIDTH_PX`,
     `TICK_LABEL_PADDING_PX`, `LABEL_LINE_HEIGHT_PX`, `AXIS_CHROME_PX`,
     `MAX_ROTATED_SIZE`, and keep `65` as `BASE_SIZE` (used both as the
     default `size` and the flat-label return value).
   - Add `estimateLabelWidth(label: string): number` (exported for direct
     unit testing, matching the module's existing export style).
   - Inside the `xAxisMode === 'categorical' && xLabels` branch, declare
     `let rotated = false` and set `xAxisConfig.rotate` / `xAxisConfig.size`
     as described above, replacing the top-level static `size: 65` for this
     branch only (the `time`/`numeric` branches keep the static `size: 65`
     from the base config object).
   - Cast `values` elements to `string` defensively (`Rotate`'s type allows
     `string | number`; our categorical `values` closure only ever produces
     strings, but `size`'s declared type is `string[]` too so no cast should
     actually be needed — confirm during implementation).

2. **No changes needed in `XYChart.tsx`** — it already just spreads whatever
   `buildXAxisConfig` returns into `axes[0]` for both chart paths.

## Files to Modify

- `analytics-web-app/src/components/xychart-axis.ts` — add rotation/size heuristic to the categorical branch of `buildXAxisConfig`.
- `analytics-web-app/src/components/__tests__/xychart-axis.test.ts` — new test cases (see Testing Strategy).

## Trade-offs

**Character-count heuristic vs. `ctx.measureText()`**
- `measureText` would be pixel-accurate but requires a live canvas 2D context.
  uPlot does pass `self` (the `uPlot` instance, which exposes `self.ctx`) into
  both `rotate` and `size`, so it's *technically* available — but:
  - The project's Vitest setup has no canvas mock (`grep -rn canvas-mock`
    found nothing, no `canvas` npm package installed), so directly unit
    testing a `measureText`-based function would require adding a canvas
    mocking dependency for a threshold decision that doesn't need pixel
    accuracy.
  - `measureText` width is also devicePixelRatio-sensitive here (uPlot scales
    `axis.font` by `pxRatio` internally — `uPlot.iife.js:2895-2898`), so a
    correct implementation would need to convert back to CSS pixels to
    compare against `foundSpace` (itself in CSS pixels), adding complexity
    with no legibility upside over the coarser estimate.
- A character-count heuristic is a pure function of the label strings, keeps
  the module dependency-free, and is trivially unit-testable — consistent
  with [[feedback_no_overdesigned_tests]]: scale precision to what the fix
  actually needs (a binary threshold), not to pixel-perfect layout.

**Shared closure boolean vs. reading uPlot's private `axis._rotate`**
- uPlot already stores the rotate decision on `axis._rotate` before calling
  `size()`, which `size(self, values, axisIdx)` could read via
  `self.axes[axisIdx]._rotate`. Rejected: `_rotate` is an underscore-prefixed
  internal field, not part of the public `uPlot.d.ts` API — depending on it
  would break silently on a uPlot upgrade. A closure variable owned by our
  own code has no such risk and is exactly as simple.

**Fixed -45° vs. continuously variable angle** — see Design section above.

## Testing Strategy

- **Unit tests** in `xychart-axis.test.ts`, alongside the existing
  `buildXAxisConfig` describe block:
  - `estimateLabelWidth` returns `label.length * AVG_CHAR_WIDTH_PX`.
  - Calling `rotate(u, ['a', 'b'], 0, 60)` with short labels and ample
    `foundSpace` returns `0`.
  - Calling `rotate(u, [longLabel], 0, 20)` with a long label and a narrow
    `foundSpace` returns `ROTATE_DEG` (`-45`).
  - After a `rotate()` call that triggers rotation, the subsequent `size()`
    call returns a value `> BASE_SIZE` and `<= MAX_ROTATED_SIZE` — this also
    documents/enforces the call-order contract (`rotate()` before `size()`)
    that uPlot itself guarantees.
  - After a `rotate()` call that does *not* trigger rotation, `size()`
    returns exactly `BASE_SIZE` (unchanged behavior, covers the existing
    `axis.size` test at `xychart-axis.test.ts:12` still passing).
  - `time` and `numeric` modes keep static `size: 65` (no `rotate` set) —
    extend the existing tests for those branches to assert `axis.rotate` is
    `undefined`.
- **Manual/visual**: build (or reuse) a chart cell with `xAxisMode:
  'categorical'` and ~30 long category labels (e.g.
  `++product+channel+branch-CL-123456`-style strings) at a normal panel
  width; confirm labels rotate and are legible, and that a chart with a
  handful of short categories (e.g. usernames) still renders flat, unchanged
  from today.
- Run `cd analytics-web-app && yarn lint && yarn type-check && yarn test`.

## Open Questions

- None — the fix is fully contained in `buildXAxisConfig` with no API or
  data-shape changes, and the existing unit-test file already covers the
  surrounding categorical-mode behavior this extends.
