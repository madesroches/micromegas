# Dynamic Y-Axis Width Plan (#1503)

## Overview

Fix [#1503](https://github.com/madesroches/micromegas/issues/1503): y-axis tick
labels are clipped at the canvas edge because the y-axis band is a hard-coded
90 px, no matter how wide the formatted labels actually are. `#1504`
(`tasks/completed/chart_axis_unit_suffix_plan.md`) reduced how often the bug is
*visible* by dropping the unit suffix from ticks on **single-line** charts with
an opaque unit, but it explicitly left the sizing bug open — and left multi-line
charts untouched, which is exactly where the reporter still sees it: two series
sharing an `ops_per_sec` scale keep the suffix, so ticks read `100 ops_per_sec`
(~105 px of text) inside a 90 px band and get cut mid-string.

The fix is purely a sizing fix: give the y-axis a `size` callback that measures
the labels uPlot just formatted and widens the band when they don't fit, never
shrinking below today's 90 px so no chart that renders correctly today moves.
**Tick text is unchanged everywhere** — multi-line charts keep repeating the
unit suffix, single-line charts keep #1504's suppression. This also covers long
*numeric* labels (`$1,234,568`, `1,234,567 ms`) that no unit-suffix policy could
fix.

Along the way it removes the duplicated y-axis config literal by extracting
`buildYAxisConfig` next to the existing `buildXAxisConfig`, so both the
single-line and multi-line paths get the fix from one place.

## Current State

### The two y-axis config sites — `analytics-web-app/src/components/XYChart.tsx`

Two near-identical `uPlot.Axis` literals, both with `size: RIGHT_AXIS_SIZE_PX`:

- **Multi-line**, one axis per unit-scale group, inside
  `isMultiSeries && normalizedSeries.length > 1`: `XYChart.tsx:743-757`. Sets
  `show: scaleInfo.hasVisible`, `side`, and a grid only for `side === 1`, and
  formats ticks with `formatYAxisTick(v, axisCf, yAxisUnit, currencyCode)` where
  `yAxisUnit = adaptiveInfo?.abbrev ?? unitDisplayAbbrev(scaleInfo.unitName)`
  (`XYChart.tsx:740`) — i.e. the raw unit string for an opaque unit.
- **Single-line**, in the `else` branch: `XYChart.tsx:1001-1013`. No `side` (so
  uPlot's default 3 = left), always grids, and passes
  `isKnownAxisUnit ? yAxisUnit : ''` — the #1504 suppression
  (`XYChart.tsx:918`, `isCompactAxisUnit`).

Everything else in the two literals (`stroke`, `ticks`, `font`, `size`) is
byte-identical.

### Why the text clips

`RIGHT_AXIS_SIZE_PX = 90` (`xychart-axis.ts:43`) is the *entire* band uPlot
reserves for tick text + tick marks + gap. uPlot's own default is a flat
`size: 50` (`node_modules/uplot/dist/uPlot.esm.js:1611`) — the library never
measures y-axis labels, so nothing else compensates. With `ticks.size = 10` and
`gap = 5` (`uPlot.esm.js:1606-1611`), ~75 px is left for text.

Tick text is drawn with `textAlign` derived from the side
(`uPlot.esm.js:4622-4627`): `side === 3` (left) → `RIGHT`, so text grows
*leftward* from the band's inner edge and a too-wide label runs off the canvas'
left edge — the "shows only `0 ops_per_sec`" symptom in the issue. `side === 1`
(right) → `LEFT`, so it runs off the right edge instead. Either way the canvas
is the clip boundary; there is no ellipsis, just a hard cut.

Label widths at the axis font (`11px -apple-system, …`), using the existing
6 px/char estimate: `100 ops_per_sec` ≈ 90 px, `$1,234,568` ≈ 60 px,
`1,234,567 ms` ≈ 72 px. Only the first *needs* more than the 75 px of text room,
but all three sit close enough to the edge that the fixed band is fragile.

### uPlot's sizing contract (what we can hook)

- `Axis.Size = number | ((self, values: string[], axisIdx, cycleNum) => number)`
  (`uPlot.d.ts:983`).
- Per layout cycle, `axesCalc` calls `axis.values(...)` first and
  `axis._size = ceil(axis.size(self, values, i, cycleNum))` immediately after
  (`uPlot.esm.js:4513-4520`) — same call, same cycle, values in hand. This is
  the same ordering guarantee the x-axis `rotate` → `size` pair already relies
  on (`xychart-axis.ts:93-108`).
- If any axis `_size` changed, the cycle re-runs, up to `CYCLE_LIMIT = 3`
  (`uPlot.esm.js:3390-3408`), so a size that depends on labels converges as long
  as the labels themselves are stable for a given plot size.
- At init, `axis.size(self, null, i, 0)` is called with **`values === null`**
  (`uPlot.esm.js:3782`) — the callback must handle that.
- `self.axes` is public (`uPlot.d.ts:50`) but `_size` is not typed — reading it
  needs a cast, so this plan does not.

### Consumers that depend on the y-axis width

- `ChartAxisBounds { left, width }` (`XYChart.tsx:17-20`) is reported from the
  `ready` and `setSize` hooks (`XYChart.tsx:855-870`, `1038-1053`), i.e. *after*
  layout converges, and drives `PropertyTimeline` / `ThreadCoverageTimeline`
  alignment on `PerformanceAnalysisPage`. A wider axis flows through correctly
  with no change.
- The x-axis right padding subtracts `RIGHT_AXIS_SIZE_PX` when a right-side
  y-axis exists (`xychart-axis.ts:100-116`), treating 90 as the clearance a
  rotated x label may cross into.
- **Cross-chart alignment**: two independent `XYChart`s stacked in a screen or
  notebook line up today only because both bands are exactly 90 px. Nothing
  synchronizes them, so any *shrink* would visibly de-align stacked charts.
  This is why the 90 px stays as a floor rather than becoming a starting point.

### Finding: multi-line charts put the primary y-axis on the right

`side: idx === 0 ? 1 : idx === 1 ? 3 : idx % 2 === 0 ? 1 : 3, // 1=left, 3=right`
(`XYChart.tsx:268`). uPlot's sides are `0 top, 1 right, 2 bottom, 3 left`
(`uPlot.d.ts:1004-1009`, confirmed by `calcPlotRect`: `side == 3` accumulates
`plotLftCss` / `hasLftAxis`, `uPlot.esm.js:3438-3443`). The comment is inverted,
so today the *first* unit group renders on the **right** and the second on the
left — the opposite of the intent, and the reason the grid condition is written
as `scaleInfo.side === 1` (`XYChart.tsx:747`) rather than "first axis". Out of
scope here (see Open Questions); the fix below is side-agnostic, and the
mockups reproduce today's right-side placement rather than the intent.

## Design

### 1. `buildYAxisConfig` in `xychart-axis.ts`

One builder for both call sites, mirroring `buildXAxisConfig`'s shape (pure,
`import type uPlot`, unit-testable):

```ts
export interface YAxisOptions {
  /** uPlot scale key; omit for the single-line default 'y'. */
  scale?: string
  /** uPlot side: 3 = left, 1 = right. Default 3 (uPlot's own default). */
  side?: 1 | 3
  /** Hidden when every series on this scale is hidden. Default true. */
  show?: boolean
  /** Only one axis draws the grid. Default true. */
  showGrid?: boolean
  /** Applied on top of the raw tick value; 1 when the caller pre-scaled. */
  conversionFactor: number
  /** Display unit for the tick suffix; '' suppresses it. */
  displayUnit: string
  /** Raw currency unit for a currency scale, else null. */
  currencyCode: string | null
}

export function buildYAxisConfig(opts: YAxisOptions): uPlot.Axis
```

Body: today's shared literal (`stroke: '#6a6a7a'`, `ticks`, `font`), plus

```ts
grid: showGrid ? { stroke: '#2a2a35', width: 1 } : { show: false },
values: (_u, vals) => vals.map(v => formatYAxisTick(v, conversionFactor, displayUnit, currencyCode)),
size: (self, values) => yAxisSize(self.width, values),
```

`formatYAxisTick` is unchanged, and so is every caller's `displayUnit` — the
builder is a pure de-duplication of the two literals plus the new `size`.

### 2. The sizing rule

```ts
/** Widest band the y axis may claim, and the fraction of chart width it may not exceed. */
export const Y_AXIS_MAX_SIZE_PX = 200
export const Y_AXIS_SIZE_FRACTION = 0.33

/**
 * Tick-band width in CSS px for a y axis, sized from its formatted labels.
 * Grow-only: never returns less than Y_AXIS_BASE_SIZE_PX, so a chart whose
 * labels already fit renders exactly as it does today.
 */
export function yAxisSize(chartWidth: number, values: string[] | null): number {
  // uPlot's init call passes values === null (uPlot.esm.js:3782).
  if (values == null) return Y_AXIS_BASE_SIZE_PX
  const maxWidth = Math.max(0, ...values.map(v => estimateLabelWidth(String(v ?? ''))))
  const needed = Math.ceil(maxWidth) + AXIS_CHROME_PX + TICK_LABEL_PADDING_PX
  const cap = Math.min(Y_AXIS_MAX_SIZE_PX, Math.round(chartWidth * Y_AXIS_SIZE_FRACTION))
  return Math.max(Y_AXIS_BASE_SIZE_PX, Math.min(cap, needed))
}
```

- **Reuses the x-axis primitives** rather than adding parallel ones:
  `estimateLabelWidth` (`xychart-axis.ts:50-52`), `AXIS_CHROME_PX = 20` (tick
  length + gap, matching `ticks.size 10 + gap 5` plus a small buffer), and
  `TICK_LABEL_PADDING_PX = 8`.
- **Floor = today's 90 px** (`RIGHT_AXIS_SIZE_PX`, renamed
  `Y_AXIS_BASE_SIZE_PX`): grow-only. Every chart that renders correctly today
  renders pixel-identically after the change; only clipped charts move. This is
  what keeps stacked charts aligned and keeps the blast radius at "charts that
  were already broken".
