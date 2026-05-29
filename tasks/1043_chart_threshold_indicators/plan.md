# Chart Threshold Indicators Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1043

## Overview

Add visual threshold support to the analytics web app's XY chart so regressions
and out-of-bounds values are obvious at a glance. Three capabilities:

1. **Horizontal reference line(s)** at fixed values (e.g. a red line marking a
   budget), configured per-chart in the cell editor.
2. **Per-row value coloring** of bars/points driven by an optional `color`
   column in the SQL result — using the same packed-RGBA `u32` convention the
   map cell already consumes. Combined with the existing color UDFs (`rgba`,
   `lerp_color`, `color_scale`), a query can emit `CASE WHEN value > $budget
   THEN rgba(0.8,0.1,0.1,1) ELSE rgba(0.2,0.7,0.2,1) END AS color`, giving the
   "green below / red above" behavior from the issue with no extra UI.
3. **User-selectable series color.** Today the per-query palette color does
   double duty: it identifies the series in the legend *and* colors the marks.
   Once marks can be colored from SQL, that coupling becomes confusing. The fix
   is to let the user pick each query's color explicitly (defaulting to today's
   rotating palette). That chosen color is the legend token and the default
   mark color; when a query supplies a `color` column the marks use the per-row
   colors and the user is free to set a neutral legend token (e.g. gray).

