# Bar Chart End-Bar Clipping & Label Centering Plan

## Overview

In the analytics web app metrics chart, when the Bar view is selected the first
and last bars are clipped in half by the plot edges, and the x-axis labels for
those end categories are pushed to the plot boundary instead of being centered
under their bars. This fix adds half-slot horizontal padding to the categorical
x-scale so every bar sits fully inside the plot area and each end label centers
on its tick.

## Current State

The chart is rendered by uPlot in
`analytics-web-app/src/components/XYChart.tsx`. Bars use uPlot's built-in
`uPlot.paths.bars!({...})` generator in two code paths:

- **Multi-series** (`XYChart.tsx:776-793`): `size: [0.8 / normalizedSeries.length]`
- **Single-series** (`XYChart.tsx:932-944`): `size: [0.8]`

In categorical mode the x values are plain integer indices `0..N-1` (usernames
mapped to `xLabels` indices — see `XYChart.tsx:93,95`). Each bar (width `0.8` of
a slot) is centered on its integer index.

The x-scale is declared with **no custom `range`**:

- Multi-series — `XYChart.tsx:697-699`:
  ```tsx
  const scales: uPlot.Scales = {
    x: { time: xAxisMode === 'time' },
  }
  ```
- Single-series — `XYChart.tsx:981-982`:
  ```tsx
  scales: {
    x: { time: xAxisMode === 'time' },
  ```

With no `range` override, uPlot defaults the horizontal scale extent to the exact
data bounds `[0, N-1]`. Consequences:

1. **End-bar clipping** — the bar centered at `x=0` has its left half outside the
   plot (before `0`), and the bar at `x=N-1` has its right half outside (after
   `N-1`), so both are drawn clipped.
2. **Off-center end labels** — the x-axis tick/label builder
   (`xychart-axis.ts:21-30`) places one tick per integer index (`incrs = [1]`).
   The ticks for `0` and `N-1` land exactly on the plot boundary, so the end
   labels sit at the plot edge instead of centered beneath their bars.

The x-axis config builder `buildXAxisConfig` already lives in a separate pure,
unit-tested module (`xychart-axis.ts`), extracted in #1089. There is no
equivalent builder for the x **scale** — both call sites inline the scale object.

## Design

Add half-slot padding to the categorical x-scale so the drawable range becomes
`[-0.5, (N-1) + 0.5]`. A slot is 1 index wide (`incrs = [1]`), and the widest bar
is `0.8` of a slot (`±0.4` around center), so `0.5` padding on each side fully
contains every bar and shifts the end ticks off the plot boundary — which also
recentres their labels (uPlot centers labels on ticks by default).

To keep both call sites DRY and consistent with the existing `buildXAxisConfig`
pattern, extract a small pure helper into `xychart-axis.ts`:

```ts
// xychart-axis.ts
export function buildXScale(xAxisMode: XAxisMode): uPlot.Scale {
  const scale: uPlot.Scale = { time: xAxisMode === 'time' }
  if (xAxisMode === 'categorical') {
    // Pad by half a slot so end bars aren't clipped and end labels stay
    // centered under their bars. A slot is 1 index wide; bars span 0.8 of it.
    scale.range = (_u, dataMin, dataMax) => [dataMin - 0.5, dataMax + 0.5]
  }
  return scale
}
```

Then replace the inline `x: { time: ... }` in both scale objects with
`x: buildXScale(xAxisMode)`.

### Scope: categorical vs. bar-only

The padding is keyed on **`xAxisMode === 'categorical'`**, not on
`chartType === 'bar'`:

- Categorical mode is exactly the discrete-index layout the metrics chart uses
  for the "vs username" view, and it is the mode where the clipping/label issue
  occurs.
- Half-slot padding is harmless (in fact slightly nicer) for a categorical
  *line* chart too — it keeps the first/last points off the plot edge — so we do
  not need to thread `chartType` into the scale builder. This keeps the helper
  signature minimal and avoids re-running scale logic when only the chart type
  toggles.
- Time and numeric modes are left unchanged (`range` stays uPlot-default),
  preserving current time-range selection and numeric auto-ranging behavior.

### Interaction with existing y-scale ranges

Only the `x` scale is touched. The per-unit and single-series `y` scale `range`
functions (`XYChart.tsx:725-732`, `983-990`) are unchanged.

## Implementation Steps

1. **Add `buildXScale` helper** to
   `analytics-web-app/src/components/xychart-axis.ts`, next to
   `buildXAxisConfig`. Import `XAxisMode` (already imported as a type) and return
   a `uPlot.Scale` with the categorical `range` padding described above.
2. **Wire multi-series path** — in `XYChart.tsx:697-699`, replace
   `x: { time: xAxisMode === 'time' }` with `x: buildXScale(xAxisMode)`. Update
   the import from `./xychart-axis` to include `buildXScale`.
3. **Wire single-series path** — in `XYChart.tsx:981-982`, replace
   `x: { time: xAxisMode === 'time' }` with `x: buildXScale(xAxisMode)`.
4. **Verify tick labels** — confirm `buildXAxisConfig`'s categorical `values`
   lookup (`xychart-axis.ts:24-30`) still resolves labels correctly now that
   ticks fall at non-boundary positions (the `Math.round(v)` index lookup is
   unaffected since ticks remain on integer indices via `incrs = [1]`).

## Files to Modify

- `analytics-web-app/src/components/xychart-axis.ts` — add `buildXScale` helper.
- `analytics-web-app/src/components/XYChart.tsx` — use `buildXScale` in both
  scale objects and extend the `./xychart-axis` import.

## Trade-offs

- **Fixed `0.5` padding vs. bar-width-derived padding.** `0.5` is half a slot and
  comfortably clears the max bar half-width (`0.4`). Deriving padding from the
  actual `size` array would couple the scale builder to bar sizing for no visible
  benefit; a constant half-slot is the conventional uPlot approach for
  categorical/ordinal bars.
- **Keying on `categorical` rather than `bar`.** Slightly widens where padding
  applies (categorical line charts also get it), but keeps the helper pure and
  independent of `chartType`, and the effect on line charts is benign/positive.
  Alternative — passing `chartType` into `buildXScale` and padding only for bars —
  was rejected as unnecessary coupling.
- **Extract helper vs. inline at both sites.** Extracting follows the existing
  `buildXAxisConfig` precedent (#1089), keeps the two call sites identical (DRY),
  and makes the padding rule unit-testable.

## Documentation

No user- or developer-facing documentation covers this chart's internals; no docs
updates required.

## Testing Strategy

- **Unit test** (`xychart-axis` test file, alongside existing `buildXAxisConfig`
  tests): assert `buildXScale('categorical').range!(u, 0, 3)` returns
  `[-0.5, 3.5]`, and that `buildXScale('time')` / `buildXScale('numeric')` have no
  `range` set (padding not applied).
- **Manual/visual** on the `claude-code-usage` screen (or any metrics screen with
  a categorical "vs username" bar view):
  - Switch to Bar view and confirm the first (`madesroches`) and last (`apestana`)
    bars render fully, not clipped at the plot edges.
  - Confirm the end-category x-axis labels are centered beneath their bars.
  - Toggle back to Line view and confirm no regression.
  - Check a single-category case (N=1) renders a centered, unclipped bar.
- Run `yarn lint` and `yarn type-check` in `analytics-web-app/`.

## Open Questions

- None. The fix is localized to the categorical x-scale; no API or data-shape
  changes are involved.