- **Cap** keeps a pathological unit (or a `max`-scale chart with huge numbers)
  from eating the plot: at most 200 px, and at most a third of the chart width.
  When the cap binds, the label still clips — same as today, no regression, and
  the floor guarantees the cap can never push the band *below* 90 px on a narrow
  chart.
- **The reported case**: `100 ops_per_sec` → `needed = 90 + 20 + 8 = 118 px`; on
  a 660 px-wide chart the cap is 200, so the band lands at 118 px and the plot
  loses ~28 px of width. Nothing is clipped.
- **Convergence**: the returned size depends only on the formatted labels and
  `self.width`. Label text depends on the scale range (fixed by our own `range`
  callbacks, not on plot width) and on tick count (a function of plot *height*).
  A wider band → narrower plot → possibly a different x-axis rotation → a
  different plot height → possibly one more/fewer y tick, whose formatted width
  is essentially the same. So the loop settles within uPlot's 3 cycles; a
  non-converged edge case degrades to a slightly-off band, not a crash.

### 3. The x-axis right-padding coupling

`xychart-axis.ts:100-116` subtracts `RIGHT_AXIS_SIZE_PX` as the clearance a
rotated x label may cross into. Because the new rule is **grow-only**, 90 px
remains a valid *lower bound* on any right-side y-axis band, so that
subtraction stays sound — it just becomes conservative (a few px of extra right
padding) in the rare grown case. Rename the constant to `Y_AXIS_BASE_SIZE_PX`
and document it there as a floor, not an exact size. No shared mutable layout
state, and no reading of uPlot internals, is needed.

