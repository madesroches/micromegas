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

Out of scope does *not* mean ignorable: this plan's right-side `padding`
shrinks `plotWidCss`, which is the very dimension per-tick space is derived
from, so a naive implementation would push charts that render today over
that cliff (see the `space` subsection under Design). The design must not
create new instances of the gap, even though it doesn't close the existing
one.

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

uPlot's `Axis.Rotate` (`node_modules/uplot/dist/uPlot.d.ts:1024`) and
`Axis.Size` (`uPlot.d.ts:983`) types each accept either a static number or a
function:

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

Four things matter for the design:
1. `rotate()` receives `foundSpace` — the actual CSS-pixel space available
   per tick at the current layout (`getIncrSpace` is called with
   `plotWidCss`, so this really is CSS pixels, matching our estimator's
   units) — so it can decide whether labels fit.
2. `size()` does **not** receive `foundSpace`, only `values` and `cycleNum`.
   It runs immediately after `rotate()` for the same axis in the same cycle,
   so the only way for `size()` to know whether we rotated is a value shared
   between the two closures (uPlot itself does this internally via `axis._rotate`,
   a private field we should not depend on).
3. `size()` is *also* called once at init, outside `axesCalc`, as
   `axis._size = axis.size(self, null, i, 0)` (`uPlot.iife.js:3785`) — note
   `values` is **`null`** and `rotate()` has not run yet. Our `size()` must
   therefore never dereference `values`; it reads the shared `maxWidth`
   instead, and returns `BASE_SIZE` on this first call because `rotated` is
   still `false`. (The top-level `padding` functions are likewise invoked
   once at init with `cycleNum: 0` — `uPlot.iife.js:3819` — before any
   `rotate()` call.)
4. `axesCalc` bails out with `if (_space == 0) return` (`uPlot.iife.js:4501`)
   *before* reaching the `rotate()`/`size()` lines. So when the per-tick
   space floor isn't met, our closure state is not merely unset — it keeps
   whatever the previous cycle left there. See the `space` subsection for
   why that matters.

