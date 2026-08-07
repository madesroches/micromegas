# Categorical Chart X-Axis: Rotate Overlapping Labels Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1425

## Overview

Chart cells in categorical X-axis mode render tick labels horizontally at a
fixed 65px axis height. When labels are long strings (e.g. version/build
identifiers), labels overlap and become unreadable even while per-tick space
stays at or above uPlot's configured `space: 60` floor (`xAxisConfig.space =
60`, see Current State) — that's the regime this plan targets. This adds
adaptive rotation to `buildXAxisConfig`: labels tilt to -45° and the axis
grows to fit them, but only when the available per-tick space is too narrow
to fit the labels horizontally — short labels keep rendering flat, as today.

Pushing category *count* high enough instead drops per-tick space below that
60px floor, at which point uPlot's own layout (`axesCalc` /
`getIncrSpace`/`findIncr`) returns early and blanks the axis entirely —
before `rotate()` or `size()` ever run. That's a pre-existing gap independent
of rotation (arguably its own issue) and out of scope here.

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

uPlot's `Axis.Rotate` and `Axis.Size` types (`node_modules/uplot/dist/uPlot.d.ts:1069,1111`)
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

This logic only runs when uPlot actually calls `rotate()`/`size()` for the
axis at all, which requires per-tick space to clear uPlot's own `space: 60`
floor (see Current State and Overview) — high category counts that push
per-tick space below that floor blank the axis before this code ever
executes, regardless of label length.

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

### `padding`

`rotate`/`size` only grow the axis's own vertical band — they reserve no
extra room to the *right* of the plot area, which is where a -45°-rotated
label actually overflows. uPlot anchors each rotated label at its tick and
draws it down-and-to-the-right (`uPlot.iife.js:4614-4667`), so the last
category's label can extend well past the plot's right edge. The only
cushion uPlot gives that edge by default is `autoPadSide`
(`uPlot.iife.js:3804-3816`): `yAxisOpts.size / 2` (~25px) when there's *no*
right-side y-axis, `0` when there is one — and `XYChart.tsx:~264-271`
(`unitScaleInfo`) assigns the *first* unit's y-axis `side: 1` (right)
whenever there are 2+ visible series, regardless of how many distinct units
those series share, so any multi-series categorical chart (2+ series,
regardless of unit count) gets no right-side cushion at all — only the true
single-series path (no explicit `side`, defaulting to left) is cushioned. A
~20-char rotated label's horizontal projection — per the `rightPadding`
formula below, `(20·AVG_CHAR_WIDTH_PX + LABEL_LINE_HEIGHT_PX)·cos45° ≈ 95px`
— far exceeds either buffer.

uPlot exposes this as a top-level `Options.padding` (`uPlot.d.ts:384`), a
4-tuple `[top, right, bottom, left]` of `PaddingSide = number | null |
(self, side, sidesWithAxes, cycleNum) => number` — not an `Axis`-level
field, so it can't be folded into `xAxisConfig` alone. uPlot calls it once
per convergence cycle, immediately *after* that cycle's `axesCalc` (i.e.
after `rotate`/`size` run for every axis): `convergeSize()` calls
`axesCalc(cycleNum)` then `paddingCalc(cycleNum)`, in that order
(`uPlot.iife.js:3403-3404`). So a padding function can safely read
whatever shared state `rotate()`/`size()` just finished updating that same
cycle — the same guarantee the plan already relies on for `rotated`.

Today `size()` independently *recomputes* `maxWidth` from `values` rather
than reading something `rotate()` set — harmless there since both get the
same `values` array. A padding function gets no `values` at all
(`PaddingSide`'s signature is `(self, side, sidesWithAxes, cycleNum)`), so
`maxWidth` needs to become genuine shared closure state, not just a
same-named local in two places. `rightPadding` itself must be declared (as
`let rightPadding: uPlot.PaddingSide = null`) alongside `const xAxisConfig` at
the top of the function, before the if/else-if chain — otherwise it's out of
scope at the function's single trailing return, and the `time`/`numeric`
branches have nothing to give them their `null` default, which degrades to
uPlot's own `autoPadSide` behavior on the right edge (a literal `0` would
instead override that cushion away, since uPlot's `ifNull(p, autoPadSide)`
only falls back to `autoPadSide` when the option is `null`). The categorical
branch then *reassigns* it; it doesn't redeclare it:

```ts
// at the top of the function, alongside `const xAxisConfig`:
let rightPadding: uPlot.PaddingSide = null