```
axesCalc cycle (uPlot.esm.js:3396-3408)
  ├─ axis[0] (x): values → rotate → size            (existing)
  ├─ axis[1..] (y): values → size = yAxisSize(...)   (new)
  └─ paddingCalc: rightPadding(...) reads sidesWithAxes, subtracts the 90 px floor
     └─ any _size changed? re-run, up to 3 cycles
```

## Mockups

Self-contained; open directly in a browser.

- **`tasks/chart_y_axis_width_mockups/option-a-dynamic-axis-width.html` — the
  chosen design.** Today's clipped multi-line chart, the same chart with a band
  sized from its labels, and a single-line currency chart that needs the same
  dynamic sizing regardless of any unit policy.
- `tasks/chart_y_axis_width_mockups/option-b-axis-title-plus-dynamic-width.html`
  — rejected alternative, kept for the record: the opaque unit drawn once as a
  rotated uPlot axis title with plain numeric ticks, which would have held the
  band at exactly 90 px. Not pursued (see Trade-offs).

## Implementation Steps

1. **`xychart-axis.ts` — constants.** Rename `RIGHT_AXIS_SIZE_PX` →
   `Y_AXIS_BASE_SIZE_PX` (update its docstring: it is now the y-axis *floor*,
   used by the x-axis padding as a lower bound). Add `Y_AXIS_MAX_SIZE_PX` and
   `Y_AXIS_SIZE_FRACTION`.