`convergeSize()` runs at most `CYCLE_LIMIT = 3` cycles (`uPlot.iife.js:3393`,
`3406`); past that it stops with whatever the last cycle produced, converged
or not. The design below settles in 2 cycles (cycle 1 flips `rotated` and
grows both `size` and `rightPadding`; cycle 2 recomputes identical values and
converges), comfortably inside that limit — with or without a right y-axis.
It's true that the one-time `_padding` init at `uPlot.iife.js:3819` runs with
`sidesWithAxes` still all-`false` (`axes.forEach(initAxis)`, which populates
it, doesn't run until `:6094`), so `_padding[1]` starts at `0` regardless of
the real layout (see the `padding` subsection) — but that init call happens
before `_setSize`/`calcSize` ever run. `convergeSize()` itself only starts
once `_init()` calls `_setSize(opts.width, opts.height)` (`:6089`), which
calls `calcSize` (`:3372`) → `calcPlotRect` (`:3422`), assigning
`sidesWithAxes[0..3]` (`:3462-3465`) from the real axis layout *before*
`commit()` → `convergeSize()` runs cycle 1. So `paddingCalc(1)`, the first
real cycle, already sees the correct `sidesWithAxes` — there's no stale value
left over from the init call for it to correct.

## Design

Extend the categorical branch of `buildXAxisConfig` with `rotate`, `size` and
`space` as functions instead of the current static `size: 65` / `space: 60`,
plus a top-level `padding` value returned alongside the axis. All four close
over two pieces of mutable state — `rotated: boolean` and `maxWidth: number`
— that `rotate()` writes and the other three read. That's safe because uPlot
always calls `rotate()` immediately before `size()` for the same axis in the
same cycle, and `paddingCalc` after `axesCalc` in the same cycle (see trace
above); `space()` is the one reader that runs *before* `rotate()` and so
intentionally reads the previous cycle's value (see its subsection). A fresh
`xAxisConfig` object — and thus a fresh closure — is built per `XYChart`
render via `buildXAxisConfig()`, so no state leaks across chart instances.

Rotation is only ever *initiated* when uPlot actually calls `rotate()` for
the axis, which requires per-tick space to clear the `space` floor (see
Current State and Overview) — high category counts that push per-tick space
below that floor blank the axis before this code ever executes, regardless of
label length. The `space` subsection covers why that floor has to become
rotation-dependent rather than staying at a static 60.

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
  maxWidth = Math.max(0, ...values.map((v) => estimateLabelWidth(String(v))))
  rotated = maxWidth + TICK_LABEL_PADDING_PX > foundSpace
  return rotated ? ROTATE_DEG : 0
}
```

`rotate()` is the sole writer of both shared values (`rotated` and
`maxWidth`); `size()`, `space()`, and `rightPadding()` are all pure readers.

- `TICK_LABEL_PADDING_PX` (e.g. `8`): small buffer so labels rotate slightly
  before they'd visually touch, not exactly at the pixel where they'd overlap.
- `ROTATE_DEG = -45`: matches the issue's suggestion and common bar-chart
  convention (uPlot only honors `rotate` on the bottom axis, which is the only
  axis this applies to here).

### `size`

```ts
xAxisConfig.size = (self) => {
  if (!rotated) return BASE_SIZE // 65, unchanged from today
  const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
  const rotatedExtent = maxWidth * Math.sin(angleRad) + LABEL_LINE_HEIGHT_PX * Math.cos(angleRad)
  const cap = Math.min(MAX_ROTATED_SIZE, Math.round(self.height * ROTATED_SIZE_FRACTION))
  return Math.min(cap, Math.ceil(rotatedExtent) + AXIS_CHROME_PX)
}
```

`size()` now reads `self` in addition to the shared `maxWidth` (still not
recomputed from `values`: uPlot's init call passes `values === null`,
`uPlot.iife.js:3785`, see point 3 above, so a `values.map(...)` here would
only be safe by accident). Reading `self.height` is safe at that same init
call even though `self.height` isn't set yet at that point (`calcSize` hasn't
run) — the `!rotated` early return fires first, since `rotated` only ever
becomes `true` from inside `rotate()`, which uPlot only calls during a real
convergence cycle, by which point `calcSize` has already assigned
`self.height`. So the `self.height` read is never reached before it's valid.

- `LABEL_LINE_HEIGHT_PX` (e.g. `14`): approximate single-line text height for
  the 11px axis font.
- `AXIS_CHROME_PX` (e.g. `20`): tick length + label gap, matching the existing
  visual spacing (`ticks`/`gap` are otherwise untouched).
- `MAX_ROTATED_SIZE` (e.g. `160`) and `ROTATED_SIZE_FRACTION` (e.g. `0.4`):
  the effective ceiling is `min(MAX_ROTATED_SIZE, round(self.height *
  ROTATED_SIZE_FRACTION))` — relative to the chart's actual height, not just
  an absolute pixel count. An absolute-only cap doesn't do what it's meant
  to at the app's real minimum chart size: `XYChart.tsx:308` floors canvas
  height at `Math.round(Math.max(250, rect.height - 32))` and
  `ChartCell.tsx:608` defaults `defaultHeight: 250`, so a bare 160px ceiling
  is 64% of a default-sized chart's height — the opposite of "can't consume
  most of the chart's vertical space." At `ROTATED_SIZE_FRACTION = 0.4` the
  axis never exceeds 40% of `self.height` regardless of canvas size, and
  `MAX_ROTATED_SIZE` only becomes the binding ceiling on taller charts
  (`self.height` above `MAX_ROTATED_SIZE / ROTATED_SIZE_FRACTION = 400`). One
  consequence: at the app's default ~250px chart height (cap 100px), a label
  whose uncapped `rotatedExtent + AXIS_CHROME_PX` exceeds that is truncated
  at the axis edge — expected for long labels at typical chart heights, not
  only a rare pathological case (see Testing Strategy for the concrete
  ~34-char example).

### `space`

The current static `xAxisConfig.space = 60` must become rotation-aware, or
the `padding` fix below regresses charts that render correctly today.

`calcPlotRect` applies right padding as `plotWidCss -= _padding[1] +
_padding[3]` (`uPlot.iife.js:3468`), and `axesCalc` derives the x-axis's
`foundSpace` from that same `plotWidCss`. So reserving a large chunk of the
plot's width on the right (~154px in the example below) directly subtracts
from the quantity that has to clear the `space` floor. Worked example, 10
categories with ~34-char labels in an 800px plot with one left-side y-axis
(`size: 90`, no right axis):

- cycle 1: `plotWidCss = 800 − 90 − 25 = 685` (90 for the y-axis, 25 for the
  no-right-axis `DEFAULT_RIGHT_CUSHION_PX` cushion — see `padding` below) →
  `foundSpace = 68.5 ≥ 60` → axis renders → labels are too wide for that
  space → `rotate()` fires → `rightPadding` jumps to `≈154` (the capped
  horizontal projection for a ~34-char label, see `padding` below).
- cycle 2: `plotWidCss = 800 − 90 − 154 = 556` → `foundSpace = 55.6 < 60` →
  `findIncr` returns `[0, 0]` → `axesCalc` hits its `_space == 0` early
  return and `drawAxesGrid` `continue`s → **the entire x-axis blanks**.

(This matches the Testing Strategy's own arithmetic for the same
configuration — see the ~10-category manual test below.)

That is the fix converting a chart with overlapping-but-readable labels into
a chart with no x-axis at all — a regression, not the pre-existing gap. Fix:

```ts
xAxisConfig.space = () => (rotated ? ROTATED_MIN_SPACE_PX : BASE_MIN_SPACE_PX)
```

- `BASE_MIN_SPACE_PX = 60`: unchanged from today's static value.
- `ROTATED_MIN_SPACE_PX` (e.g. `20`): once labels are tilted they no longer
  need to fit horizontally within a tick slot — adjacent rotated baselines
  are `foundSpace · sin45°` apart, so clearing `LABEL_LINE_HEIGHT_PX` only
  takes `14 / 0.707 ≈ 20px` of per-tick width.

Because this branch pins `incrs = [1]`, `space` is *purely* a blank/don't-blank
floor here, not a tick-density control: `findIncr` can only return
`[1, foundSpace]` or `[0, 0]`, so lowering the floor never adds or removes
ticks — every category index is a tick either way. That keeps the change
narrowly scoped to the failure above.

`space()` is called from `getIncrSpace` *before* `rotate()` in the same cycle,
so it reads the previous cycle's `rotated` — the same one-cycle lag uPlot's
own convergence loop is built to absorb. The state machine is monotone and
cannot oscillate: rotating can only *shrink* `foundSpace` (padding grows),
which can only make the rotate predicate more true; and un-rotating can only
*grow* `foundSpace`, which can only make it more false. So `rotated` changes
at most once after the initial cycle, well inside `CYCLE_LIMIT = 3`.

This does **not** close the pre-existing gap from the Overview: a chart with
enough categories to miss the 60px floor on cycle 1 never reaches `rotate()`,
so `rotated` stays `false`, the floor stays 60, and the axis blanks exactly as
it does today. The rotated floor only protects charts that *did* start
rotating from being blanked by our own padding.

One consequence of `axesCalc`'s early return (point 4 above) is worth
recording: if a later resize drops `foundSpace` below even the rotated floor,
`rotate()` doesn't run, so `rotated`/`maxWidth` stay latched at their last
values and `rightPadding` keeps reserving its ~95px while the axis is blank.
Widening the window recovers normally (the floor is met again and `rotate()`
re-evaluates); the only artifact is that the blank/render threshold has a
little hysteresis. Acceptable — the alternative is resetting shared state
from a callback uPlot doesn't call in that path.

### `padding`

`rotate`/`size` only grow the axis's own vertical band — they reserve no
extra room to the *right* of the plot area, which is where a -45°-rotated
label actually overflows. uPlot anchors each rotated label at its tick and
draws it down-and-to-the-right (`uPlot.iife.js:4614-4667`), so the last
category's label can extend well past the plot's right edge. The only
cushion uPlot gives that edge by default is `autoPadSide`
(`uPlot.iife.js:3804-3816`): `round(yAxisOpts.size / 2)` — exactly **25px**,
since `yAxisOpts.size` defaults to `50` (`uPlot.iife.js:1614`) — when there's
*no* right-side y-axis, `0` when there is one. And `XYChart.tsx:~264-271`
(`unitScaleInfo`) assigns the *first* unit's y-axis `side: 1` (right)
whenever there are 2+ visible series, regardless of how many distinct units
those series share, so any multi-series categorical chart (2+ series,
regardless of unit count) gets no right-side cushion at all — only the true
single-series path (no explicit `side`, defaulting to left) is cushioned.

> Careful reading `unitScaleInfo`: the trailing comment on that line reads
> `// 1=left, 3=right`, which is **backwards** relative to uPlot, where
> `side` is `0=top, 1=right, 2=bottom, 3=left` (`Axis.Side`). The *values*
> are what this plan reasons about, and `side: 1` really does place a
> right-hand axis. Don't "fix" the values to match the comment; if anything,
> fix the comment. (Out of scope here — flagged only so the padding analysis
> isn't misread.)

A
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

A padding function gets no `values` at all (`PaddingSide`'s signature is
`(self, side, sidesWithAxes, cycleNum)`), which is the second reason
`maxWidth` is genuine shared closure state written by `rotate()` rather than
recomputed per callback (the first being `size()`'s `values === null` init
call). `rightPadding` itself must be declared (as
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
xAxisConfig.size = (self) => {
  if (!rotated) return BASE_SIZE
  const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
  const rotatedExtent = maxWidth * Math.sin(angleRad) + LABEL_LINE_HEIGHT_PX * Math.cos(angleRad)
  const cap = Math.min(MAX_ROTATED_SIZE, Math.round(self.height * ROTATED_SIZE_FRACTION))
  return Math.min(cap, Math.ceil(rotatedExtent) + AXIS_CHROME_PX)
}
xAxisConfig.space = () => (rotated ? ROTATED_MIN_SPACE_PX : BASE_MIN_SPACE_PX)

rightPadding = (self, _side, sidesWithAxes) => {
  // Not rotated: reproduce uPlot's own autoPadSide result for the right edge,
  // since a function's numeric return can never fall back to it (see below).
  // Mirrors autoPadSide's side-1 branch exactly (uPlot.iife.js:3812-3813),
  // including its (hasTopAxis || hasBtmAxis) guard — not just hasRgtAxis.
  if (!rotated) return (sidesWithAxes[0] || sidesWithAxes[2]) && !sidesWithAxes[1] ? DEFAULT_RIGHT_CUSHION_PX : 0
  const angleRad = (Math.abs(ROTATE_DEG) * Math.PI) / 180
  const horizontalExtent = maxWidth * Math.cos(angleRad) + LABEL_LINE_HEIGHT_PX * Math.sin(angleRad)
  const cap = Math.min(MAX_ROTATED_SIZE, Math.round(self.width * ROTATED_SIZE_FRACTION))
  const capped = Math.min(cap, Math.ceil(horizontalExtent))
  // A right y-axis (XYChart.tsx's `size: 90`) already reserves clearance that
  // rotated labels can safely cross into (axis-label drawing is unclipped,
  // and rotated x labels sit ~15px below the plot bottom), so subtract that
  // band instead of reserving the full projection on top of it.
  return Math.max(0, capped - (sidesWithAxes[1] ? RIGHT_AXIS_SIZE_PX : 0))
}

// after the if/else-if chain, the function's single existing trailing return:
return { axis: xAxisConfig, rightPadding }
```

`buildXAxisConfig` now returns `{ axis, rightPadding }` instead of a bare
`uPlot.Axis`. Both call sites in `XYChart.tsx` (multi-series `~705`,
single-series `~998`) destructure it and set `padding: [null, rightPadding,
null, null]` on their uPlot `Options` object — neither currently sets
`padding` at all (confirmed via grep), so `null` on the other three sides
keeps uPlot's default `autoPadSide` behavior there, and in `time`/`numeric`
mode `rightPadding` stays at its hoisted `null` and keeps that cushion on the
right edge too.

The categorical branch is different, and this is the subtlety the
non-rotated `(sidesWithAxes[0] || sidesWithAxes[2]) && !sidesWithAxes[1] ?
DEFAULT_RIGHT_CUSHION_PX : 0` line above exists for. uPlot normalizes the
option once, at init:

```js
const padding = (opts.padding || [...]).map(p => fnOrSelf(ifNull(p, autoPadSide)))   // 3818
```

`ifNull` substitutes `autoPadSide` only for a `null` **option value**. Once
we install a *function*, whatever number it returns is final — there is no
per-call fallback, and `PaddingSide`'s function form is typed
`=> number`, so it cannot return `null` to opt back in. A naive
`if (!rotated) return 0` would therefore silently strip the 25px cushion from
every non-rotated categorical chart — precisely the cushion that keeps the
last *horizontal* label from being chopped, i.e. a regression in the exact
case this plan is supposed to leave untouched. Reproducing `autoPadSide`'s
result takes reading three slots, not one: `sidesWithAxes[1]` is
`hasRgtAxis`, but `autoPadSide`'s own side-1 branch (`uPlot.iife.js:3812-3813`)
is also gated on `hasTopAxis || hasBtmAxis` (`sidesWithAxes[0] ||
sidesWithAxes[2]`) — with no axis on the top or bottom, the right edge gets
no cushion either, regardless of `hasRgtAxis`. In this app the categorical
axis is always the bottom axis, so `sidesWithAxes[2]` is `true` on every real
convergence cycle and the guard is moot in practice — except at the one-time
`_padding` init call (`uPlot.iife.js:3819`), which runs *before*
`axes.forEach(initAxis)` (`:6094`) has populated `sidesWithAxes`, so it always
sees `[false, false, false, false]` there and returns `0` regardless of the
real axis layout. `paddingCalc(1)`'s first real call corrects it on the next
cycle (see Current State).

- `DEFAULT_RIGHT_CUSHION_PX = 25`: mirrors `round(yAxisOpts.size / 2)` with
  uPlot's default `yAxisOpts.size = 50`. It is a mirror of a library
  constant, so note it as such — if a future uPlot upgrade changes that
  default, this drifts silently (a 25-vs-whatever px cushion, not a
  correctness break).

A `Math.max` with the cushion is still not needed for the no-right-axis case:
the rotated projection is ~95px for a 20-char label and grows from there,
always well above 25. But when a right y-axis is present
(`sidesWithAxes[1]`), `calcAxesRects` places it at `plotLft + plotWid` and
grows outward, so it already reserves `RIGHT_AXIS_SIZE_PX` (90, mirroring
`XYChart.tsx`'s `size: 90`, the same way `DEFAULT_RIGHT_CUSHION_PX` mirrors
uPlot's 25) of clearance that rotated labels can safely cross — reserving the
full projection on top of that band would double-count it and blank out most
of the padding as empty space. The rotated branch therefore subtracts
`RIGHT_AXIS_SIZE_PX` from the capped projection when a right axis exists,
floored at `0` via `Math.max` so a short rotated label with a right axis
still reduces cleanly to no extra padding rather than a negative one.
The *cost* of reserving a capped pixel count is not the same on both axes:
vertical `size` only shrinks `plotHgtCss`, while horizontal `rightPadding`
shrinks `plotWidCss` (`uPlot.iife.js:3468`), which is the exact quantity
`getIncrSpace` (`:4501`) divides to get `foundSpace` — so a capped
`rightPadding` feeds back into the same rotate/blank decision the `space`
subsection covers, and a fixed absolute cap would be a much bigger bite out
of the available width than out of the available height on a narrow chart
cell. So `rightPadding`'s cap is width-relative, using the same
`ROTATED_SIZE_FRACTION` as `size()` but applied to `self.width`:
`min(MAX_ROTATED_SIZE, round(self.width * ROTATED_SIZE_FRACTION))`. At
`XYChart.tsx:307`'s 400px width floor that's `160px` — 40% of the canvas by
design, matching the same fraction `size()` uses for height. That reframes
what used to be flagged as an "unusually narrow panel" risk: 400px is the
app's enforced *minimum* chart width, not a rare case, so the fraction is
chosen to be a bound the design accepts at that floor, not an edge case to
caveat around. The rotation-aware floor (`ROTATED_MIN_SPACE_PX = 20`) still
absorbs the residual feedback into `foundSpace`.

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
     `MAX_ROTATED_SIZE`, `ROTATED_SIZE_FRACTION` (0.4, the fraction of
     `self.height`/`self.width` the rotated cap is relative to — see the
     Design section's `size` and `padding` subsections), `DEFAULT_RIGHT_CUSHION_PX`
     (25, mirroring uPlot's `round(yAxisOpts.size / 2)`), `RIGHT_AXIS_SIZE_PX`
     (90, mirroring `XYChart.tsx`'s right-axis `size: 90`),
     `ROTATED_MIN_SPACE_PX` (20), and keep `65`
     as `BASE_SIZE` (used both as the default `size` and the flat-label
     return value) and `60` as `BASE_MIN_SPACE_PX` (today's static
     `space`, still used by the `numeric` branch as a plain number).
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
     `xAxisConfig.size` / `xAxisConfig.space`, and *reassign* (not redeclare)
     `rightPadding = (_u, _side, sidesWithAxes) => {...}` as described in the
     Design section's `space` and `padding` subsections above, replacing the
     top-level static `size: 65` **and** this branch's static `space = 60`
     for this branch only (the `time`/`numeric` branches keep the static
     `size: 65` from the base config object, `numeric` keeps its static
     `space = 60`, and both leave `rightPadding` at its hoisted `null`
     default).
   - `xAxisConfig.size` takes only `self` (for `self.height`, used by the
     height-relative cap) — do **not** read `values` in it. uPlot's init
     call passes `values === null` (`uPlot.iife.js:3785`); `self.height` is
     likewise unset at that same call, but the `!rotated` early return means
     neither is ever read there (see the `size` subsection).
   - `xAxisConfig.space` is assigned a function; `Axis.Space`
     (`uPlot.d.ts:985`) accepts one, so this type-checks without a cast.
   - Cast `values` elements to `string` defensively in `rotate` (`Rotate`'s
     type allows `string | number`); `size` no longer touches `values` at
     all, so its `string[]` parameter type is moot.

2. **`analytics-web-app/src/components/XYChart.tsx`** — update the single
   call site (`XYChart.tsx:618`) to destructure `{ axis: xAxisConfig,
   rightPadding }`, and add `padding: [null, rightPadding, null, null]` to
   both uPlot `Options` objects (multi-series `~705`, single-series `~998`)
   that consume `xAxisConfig`; neither sets `padding` today.

## Files to Modify

- `analytics-web-app/src/components/xychart-axis.ts` — add the rotation/size/space/padding heuristic to the categorical branch of `buildXAxisConfig`.
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
    call (passed a stub `self` with a `.height`, e.g. `{ height: 250 } as
    uPlot`) returns a value `> BASE_SIZE` and `<= Math.min(MAX_ROTATED_SIZE,
    Math.round(250 * ROTATED_SIZE_FRACTION))` — this also documents/enforces
    both the call-order contract (`rotate()` before `size()`) that uPlot
    itself guarantees and that the cap is height-relative, not just the flat
    `MAX_ROTATED_SIZE`.
  - After a `rotate()` call that does *not* trigger rotation, `size()`
    returns exactly `BASE_SIZE` (unchanged behavior for the flat-label case).
  - `size()` called with no arguments at all (uPlot's init call shape,
    `values === null` and, before any `rotate()` call, `self` undefined too)
    returns `BASE_SIZE` and does not throw — guards both the regression of
    reintroducing a `values.map(...)` in `size` and of reading `self.height`
    before the `!rotated` guard.
  - `time` and `numeric` modes keep static `size: 65` (no `rotate` set) —
    extend the existing tests for those branches to assert `axis.rotate` is
    `undefined` and `rightPadding` is `null`. `numeric` also keeps a numeric
    `axis.space === 60` (not a function).
  - `space()` returns `BASE_MIN_SPACE_PX` (60) before any `rotate()` call and
    after a non-rotating one, and `ROTATED_MIN_SPACE_PX` (20) after a
    rotating one — the assertion that keeps our own `padding` from blanking
    the axis.
  - `rightPadding(u, 1, [false, false, true, true], 0)` before any `rotate()`
    call (no right axis) returns `DEFAULT_RIGHT_CUSHION_PX` (25), and with
    `sidesWithAxes[1] === true` returns `0` — i.e. it reproduces uPlot's
    `autoPadSide` rather than collapsing the cushion to `0`.
  - After a `rotate()` call that triggers rotation, `rightPadding()` (passed
    a stub `self` with a `.width`, e.g. `{ width: 800 } as uPlot`) with
    `sidesWithAxes[1] === false` returns a value `> DEFAULT_RIGHT_CUSHION_PX`
    and `<= Math.min(MAX_ROTATED_SIZE, Math.round(800 * ROTATED_SIZE_FRACTION))`;
    with `sidesWithAxes[1] === true` it returns that same capped value minus
    `RIGHT_AXIS_SIZE_PX`, floored at `0` — the right-axis case is reduced by
    the y-axis's own reserved width rather than stacking both, and the cap
    itself is width-relative, not just the flat `MAX_ROTATED_SIZE`. Same
    call-order contract as the `size()` test above (`rotate()` before
    `rightPadding()`).
- **Manual/visual**: build (or reuse) a chart cell with `xAxisMode:
  'categorical'` and ~10 long category labels (e.g.
  `++product+channel+branch-CL-123456`-style ~34-char strings) at a normal
  panel width (~800px) — enough categories to stay well above uPlot's
  `space: 60` per-tick floor, so `rotate()`/`size()` actually run; confirm
  labels rotate. At the app's default chart-cell height (`ChartCell.tsx`'s
  `defaultHeight: 250`, floored via `XYChart.tsx:308`'s `Math.round(Math.max(250,
  rect.height - 32))`), the height-relative cap (`min(MAX_ROTATED_SIZE,
  round(self.height * ROTATED_SIZE_FRACTION))` = `100px` at `height = 250`) is
  well under this label's ~175px uncapped rotated extent (`ceil(34 ×
  AVG_CHAR_WIDTH_PX × sin45° + LABEL_LINE_HEIGHT_PX × cos45°) +
  AXIS_CHROME_PX`), so the label's tail is expected to be truncated at the
  axis edge — that's the documented trade-off from the `size` subsection, not
  a bug. The pass criterion at this height is: the axis band stays a minority
  of the chart's height (doesn't dominate the cell), and any truncated label
  ends cleanly at the axis edge rather than mid-glyph or overflowing outside
  the axis box — not that the full 34-character string is legible.
  Also confirm that a chart with a handful of short
  categories (e.g. usernames) still renders flat, unchanged from today. Do
  not use a much higher category count (e.g. ~30) at the same width as a
  rotation test: that pushes per-tick space below the `space: 60` floor and
  uPlot blanks the axis entirely before rotation logic ever runs — a
  separate pre-existing gap (see Overview), not evidence that rotation
  "isn't needed."
  **The ~10-category case above is already the discriminating one; no
  separate count is needed.** At an ~800px panel, 10 long categories give
  ~68.5px per tick before the padding fix (`(800 − 90 y-axis − 25 cushion) /
  10`) and roughly ~53px after it (accounting for the larger rotated
  padding) — straddling the 60px floor, so it's the case that blanks the
  axis if `space` is left static. Confirm the axis renders, not just that
  labels are tilted. Do not treat 12–14 categories at ~800px as a more
  discriminating test: `(800 − 90 − 25) / 12 ≈ 57px` is already under the
  60px floor before any rotation code runs (worse still with a second
  y-axis, e.g. `(800 − 90·2) / 12 ≈ 51.7px` — no `− 25` here since a right
  axis makes `rightPadding` reproduce uPlot's own `0`-cushion `autoPadSide`
  result, not `DEFAULT_RIGHT_CUSHION_PX`), so that count blanks for
  pre-existing reasons (see Overview), not because of anything this fix
  changes.
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
- Deliberately left open (not blocking): the pre-existing high-category-count
  blanking described in the Overview is still present after this change. The
  rotation-aware `space` floor only prevents *this* fix from creating new
  instances of it; a chart whose labels never get a chance to rotate still
  blanks. Closing that properly means deciding the floor from the label set
  known at build time (`xLabels` is in the closure) rather than from
  `rotated`, which changes behavior for charts that don't rotate — its own
  issue, its own regression surface.