// inside the `xAxisMode === 'categorical' && xLabels` branch:
let rotated = false
let maxWidth = 0

xAxisConfig.rotate = (_u, values, _axisIdx, foundSpace) => {
  maxWidth = Math.max(0, ...values.map((v) => estimateLabelWidth(String(v))))
  rotated = maxWidth + TICK_LABEL_PADDING_PX > foundSpace
  return rotated ? ROTATE_DEG : 0
}
xAxisConfig.size = (_u) => {
  if (!rotated) return BASE_SIZE
  const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
  const rotatedExtent = maxWidth * Math.sin(angleRad) + LABEL_LINE_HEIGHT_PX * Math.cos(angleRad)
  return Math.min(MAX_ROTATED_SIZE, Math.ceil(rotatedExtent) + AXIS_CHROME_PX)
}

rightPadding = () => {
  if (!rotated) return 0
  const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
  const horizontalExtent = maxWidth * Math.cos(angleRad) + LABEL_LINE_HEIGHT_PX * Math.sin(angleRad)
  return Math.min(MAX_ROTATED_SIZE, Math.ceil(horizontalExtent))
}

// after the if/else-if chain, the function's single existing trailing return:
return { axis: xAxisConfig, rightPadding }
```

`buildXAxisConfig` now returns `{ axis, rightPadding }` instead of a bare
`uPlot.Axis`. Both call sites in `XYChart.tsx` (multi-series `~705`,
single-series `~998`) destructure it and set `padding: [null, rightPadding,
null, null]` on their uPlot `Options` object — neither currently sets
`padding` at all (confirmed via grep), so `null` on the other three sides
keeps uPlot's default `autoPadSide` behavior there, and `rightPadding`
defaults to `null` too, preserving that same cushion on the right edge for
`time`/`numeric` mode and any non-rotated categorical chart — it only
overrides the right edge with a computed value once the categorical branch
actually rotates. `MAX_ROTATED_SIZE` doubles as the horizontal cap
too: at `ROTATE_DEG = -45`, `sin` and `cos` are equal, so the horizontal and
vertical projections share the same magnitude and the same ceiling is
exactly as valid here.

This only reserves the *right* side, matching the app's current layout:
the categorical axis is always the bottom `axes[0]`, and with `ROTATE_DEG`
fixed negative its labels only ever lean right-and-down. If a future change
ever put a rotate-affected axis on a side with no fixed axis on its far
side (e.g. a left-anchored axis with no left y-axis), the same technique
would apply to `padding[3]`.

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
   - Change `buildXAxisConfig`'s return type from `uPlot.Axis` to
     `{ axis: uPlot.Axis; rightPadding: uPlot.PaddingSide }`.
   - Declare `let rightPadding: uPlot.PaddingSide = null` alongside `const
     xAxisConfig = {...}` at the top of the function, before the
     if/else-if chain — this is what gives the `time`/`numeric` branches
     their `null` default (degrading to uPlot's own `autoPadSide` cushion on
     the right edge, unlike a literal `0`, which would override it away) at
     the function's single trailing `return { axis: xAxisConfig,
     rightPadding }`.
   - Inside the `xAxisMode === 'categorical' && xLabels` branch, declare
     `let rotated = false` and `let maxWidth = 0`, set `xAxisConfig.rotate` /
     `xAxisConfig.size`, and *reassign* (not redeclare) `rightPadding = () =>
     {...}` as described in the Design section's `padding` subsection above,
     replacing the top-level static `size: 65` for this branch only (the
     `time`/`numeric` branches keep the static `size: 65` from the base
     config object and leave `rightPadding` at its hoisted `null` default).
   - Cast `values` elements to `string` defensively (`Rotate`'s type allows
     `string | number`; our categorical `values` closure only ever produces
     strings, but `size`'s declared type is `string[]` too so no cast should
     actually be needed — confirm during implementation).

2. **`analytics-web-app/src/components/XYChart.tsx`** — update the single
   call site (`XYChart.tsx:618`) to destructure `{ axis: xAxisConfig,
   rightPadding }`, and add `padding: [null, rightPadding, null, null]` to
   both uPlot `Options` objects (multi-series `~705`, single-series `~998`)
   that consume `xAxisConfig`; neither sets `padding` today.

## Files to Modify

- `analytics-web-app/src/components/xychart-axis.ts` — add rotation/size/padding heuristic to the categorical branch of `buildXAxisConfig`.
- `analytics-web-app/src/components/XYChart.tsx` — wire the new `rightPadding` return value into both uPlot `Options` objects.
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

- **Unit tests** in `xychart-axis.test.ts`:
  - Update all 4 existing cases in the `buildXAxisConfig` describe block
    (`time mode leaves values/incrs unset`, `categorical mode maps tick
    indices to labels`, `categorical without labels falls through`,
    `numeric mode abbreviates`) to destructure `const { axis } =
    buildXAxisConfig(...)` instead of `const axis = buildXAxisConfig(...)` —
    every one of them reads `axis.values`/`axis.incrs`/`axis.size` directly
    off the return value today, and all break once the return shape changes
    to `{ axis, rightPadding }`. This is a mechanical rewrite of the whole
    describe block, not just the two branches below gaining new assertions.
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
    returns exactly `BASE_SIZE` (unchanged behavior for the flat-label case).
  - `time` and `numeric` modes keep static `size: 65` (no `rotate` set) —
    extend the existing tests for those branches to assert `axis.rotate` is
    `undefined` and `rightPadding` is `null`.
  - Calling `rightPadding()` before any `rotate()` call, or after a
    `rotate()` call that doesn't trigger rotation, returns `0`.
  - After a `rotate()` call that triggers rotation, `rightPadding()` returns
    a value `> 0` and `<= MAX_ROTATED_SIZE` — same call-order contract as
    the `size()` test above (`rotate()` before `rightPadding()`).
- **Manual/visual**: build (or reuse) a chart cell with `xAxisMode:
  'categorical'` and ~10 long category labels (e.g.
  `++product+channel+branch-CL-123456`-style ~34-char strings) at a normal
  panel width (~800px) — enough categories to stay well above uPlot's
  `space: 60` per-tick floor, so `rotate()`/`size()` actually run; confirm
  labels rotate and are legible, and that a chart with a handful of short
  categories (e.g. usernames) still renders flat, unchanged from today. Do
  not use a much higher category count (e.g. ~30) at the same width as a
  rotation test: that pushes per-tick space below the `space: 60` floor and
  uPlot blanks the axis entirely before rotation logic ever runs — a
  separate pre-existing gap (see Overview), not evidence that rotation
  "isn't needed."
  Additionally, on that same rotated chart, confirm the **last** category's
  label is fully visible and not clipped at the chart's right edge, in both
  a true single-series chart (no explicit `side`, the only cushioned case)
  and a multi-series chart with 2+ visible series (which puts a `side: 1`
  right y-axis on the chart per `XYChart.tsx:~268`'s `unitScaleInfo` —
  the zero-right-cushion case the `padding` fix targets — regardless of
  whether those series share one unit or several; a multi-series/
  single-shared-unit chart is *not* a safe stand-in for the single-series
  control, since it still gets `side: 1` and zero cushion).
- Run `cd analytics-web-app && yarn lint && yarn type-check && yarn test`.

## Documentation

No user- or developer-facing documentation covers this chart's internals; no
docs updates required.

## Open Questions

- None — the fix stays within `xychart-axis.ts` plus a small, mechanical
  wiring change at `buildXAxisConfig`'s single call site in `XYChart.tsx`
  (destructure the new `{ axis, rightPadding }` return shape instead of a
  bare `uPlot.Axis`). The existing unit-test file covers the surrounding
  categorical-mode behavior this extends, but all 4 of its existing
  `buildXAxisConfig` cases must be updated to destructure `{ axis }` from
  the new return shape (see Testing Strategy) — a mechanical but total
  rewrite of that describe block, not a small extension.