2. **`xychart-axis.ts` — `yAxisSize(chartWidth, values)`**, exported for direct
   unit testing.
3. **`xychart-axis.ts` — `buildYAxisConfig(opts)`**, returning the full
   `uPlot.Axis` including `values` and `size`.
4. **`XYChart.tsx` — multi-line path** (`743-757`): replace the literal with
   `buildYAxisConfig({ scale: scaleName, side: scaleInfo.side as 1 | 3, show:
   scaleInfo.hasVisible, showGrid: scaleInfo.side === 1, conversionFactor:
   axisCf, displayUnit: yAxisUnit, currencyCode: isCurrencyScale ?
   scaleInfo.unitName : null })`. Tick text unchanged.
5. **`XYChart.tsx` — single-line path** (`1001-1013`): replace the literal with
   `buildYAxisConfig({ conversionFactor: 1, displayUnit: isKnownAxisUnit ?
   yAxisUnit : '', currencyCode: isCurrencyScale ? primaryUnit : null })`.
   Behavior-identical apart from sizing.
6. **Tests** — extend `src/components/__tests__/xychart-axis.test.ts`; update
   the existing `RIGHT_AXIS_SIZE_PX` references at `xychart-axis.test.ts:15,135`.
7. **`CHANGELOG.md`** — Fixed entry.

## Files to Modify

- `analytics-web-app/src/components/xychart-axis.ts` — constants, `yAxisSize`,
  `buildYAxisConfig`.
- `analytics-web-app/src/components/XYChart.tsx` — both y-axis literals → the
  builder.
- `analytics-web-app/src/components/__tests__/xychart-axis.test.ts` — new cases,
  renamed constant.
- `CHANGELOG.md` — Fixed entry.

## Trade-offs

- **Char-count estimate vs. `ctx.measureText`.** Measuring would be exact, but
  the only ctx available inside `size` is uPlot's own, whose font state is
  cached (`ctxFont`, `uPlot.esm.js:3958-3962`) — mutating it risks a stale-cache
  mis-render unless carefully restored — and a private offscreen canvas returns
  no 2d context under jsdom, so the estimate would still be needed as a
  fallback. `estimateLabelWidth` is already the module's established, tested
  approach for exactly this question on the x axis (`xychart-axis.ts:16-18`).
  Digits at 11 px in the axis font are ~6.1 px against the 6 px estimate, a
  ~2% underestimate that `TICK_LABEL_PADDING_PX` absorbs well past any
  realistic label length.
- **Grow-only (floor at 90 px) vs. size-to-fit in both directions.**
  Size-to-fit would give short-label charts ~40 px of plot width back, but it
  de-aligns independently rendered charts stacked in a screen (nothing
  synchronizes their bands) and changes the look of every chart in the app to
  fix a bug in a few. Grow-only keeps the diff's visible effect confined to
  charts that are currently broken. Revisitable later as a deliberate layout
  change.
- **Repeated suffix (chosen) vs. an axis title on a shared multi-line axis.**
  An axis title would have kept the band at 90 px and given plain numeric ticks,
  matching #1504's single-line rule — but it introduces a concept the codebase
  doesn't use anywhere (uPlot axis labels), puts the unit somewhere readers of
  these charts don't currently look, and leaves the sizing bug to be fixed
  anyway for long numeric labels. Keeping the suffix on the ticks means the fix
  is *only* sizing: no change to what any chart says, one mechanism to reason
  about. Cost: ~28 px of plot width on a multi-line chart with a long opaque
  unit, and the unit still repeats on every tick row.