The issue originally framed item 2 as a UI toggle ("green below threshold, red
above"). The user has since asked that the color come from the SQL query, like
the map. SQL-driven coloring subsumes the UI toggle (any conditional rule is
expressible in SQL) and reuses an established convention, so it is the chosen
mechanism. Multi-series charts are fully supported — each query may carry its
own `color` column. See [Trade-offs](#trade-offs).

## Current State

### Chart rendering — `analytics-web-app/src/components/XYChart.tsx`

- `XYChart` wraps uPlot (`^1.6.32`). It runs a single-series path
  (`XYChart.tsx:646-758`) and a multi-series path (`XYChart.tsx:471-644`).
- Series colors come from a fixed per-series color: single-series uses a
  hard-coded `#bf360c` (`XYChart.tsx:702-704`); multi-series rotates through
  `SERIES_COLORS` (`chart-constants.ts`, applied at `XYChart.tsx:568,576`).
- Bars are drawn with the uPlot bars path builder:
  `uPlot.paths.bars!({ size: [0.8], gap: 1 })` (single) and per-series
  `align` for grouped bars (multi) (`XYChart.tsx:577-578,705`).
- **Y-scale unit subtlety that affects reference lines:**
  - *Single-series* pre-multiplies y values by an adaptive `conversionFactor`
    (`XYChart.tsx:650,655-657`), so the `y` scale and axis operate in *display*
    units (e.g. ms → µs).
  - *Multi-series* pushes **raw** `d.y` into uPlot (`XYChart.tsx:495`); the
    adaptive factor is applied only inside the axis `values` formatter
    (`XYChart.tsx:552-558`). The scale itself is in raw units.
  - A reference line value supplied in raw data units must therefore be scaled
    by the same `conversionFactor` in the single-series path, and used directly
    in the multi-series path.
- uPlot scale names: single-series uses `'y'`; multi-series uses `unit || 'y'`
  (`XYChart.tsx:569,529`).
- Plugins are already used for tooltips and are attached via `opts.plugins`
  (`XYChart.tsx:591,665`). `hooks` (`ready`, `setSize`, `setSelect`) are set on
  the options object — a `draw` hook is the natural place to paint reference
  lines.

### Chart data extraction — `analytics-web-app/src/lib/arrow-utils.ts`

- `validateChartColumns` (`arrow-utils.ts:181-213`) **requires exactly 2
  columns** (X, Y). This is the gate that must relax to admit an optional color
  column.
- `extractChartData` (`arrow-utils.ts:405-498`, single-series) and
  `extractMultiSeriesChartData` (`arrow-utils.ts:237-400`, multi-series)
  build `{ x, y }` point arrays. Points are **sorted** by x in time/numeric
  mode (`arrow-utils.ts:494,339`), so any per-row color must travel *with* the
  point, not in a parallel array.
- `ChartSeriesData` (`arrow-utils.ts:219-223`) is the series shape consumed by
  `XYChart`; its `data` is `{ x: number; y: number }[]`.
- `isNumericType` / dictionary unwrap helpers already exist for type checks.

### Cell config & editor — `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx`

- Chart options live in `ChartCellConfigV2.options` (`ChartCell.tsx:44-53`),
  currently `scale_mode` and `chart_type`. Options are persisted in the screen
  JSON and round-tripped through `migrateChartConfig`.
- String option values are macro-substituted by `substituteOptionsWithMacros`
  (`ChartCell.tsx:85-107`); non-string values pass through untouched — so an
  array like `reference_lines` is **not** auto-substituted and needs explicit
  handling (the renderer already does this for per-series `label`/`unit`,
  `ChartCell.tsx:188-192`).
- `ChartCell` passes options to `XYChart` (`ChartCell.tsx:194-256`).
- `ChartCellEditor` (`ChartCell.tsx:264-415`) renders per-query blocks plus
  chart-level controls; it is where a "Reference lines" section is added.

### Packed-RGBA convention (reuse target) — `analytics-web-app/src/components/map/overlay.ts`

- The map encodes color as a packed `u32` in `0xRRGGBBAA` byte order.
  `rgbaFromHex` (`overlay.ts:104-122`) and `hexFromRgba` (`overlay.ts:125-129`)
  convert between `#rrggbbaa` strings and the packed `u32`. `hexFromRgba`
  already emits an 8-digit hex string, which is a valid CSS color.
- Color UDFs that produce these `u32` values already ship:
  `rgba(r,g,b,a)`, `lerp_color(c1,c2,t)`, `color_scale(name,t,a)`
  (see `tasks/completed/1062_color_udfs_plan.md`,
  `tasks/completed/1069_color_scale_udf_plan.md` and
  `mkdocs/docs/query-guide/functions-reference.md`).

### Docs

- `mkdocs/docs/web-app/notebooks/cell-types.md` documents the chart cell's
  columns and `options.*` keys (lines 9-53). Needs the new `color` column and
  `options.reference_lines`.

## Design

### 1. Optional `color` column (SQL-driven per-row coloring)

**Detection rule (explicit, order-independent):** a result column named
`color` (case-insensitive) is the color channel. The X and Y axes are the
first two *non-color* columns, in order. When no `color` column is present,
the current "exactly 2 columns" rule still applies (back-compat). The color
column must be an integer type (packed `u32`, matching the map and the color
UDFs); a leading-`#` hex string is also accepted as a convenience.

**Decode helper (shared).** Add `packedRgbaToCss(u32): string` returning an
`#rrggbbaa` string. To keep the chart core decoupled from the map module,
move `rgbaFromHex`/`hexFromRgba` into a new `analytics-web-app/src/lib/color-utils.ts`
and re-export them from `overlay.ts` for back-compat. `packedRgbaToCss` is
then `hexFromRgba` (renamed/aliased) living in the shared module. This is a
small, mechanical relocation — no behavior change — and gives both the map and
the chart one source of truth.

**Type change.** Extend the chart point shape with an optional color:

```ts
// arrow-utils.ts
export interface ChartPoint {
  x: number
  y: number
  color?: string   // CSS color decoded from the SQL `color` column, if present
}

export interface ChartSeriesData {
  label: string
  unit: string
  color?: string   // user-chosen series color; default = rotating palette by index
  data: ChartPoint[]
}
```

(`{ x: number; y: number }` literals throughout become `ChartPoint`; the field
is additive so existing call sites compile unchanged.)

**Extraction.** In `validateChartColumns`, return the resolved
`{ xColumnName, yColumnName, colorColumnName? }` (keeping the existing
`xType`/`yType`, which callers still use for `detectXAxisMode`/`timestampToMs`)
instead of assuming columns 0/1. `extractChartData` and
`extractMultiSeriesChartData` read the color column per row, decode via
`packedRgbaToCss` (or pass through a `#…` string), and set `point.color`. Note
`extractMultiSeriesChartData`'s zero-row branch (`arrow-utils.ts:255-268`) does
its own column check and must be relaxed alongside `validateChartColumns`. Null/invalid color → leave `point.color` undefined (falls back
to series color). Color travels with the point through the existing sort.

### 2. Reference lines (UI-configured)

**Config shape** (stored in `options.reference_lines`):

```ts
export interface ReferenceLine {
  name?: string            // user-entered name shown before the value (macros ok)
  value: number | string   // threshold; number, or a macro string like '$budget'
  unit?: string            // unit of the value — selects the scale AND drives
                           //   the computed value text; default = primary unit
  color?: string           // CSS color; default crimson '#c62828'
  style?: 'solid' | 'dashed'  // default 'dashed'
}
```

`ChartCellConfigV2.options` gains `reference_lines?: ReferenceLine[]`. The user
supplies a descriptive `name` (e.g. "frame budget"); the **value portion of the
label is computed** (see below) so it always matches the axis. `unit` doubles as
the scale selector, so there is no separate scale field.

**Macro resolution.** `name`, `value`, and `unit` support macros (`$variable`,
`$cell.column`, time-range, etc.). `ChartCell` resolves them before passing to
`XYChart`, mirroring `ChartCell.tsx:188-192`: `name` → `substituteMacros(...)`;
`value` → if a string, `substituteMacros(...)` then `Number(...)`; if a number,
used directly. Lines whose value resolves to NaN are dropped.

**Macro validation (editor).** The editor already validates macros in each
query's `sql`/`unit`/`label` (`ChartCell.tsx:293-309`). Extend
`validationErrors` to also run `validateMacros` over every reference line's
`name`, (string) `value`, and `unit`, surfacing errors as
`Reference line N: <error>`.

**Computed label.** The label is `name` followed by the value, where the value
text is *not* configurable — it is formatted from `value` + `unit` exactly like
the y-axis tick labels, so it always matches the axis. The axis tick formatting
(adaptive conversion factor + abbreviation + number formatting) currently lives
inline in the axis `values` closures (single: `XYChart.tsx:686-694`; multi:
`XYChart.tsx:550-560`). Extract it into a shared
`formatAxisValue(value, scaleUnitInfo)` helper and call it from both the axes
and the reference-line label (DRY). The drawn label is
`name ? \`${name}  ${formatAxisValue(...)}\` : formatAxisValue(...)` — e.g.
`name: "frame budget", value: 16000, unit: 'us'` on a scale whose adaptive unit
is `ms` renders `frame budget  16 ms` (the value text being the same string the
axis shows at that height).

**Rendering.** `XYChart` gains a prop `referenceLines?: ReferenceLine[]` and a
uPlot plugin with a `draw` hook (runs after series):

```
for each line:
  scaleName = (line.unit ?? primaryUnit) || 'y'        // unit selects the scale
  v = singleSeriesPath ? line.value * conversionFactor  // match plotted space
                       : line.value                     // multi-series: raw
  yPx = u.valToPos(v, scaleName, true)                  // canvas px
  if yPx within u.bbox:
     draw horizontal line across u.bbox at yPx (dashed via ctx.setLineDash)
     draw label (name + formatAxisValue) at the right edge, just above the line
```

If `line.unit` matches no scale (multi-series), attach to the primary scale and
still format the label with the given unit. When `unit` is unset it defaults to
the primary series unit.

The plugin reads the line list from a ref so the chart need not be recreated
when only thresholds change. Reference lines do not participate in the y-scale
range (they are overlays); a threshold above `scaleMode` range simply clips at
the top edge. (Optional follow-up: expand the scale to include visible
threshold values — noted as out of scope.)

### 3. Applying per-row colors in uPlot

**Bars.** uPlot's bars path builder supports per-datapoint fill/stroke via
`disp` (confirmed in `node_modules/uplot/dist/uPlot.d.ts:725-744`):

```ts
uPlot.paths.bars!({
  size: [...], gap: 1, align,
  disp: {
    fill: {
      unit: 3 /* Color */, kind: 2 /* Discrete */,
      values: (u, sidx, i0, i1) => colorArrayForSeries[sidx] // CSS strings
    },
    stroke: { unit: 3, kind: 2, values: (u, sidx) => strokeArrayForSeries[sidx] },
  },
})
```

`colorArrayForSeries[sidx]` is built from the series points: `point.color ??
seriesColor`, indexed by data position. When no point in a series carries a
color, skip `disp` entirely and keep the existing flat `fill`/`stroke` (avoids
the per-bar code path when unused).

**Line charts (gradient stroke).** uPlot's `Series.stroke` accepts a function
returning a `CanvasGradient` (confirmed in `node_modules/uplot/dist/uPlot.d.ts:792,932`
— `strokeStyle` is `string | CanvasGradient | CanvasPattern`). Rather than
coloring individual segments, the whole line is stroked with one horizontal
gradient whose color stops sit at each point's x position; the browser
interpolates colors between consecutive points:

```ts
stroke: (u, sidx) => {
  const pts = colorStopsForSeries[sidx]            // [{ x, color }], color = point.color ?? seriesColor
  if (!pts) return seriesColor                      // no per-row colors → flat
  const { left, width } = u.bbox
  const g = u.ctx.createLinearGradient(left, 0, left + width, 0)
  for (const { x, color } of normalizeStops(u, pts, width, left)) {
    g.addColorStop(x, color)                         // x already clamped to [0,1] + sorted
  }
  return g
}
```

`normalizeStops` maps each point's x to `clamp((valToPos(xScaleVal,'x',true) - left) / width, 0, 1)`,
where `xScaleVal = xAxisMode === 'time' ? point.x / 1000 : point.x` — the same
millisecond→second transform applied to the x values actually fed to uPlot
(`XYChart.tsx:478,493,652-653`). It then **sorts ascending and dedupes** offsets
(`addColorStop` throws on out-of-range offsets and renders oddly on
unsorted/duplicate ones). The same gradient is applied to `fill` so the area
under the line is tinted consistently.

`ChartPoint.x` carries the raw millisecond value from `timestampToMs`
(`arrow-utils.ts:477`), but in time mode uPlot's x scale is calibrated in
seconds; passing raw `point.x` to `valToPos` would misplace every stop by 1000x.
The y conversion-factor subtlety is separate (only y is pre-scaled), so the x
transform above is the only x-side handling needed.

**Large-N guard.** Stop count equals point count; thousands of stops per redraw
is slow and some browsers cap stops. When a series exceeds a threshold (e.g.
`> 512` points), downsample stops to evenly-spaced buckets (carry the color of
each bucket's representative point) before building the gradient. Below the
threshold, one stop per point.

As with bars, when no point in a series carries a color, return the flat
`seriesColor` and skip gradient construction entirely.

In every snippet above, `seriesColor` is the **user-chosen series color**
(next subsection), not a hard-coded palette lookup.

### 4. Series color & legend identity (user-selectable)

Decouple the auto-assigned palette color into an explicit per-query choice so
it no longer silently doubles as both legend identity and mark color.

- **Config.** `ChartQueryDef` gains `color?: string` (a `#rrggbb` hex).
  `addQuery` **seeds it with the rotating palette color for the new index**
  (`SERIES_COLORS[i % SERIES_COLORS.length]`), so a query always has a concrete
  color the moment it exists — the palette is the *initial suggestion*, not a
  remembered "default" the user can revert to. Existing saved configs that
  predate the field have no `color`; the renderer falls back to palette-by-index
  for those (so they render identically), and the editor writes the concrete
  color on first edit. `getRendererProps` threads each query's resolved color
  into `ChartSeriesData.color` (and the single-series path), replacing the
  index-based lookups in `XYChart` (`XYChart.tsx:568,576,702-704,819`).
- **Legend token.** The legend swatch uses `series.color` (the chosen color).
  This is the series' stable identity and is **independent of per-row mark
  colors**. The renderer makes no attempt to summarize data-driven colors into
  the swatch (a `color` column can be an arbitrary, non-ordered function, so
  any sampled gradient/representative chip would be misleading). Instead the
  user owns the token: when a query is colored from SQL, they set its series
  color to a neutral value (e.g. gray) to signal "colored by value".
- **Default marks.** When a series has no `color` column, both marks and legend
  use `series.color` — exactly today's behavior, just user-overridable.
- **Single-series.** The single-series render path consumes the `data` prop and
  wraps it into `normalizedSeries[0]` internally (`XYChart.tsx:146-150`), which
  carries no color — so the chosen color cannot ride in on `ChartSeriesData`
  there. Add a `color?: string` prop to `XYChart` for this path: `ChartCell`
  passes the resolved single-query color (default `--chart-line` / rust), and
  the path uses it for the line/bars (replacing the hard-coded `#bf360c` at
  `XYChart.tsx:702-704`) and the header line indicator (`XYChart.tsx:851`).
  (Alternatively, `ChartCell` could pass a one-element `series=[{…, color}]`
  instead of `data`; the explicit prop is preferred to avoid disturbing the
  `data`-based single-series code path.)

### Color precedence (per series)

1. SQL `color` column present → marks use per-row colors (bar fills via `disp`;
   line stroke + fill via a per-point `CanvasGradient`). Legend token = the
   user-chosen `series.color`.
2. Otherwise → marks and legend both use the user-chosen `series.color`
   (default = rotating palette).

Reference lines are independent of coloring and always draw when configured.

### Editor UI (`ChartCellEditor`)

Add a chart-level **Reference Lines** section below the query blocks (these are
chart-wide, not per-query). Each row, left to right: a `name` text input, the
numeric/`$var` value input, a `unit` input, a color swatch
(`<input type="color">` like MapCell, defaulting crimson), and a solid/dashed
toggle. The value text in the drawn label is computed (not entered) and the unit
picks the scale, so there is no separate label-value field and no scale selector.
"+ Add
reference line" / remove-row controls mirror the existing "+ Add Query" affordance.

**Per-query color picker.** The static palette dot in each query block header
(`ChartCell.tsx:318-322`) becomes an editable color swatch
(`<input type="color">`, the MapCell pattern) bound to `query.color`. It shows
the query's current color (seeded from the palette at creation) and picking a
color stores `query.color`. There is **no "reset" affordance** — the color is
just an editable property, and the palette only supplies the starting value for
a freshly added query, so there is no separate baseline to revert to. This
control is the single place the user sets both the legend token and the default
mark color.

Add a short hint under the SQL editor noting that an optional `color` column
(packed RGBA `u32`, e.g. via `rgba()`/`color_scale()`) colors each point — bar
fills, or an interpolated line gradient — and that when used, the query color
above acts only as the legend token (set it to a neutral color). Link to the
functions reference.

## Implementation Steps

### Phase 1 — Shared color helper
1. Create `analytics-web-app/src/lib/color-utils.ts`; move `rgbaFromHex` and
   `hexFromRgba` from `components/map/overlay.ts` into it, add
   `packedRgbaToCss` (alias of `hexFromRgba`). Re-export from `overlay.ts`.
2. Update existing map imports if any reference the originals directly.

### Phase 2 — Color column extraction
3. `arrow-utils.ts`: introduce `ChartPoint`; relax `validateChartColumns` to
   detect an optional `color` column and return resolved column names
   *alongside* the existing `xType`/`yType` (callers still need the types for
   `detectXAxisMode`/`timestampToMs`). Also relax the zero-row branch in
   `extractMultiSeriesChartData` (`arrow-utils.ts:255-268`), which has its own
   hard-coded `fields.length !== 2` check and never calls `validateChartColumns`
   — otherwise a 0-row query that selects a `color` column (3 columns) errors.
4. Decode `point.color` in `extractChartData` and `extractMultiSeriesChartData`
   (all extraction branches: categorical + time/numeric, single + multi).

### Phase 3 — User-selectable series color
5. `ChartCell.tsx`: add `color?: string` to `ChartQueryDef`; carry it through
   `_queryMeta` (`getRendererProps`). Multi-series: merge it into
   `ChartSeriesData.color` in the `resolvedSeries` map (`ChartCell.tsx:188-192`),
   falling back to `SERIES_COLORS[i % len]`. Single-series: pass the resolved
   color via the new `XYChart` `color?` prop, falling back to `SERIES_COLORS[0]`.
6. `XYChart.tsx`: add the `color?` prop (single-series). Replace index-based
   palette lookups (`XYChart.tsx:568,576,819`) with `series.color ?? palette[i]`
   and the single-series hard-coded `#bf360c` (`XYChart.tsx:702-704`) with the
   `color` prop; the legend swatch and default marks both read it.

### Phase 4 — Reference line + per-row mark color rendering
7. `XYChart.tsx`: add `referenceLines` prop + `ReferenceLine` type (or import
   from a shared types module); add a `createReferenceLinePlugin` using a
   `draw` hook with scale-name + conversion-factor resolution as above.
8. Build per-series color arrays from points. For bars, wire
   `disp.fill`/`disp.stroke` in both single- and multi-series paths; skip
   `disp` when no colors present. For lines, supply a `stroke`/`fill` function
   returning a `CanvasGradient` built from clamped+sorted+deduped per-point
   stops (with the large-N downsampling guard); return the flat `seriesColor`
   (the user-chosen series color) when the series carries no per-row colors.

### Phase 5 — Config, editor, plumbing
9. `ChartCell.tsx`: add `reference_lines` to `ChartCellConfigV2.options`;
   resolve macros in each reference line's `name`, `value`, and `unit` (mirroring
   `ChartCell.tsx:188-192`, since the array is skipped by
   `substituteOptionsWithMacros`); pass `referenceLines` to `XYChart` in both
   single- and multi-series render paths.
10. `ChartCellEditor`: add the Reference Lines section, the per-query color
    picker (replacing the static palette dot), and the SQL color-column hint.

### Phase 6 — Docs & tests
11. Update `mkdocs/docs/web-app/notebooks/cell-types.md` (chart section).
12. Tests (below).

## Files to Modify

- `analytics-web-app/src/lib/color-utils.ts` (new)
- `analytics-web-app/src/components/map/overlay.ts` (move helpers, re-export)
- `analytics-web-app/src/lib/arrow-utils.ts`
- `analytics-web-app/src/components/XYChart.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- Tests: `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts`,
  new `color-utils.test.ts`, and `XYChart`/`ChartCell` tests as applicable.

## Trade-offs

- **SQL-driven coloring vs. a UI green/red toggle.** The issue suggested a UI
  toggle. SQL-driven coloring via the `color` column is more flexible (any
  rule, gradients via `color_scale`, multi-band), reuses the established map
  convention + existing color UDFs (DRY), and needs no new option schema for
  coloring. Cost: users write a `CASE`/UDF expression instead of clicking a
  toggle. A UI helper that *generates* the CASE expression from a threshold is
  a clean future addition that builds on this mechanism rather than competing
  with it.
- **Color by name (`color`) vs. positional 3rd column.** Name-based detection
  is order-independent and self-documenting, and avoids ambiguity when a query
  legitimately returns a non-color third column. Cost: a column must be aliased
  `color`.
- **`disp.fill` vs. a custom `draw` hook for per-bar color.** `disp` is the
  documented, layout-cached uPlot mechanism and keeps hit-testing/tooltips
  intact; a hand-rolled draw hook would duplicate bar geometry. If a `disp`
  edge case appears, a draw-hook fallback exists but is not the default.
- **Reference lines excluded from y-scale range.** Keeps scaling behavior
  (P99/Max) unchanged and predictable; a threshold far outside the data won't
  compress the series. Including them is a small future toggle.
- **Line gradient stroke vs. per-segment solid colors.** A single
  `CanvasGradient` with one stop per point gives smooth interpolation between
  consecutive points and is less code than a custom path builder that strokes
  each segment with its own solid color (which would also lose interpolation).
  Cost: color is interpolated in sRGB along the x pixel axis, and the gradient
  is rebuilt each draw — mitigated by the large-N downsampling guard.
- **User-chosen legend token vs. auto-derived swatch.** A data-colored series
  has no single representative color, and a `color` column can be an arbitrary
  non-ordered function, so any auto-sampled gradient/representative chip in the
  legend would frequently misrepresent the data. Letting the user pick the
  series color (and set it neutral when SQL drives the marks) keeps the legend
  honest, requires no sampling heuristics, and is a strictly more capable
  version of the existing palette behavior (the default is unchanged). Cost: an
  extra control per query and a `color?` config field.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md`: document the optional `color`
  column (packed RGBA `u32`, link to color UDFs) and `options.reference_lines`
  with a short example query using `rgba()` + a reference line.
- `mkdocs/docs/query-guide/functions-reference.md`: already documents the color
  UDFs; optionally cross-link from the chart section.

## Testing Strategy

- **Unit (`arrow-utils`)**: 3-column extraction sets `point.color`; color
  survives x-sort; integer `u32` and `#rrggbbaa` decode correctly; missing/null
  color → undefined; 2-column path unchanged; non-color extra column still
  errors.
- **Unit (`color-utils`)**: round-trip `rgbaFromHex`/`packedRgbaToCss`; alpha
  preserved; map re-exports still resolve.
- **Component (`XYChart`)**: reference-line plugin computes the right `valToPos`
  in single- (with conversion factor) and multi-series (raw) paths; bars apply
  per-row `disp` colors when present and fall back otherwise; line gradient
  stops are clamped to `[0,1]`, sorted, and downsampled past the large-N
  threshold (unit-test `normalizeStops` directly — it's pure and the highest-risk
  piece). Mock uPlot as in existing chart tests.
- **Manual**: a benchmark-style bar query with `CASE … rgba() AS color` plus a
  budget reference line — verify green/red bars and a dashed crimson line at
  the budget; switch to line mode and confirm the stroke interpolates between
  point colors. Exercise both P99/Max modes, single and multi-series.
- Run `yarn lint`, `yarn type-check`, `yarn test` in `analytics-web-app/`.

## Open Questions

None outstanding — scope confirmed: variable-driven threshold values; reserved
`color` column; per-row coloring for both bars (`disp`) and lines (gradient
stroke); multi-series supported with each query carrying its own `color`
column; per-query user-selectable series color drives the legend token (set
neutral when SQL dictates mark colors).