- **Constant floor vs. plumbing the real right-axis size into the x-axis
  padding.** Sharing mutable layout state between the two builders (or casting
  to read `axis._size`) would make the padding exact; with a grow-only rule the
  constant is already a correct lower bound, so the extra coupling buys a few
  pixels in a rare case.

## Documentation

- `CHANGELOG.md`: **Fixed** — y-axis tick labels no longer clip at the canvas
  edge; the axis band now widens to fit its labels (#1503).
- `mkdocs/docs/web-app/notebooks/cell-types.md:30` needs no change: tick text is
  unchanged by this plan, so the documented unit behavior still holds.

## Testing Strategy

Unit tests in `src/components/__tests__/xychart-axis.test.ts`:

- `yAxisSize`:
  - `values === null` (uPlot's init call) → `Y_AXIS_BASE_SIZE_PX`.
  - short labels (`['0','100','200']`) → exactly `Y_AXIS_BASE_SIZE_PX` — the
    grow-only floor, and the regression guard for "no existing chart moves".
  - `['100 ops_per_sec', …]` → `> Y_AXIS_BASE_SIZE_PX` and equal to
    `estimateLabelWidth(widest) + AXIS_CHROME_PX + TICK_LABEL_PADDING_PX`.
  - widest-of-many: the result tracks the longest label, not the first or last.
  - fraction cap binds: a huge label with a modest `chartWidth` →
    `round(chartWidth * Y_AXIS_SIZE_FRACTION)`; with a *tiny* `chartWidth` →
    still the floor (the cap can never undercut it).
  - `Y_AXIS_MAX_SIZE_PX` binds for a huge label on a very wide chart.
- `buildYAxisConfig`: `scale`/`side`/`show` pass-through and their defaults;
  `showGrid: false` yields `{ show: false }`; `values` formatting matches
  `formatYAxisTick` for the plain, suppressed-suffix (`''`), and currency cases;
  `size` is a function returning the floor for `null` values.

Manual verification (services per `CLAUDE.md`, monolith mode):

1. Multi-line chart, two series on `redis_ops_per_sec | ops_per_sec` — full
   `100 ops_per_sec` … `400 ops_per_sec` labels visible, nothing clipped, band
   visibly wider than on `main`.
2. Single-line `ops_per_sec` chart — unchanged from #1504 (plain ticks, unit in
   header/stats/tooltip).
3. Single-line currency chart with values in the millions, `Max` scale — band
   grows, `$1,234,568` fully visible.
4. Known-unit charts (ms / bytes / percent), single and multi-line — pixel
   identical to `main` (the floor case).
5. Categorical x-axis chart with rotated labels **and** a right-side y-axis —
   right padding still clears the rotated labels (the `Y_AXIS_BASE_SIZE_PX`
   consumer at `xychart-axis.ts:115`).
6. Narrow chart (drag a screen cell to ~300 px) — band stays near 90 px, no
   runaway axis, plot still usable.
7. `PerformanceAnalysisPage` — `PropertyTimeline` / `ThreadCoverageTimeline`
   stay aligned with the chart's plot area after the band grows.

`yarn lint`, `yarn type-check`, and targeted `vitest run xychart-axis units` in
`analytics-web-app/`. Note the full `yarn test` suite has pre-existing,
unrelated `apache-arrow` type failures on `main` (see #1504's test plan).

## Open Questions

1. **The inverted `side` comment** (`XYChart.tsx:268`): today the first unit
   group's axis lands on the right and the second on the left, the opposite of
   the comment's intent. Fixing it is a one-line change but a visible layout
   shift for every multi-line chart, and it would also mean rewriting the grid
   condition (`scaleInfo.side === 1` → "first axis"). Worth a separate issue —
   confirm the intent first.
2. **Should size-to-fit eventually replace grow-only?** If charts in a screen
   are meant to share a left edge, the real fix is a band synchronized across a
   screen's charts rather than a per-chart constant. Out of scope, but it's the
   direction that would let the axis shrink safely.
