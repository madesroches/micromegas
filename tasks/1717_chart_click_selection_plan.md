# Chart Cell Click Selection Plan

Issue: #1717

## Overview

Add an opt-in `selectionMode: 'single'` to the notebook chart cell. When it is on, clicking a bar
(bar charts) or a point (line charts) publishes the source row of that mark as
`$cellname.selected.<column>`, the same way table and map cells already do. Downstream cells then
re-run against the selection. This supports the common "aggregate per category, click to drill
into the detail table" layout without a duplicate selectable table next to the chart.

## Current State

**Selection plumbing is already generic.** None of it is table-specific:

- `CellRendererProps.selectionMode` / `onSelectionChange` (`analytics-web-app/src/lib/screen-renderers/cell-registry.ts:59-62`).
- `NotebookRenderer` reads `cell.options?.selectionMode ?? meta.defaultSelectionMode` for any
  cell type and wires `onSelectionChange` to `updateCellSelection` (`NotebookRenderer.tsx:649-682`).
  `HorizontalGroupCell` does the same for children (`HorizontalGroupCell.tsx:182-212`).
- `useCellExecution.updateCellSelection` stores the row in `cellSelectionsRef` and re-runs every
  cell below (`useCellExecution.ts:461-479`). A downstream cell that references an unset selection
  is marked `blocked` and execution halts (`useCellExecution.ts:170-194`).
- Macro type resolution for `$cell.selected.col` reads the column's Arrow type from
  `cellResults[cell]` (`macro-resolve.ts:86-92`), which holds the cell's `state.data[0]`
  (`useCellExecution.ts:122-131`).

**Publishers today:**

- `TableCell` (`cells/TableCell.tsx:32-77`): keeps `selectedRowIndex` as ephemeral state,
  publishes the row as a plain object, and clears both on data change. It uses `useLatestRef` on
  `onSelectionChange` because the callback is an inline arrow in `NotebookRenderer`.
- `MapCell` (`cells/MapCell.tsx:284-324`): same pattern; it publishes with `rowValues(table, rowIndex)`
  from `lib/arrow-utils.ts:874`, and opts in through `defaultSelectionMode: 'single'`.
- The table editor exposes the mode with `components/RowSelectionEditor.tsx`.

**Chart cell** (`cells/ChartCell.tsx`):

- Config v2 holds `queries[]` and `options` (`scale_mode`, `chart_type`, `reference_lines`,
  `[key: string]: unknown`). `getRendererProps` passes `options` through, adding `_queryMeta`.
- `execute` returns one Arrow table per query in `data[]`, and the renderer uses `data[i]` for
  series `i`. One table means the single-series `XYChart` path; more than one means multi-series.
- The only interaction is drag-to-zoom (`onTimeRangeSelect`), and only in time mode.

**XYChart** (`components/XYChart.tsx`, uPlot-based, also used by `MetricsRenderer`):

- It rebuilds the uPlot instance on most prop changes (effect at `XYChart.tsx:603-1078`).
- Single-series: the uPlot data array is `singleData` in order, so uPlot index = point index.
- Multi-series: it builds a union X array and maps each series onto it with `null` gaps
  (`XYChart.tsx:626-656`). Bars use `uPlot.paths.bars` with a per-series `align` offset.
- Plugins in use: a tooltip plugin and `refLinePlugin` (canvas overlay in a draw hook).

**Chart data extraction** (`lib/arrow-utils.ts:386-690`): `extractChartData` and
`extractMultiSeriesChartData` turn Arrow rows into `ChartPoint { x, y, color? }`. They drop
rows with nulls or non-finite values and sort time/numeric data by X, so a point's position no
longer matches its source row. Chart queries return exactly X + Y (+ optional `color`)
columns (`validateChartColumns`, `arrow-utils.ts:304-358`). The published row is therefore
`{ <x_col>, <y_col>, color? }`.

## Design

### 1. Keep the source row on every point

Add `rowIndex: number` to `ChartPoint` (`lib/arrow-utils.ts:208`). Both extract functions set it
to the Arrow row `r`/`i` the point was built from. That covers the categorical second pass at
`arrow-utils.ts:558` too. The field is required, so every construction site has to set it. Besides
`arrow-utils.ts`, `TimeSeriesChart` in `XYChart.tsx` (~line 1265) builds `ChartPoint`s from its
`{ time, value }` data, so it maps `(d, i) => ({ x: d.time, y: d.value, rowIndex: i })`. Sorting and null-skipping then no longer lose the mapping.

### 2. Hit-testing helpers (pure, new module)

New file `components/xychart-selection.ts`, holding pure functions with no uPlot or DOM
dependency:

```ts
export interface ChartPointHit {
  seriesIndex: number // 0-based index into XYChart's normalized series (== query index)
  rowIndex: number    // source Arrow row
}

export interface BarRect { seriesIndex: number; dataIdx: number; left: number; top: number; width: number; height: number }

/** Topmost rect containing (x, y), in canvas pixels. */
export function hitTestBars(rects: BarRect[], x: number, y: number): BarRect | null

/** Nearest candidate within `radius` CSS px, or null. */
export function findNearestPoint<T extends { px: number; py: number }>(candidates: T[], x: number, y: number, radius: number): T | null

/** A mouseup this close to its mousedown counts as a click, not a drag-to-zoom. */
export function isClick(down: { x: number; y: number }, up: { x: number; y: number }, threshold?: number): boolean

/** Clicking the selected mark again clears the selection. */
export function nextSelection(current: ChartPointHit | null, hit: ChartPointHit | null): ChartPointHit | null

export function sameHit(a: ChartPointHit | null, b: ChartPointHit | null): boolean

/** Multiply the alpha of a CSS color (`#rrggbb`, `#rrggbbaa`, or `rgba(...)`) by `factor`. */
export function fadeCss(css: string, factor: number): string

/** Per-bar colors for one series: the base colors, faded for every bar except the selected one. */
export function barColors(base: string[], selectedDataIdx: number | null, seriesIndex: number, selectedSeriesIndex: number | null): string[]

/** `fadeCss` applied to each gradient stop's color. */
export function fadeStops(stops: { x: number; color: string }[], factor: number): { x: number; color: string }[]
```

`fadeCss` must accept every format a mark color arrives in: the series color (`#rrggbb`),
`hexToRgba` output (`rgba(...)`), and `cellColorToCss` output for SQL per-row colors
(`#rrggbbaa`).

### 3. XYChart: optional selection support

Add two optional props:

```ts
/** When set, clicking a bar/point reports it; clicking empty space reports null. */
onPointSelect?: (hit: ChartPointHit | null) => void
selectedPoint?: ChartPointHit | null
```

Implementation inside XYChart. Both the single- and multi-series paths get a shared
`createSelectionPlugin()`, built like `createReferenceLinePlugin`:

- **Index → row lookup.** At build time, keep `rowIndexBySeries: (number | null)[][]` aligned with
  the uPlot data arrays. In single-series it is `singleData.map(p => p.rowIndex)`. In
  multi-series, fill it in the same loop that builds `yArr`, from the same `xIndex` lookup, so
  "last point at a duplicate X wins" applies to both. Store it in a ref the plugin reads.
- **Bar rects.** Both `uPlot.paths.bars({...})` calls in each path get
  `each: (u, sIdx, dIdx, l, t, w, h) => rects.push(...)`. Each bar series' `paths` function is
  wrapped, `(u, si, i0, i1) => { clear the rects for si; return bars(u, si, i0, i1) }`, so a
  series' rects are reset exactly when its paths are rebuilt. (A `drawClear` hook would empty them
  on every `redraw(false)`, which reuses cached paths and never refills them.) uPlot reports these rects in canvas pixels and `sIdx` is 1-based, so subtract
  1 to get `seriesIndex`.
- **Click.** On `init`, attach `mousedown`/`mouseup` listeners to `u.over`. On a mouseup where
  `isClick(down, up)` holds (so a drag-to-zoom in time mode is never also a click), hit-test as
  follows:
  - bar charts: convert the cursor to canvas px (`u.bbox.left + u.cursor.left * devicePixelRatio`,
    same for top) and run `hitTestBars`.
  - line charts: with `r = 8`, scan each visible series over the indices
    `u.valToIdx(u.posToVal(left - r, 'x'))`..`u.valToIdx(u.posToVal(left + r, 'x'))`, so every
    point within the radius is a candidate even with `null` gaps or dense data. For each non-null
    y, build candidates `{ px: u.valToPos(x[idx], 'x'), py: u.valToPos(y, series.scale) }` and run
    `findNearestPoint(..., radius = r)`.
  - Map the hit to `ChartPointHit` with `rowIndexBySeries`, then call
    `onPointSelectRef.current(nextSelection(selectedRef.current, hit))`. A miss passes `null`,
    which clears the selection.
  - Hidden series (legend toggle) are skipped in both modes.
  - uPlot's `cursor.drag.dist` defaults to 0, so a 1px jitter would otherwise create a selection and
    zoom (`XYChart.tsx:870-883`) while also passing `isClick`. Both cursor configs (~lines 847 and
    1023) therefore set `cursor.drag.dist` to the same threshold constant `isClick` uses, measured
    on X as uPlot does.
- **Highlight: dim the unselected marks** (mockup option B). While a selection exists, every
  mark except the selected one fades. With no selection, the chart looks exactly as it does today.
  The style is read from `selectedRef` when the chart draws. The selected `dataIdx` is
  `rowIndexBySeries[seriesIndex].indexOf(rowIndex)` (the union-X index in multi-series). Only
  `rowIndexBySeries` is rebuilt with the chart; the selected `dataIdx` is derived from
  `selectedRef.current` and `rowIndexBySeries` on every use: the bar `disp` `values()` callbacks, the
  points callbacks, and the `drawClear`/`draw` hooks. A `seriesIndex` out of range or an `indexOf` of -1 (a stale
  selection from before new data arrived) counts as no selection. A selection change only triggers
  `chartRef.current?.redraw(true)`. `true` rebuilds the cached paths so the bar `disp` callbacks
  run again; the uPlot instance itself is not rebuilt.
  - **Bars.** Every bar series always gets `disp.fill` and `disp.stroke` facets. Today only
    per-row-colored series have them. uPlot requires both when the stroke width is non-zero.
    Their `values()` callbacks start from the series' base color arrays, which are the existing
    `fillArr`/`strokeArr` or, without per-row colors, the series color repeated as `hexToRgba(seriesColor, 0.6)` for fill and `seriesColor` for stroke
    (matching today's `fill`/`stroke`). When a selection
    exists, `barColors` (Design §2) returns `fadeCss(color, DIM_FILL = 0.25)` (stroke `DIM_STROKE = 0.5`) for every
    bar except the selected `(seriesIndex, dataIdx)`, which keeps its base color. SQL per-row colors are faded, not replaced, so their hue stays readable.
  - **Lines.** A `drawClear` hook in the selection plugin sets every series' `alpha` from
    `selectedRef` before the series draw, only when `chartType === 'line'` (bar series keep
    `alpha = 1` and are dimmed only through `barColors`): `DIM_LINE (0.3)` for unselected series while a selection
    exists, 1 otherwise. This covers the first draw after a rebuild and every `redraw(true)`.
    The series that owns the selection stays at 1, so its line keeps full strength. While a selection exists, its
    `points.stroke` and `points.fill` both become a faded copy of the line's own style with
    `DIM_POINT = 0.3`: `fadeCss(color, DIM_POINT)` for a plain series, or, for a per-row-colored
    series, the same gradient as its line stroke built from `fadeStops(gradStops, DIM_POINT)`, so
    each point keeps its row's hue. With no selection `points.stroke` returns the series stroke and
    `points.fill` returns `undefined`, keeping uPlot's default (the canvas `#fff` comes from uPlot's
    own fallback, so the hover dot stays solid in the series color). The line-stroke gradient code moves into one helper taking a stop
    list, shared by the stroke and the faded points. A `draw` hook then paints the selected point as
    a dot of radius 6 CSS px (12px diameter, scaled by `devicePixelRatio` in the draw hook) filled with `point.color ?? seriesColor`, in the same overlay style as `refLinePlugin`.
    uPlot's hover dot takes `points.fill` (falling back to `points.stroke`), so it looks faded on that series while a selection
    exists.
  - Hidden series (legend toggle) are unaffected. When the selected series is hidden, nothing is
    dimmed.
  - XYChart does not enable uPlot's `cursor.focus` (which also drives `series.alpha`), so nothing
    competes for that field. Keep it off.
  A separate `useEffect([selectedPoint])` updates `selectedRef` and triggers the redraw.
- **Affordance.** The selection plugin is always installed in both paths and checks
  `onPointSelectRef.current` at event time. When it is undefined, clicks do nothing and the
  `setCursor` hook leaves the default cursor. When set, the hook sets `u.over.style.cursor` to
  `pointer` while the cursor is over a hit, and back to the default otherwise.

### 4. ChartCell: own the selection and publish the row

In `ChartCell`, following the `TableCell`/`MapCell` pattern:

- `const [selected, setSelected] = useState<ChartPointHit | null>(null)`.
- `onSelectionChangeRef = useLatestRef(onSelectionChange)`.
- Clear on data change, as `MapCell` does: clear the state during render, keyed on `data`
  (`const [dataForSelection, setDataForSelection] = useState(data); if (dataForSelection !== data) { setDataForSelection(data); setSelected(null) }`),
  so XYChart never sees a stale `selectedPoint` against new data. Publish the clear after commit
  with `useEffect(() => { onSelectionChangeRef.current?.(null) }, [data, onSelectionChangeRef])`.
- `handlePointSelect(hit)`: set the state, then publish `hit ? rowValues(data[hit.seriesIndex], hit.rowIndex) : null`.
  The index works because XYChart series `i` is query `i`'s table in both paths.
- Pass `onPointSelect={selectionMode === 'single' ? handlePointSelect : undefined}` and
  `selectedPoint={selectionMode === 'single' ? selected : null}` to both `<XYChart>` usages. `selectionMode` is already a
  `CellRendererProps` field that `NotebookRenderer` fills from `options.selectionMode`, so the
  chart cell needs no registry change. It does **not** set `defaultSelectionMode`, so the
  default stays `'none'`.

### 5. Editor

In `ChartCellEditor`, render `<RowSelectionEditor selectionMode={…} cellName={v2.name} onChange={…} />`
below the Reference Lines block. The `onChange` handler writes through the existing
`updateConfig({ options: { ...v2.options, selectionMode: mode } })`, so a v1 config is
migrated to v2 on first edit like every other chart edit. The component's "Row Selection" heading
and the `$cell.selected.column` help text describe chart selection accurately too, since each
mark is one source row, so the component is reused unchanged.

`substituteOptionsWithMacros` passes `selectionMode` through `substituteMacros`, which leaves
`'single'`/`'none'` unchanged. No special case is needed.

## Mockups

These are interactive mockups in `tasks/1717_chart_click_selection_mockups/`. Each shows the same
notebook: a bar/line chart (`cost_by_user`) driving a detail table, and a two-query grouped bar
chart. Clicking a mark toggles the selection, and a side panel shows the published
`$cell.selected.*` values.

- `option-a-outline.html` — the selected bar gets a 2px outline and the selected point a ring;
  other marks are unchanged.
- `option-b-dim-others.html` — **chosen.** Unselected marks fade and the selected one stays at
  full strength.
- `option-c-band.html` — a translucent gold band over the selected X slot plus a thin outline.

## Implementation Steps

1. **arrow-utils**: add `rowIndex` to `ChartPoint` and populate it at every construction site
   in `extractChartData` / `extractMultiSeriesChartData`, including the categorical first-pass
   placeholder (`arrow-utils.ts:475`), which the second pass replaces. Update `TimeSeriesChart` in
   `XYChart.tsx`, and the ~14 `toEqual([{x, y}])` assertions in `arrow-utils.test.ts` (and any other
   test literals that build `ChartPoint`s) to include `rowIndex`.
2. **xychart-selection.ts**: add the pure helpers from Design §2, with unit tests.
3. **XYChart**: add `onPointSelect` / `selectedPoint` props, `rowIndexBySeries`, the bar `each`
   rect collection, `createSelectionPlugin` (click, highlight, cursor) in both paths, and the
   redraw-on-selection effect. Keep `onPointSelect` out of the main effect's deps by reading it through
   `onPointSelectRef`, updated in the same `useLayoutEffect` as `onTimeRangeSelectRef`, so a new
   callback identity does not rebuild the chart.
4. **ChartCell renderer**: selection state, clear-on-data-change, `handlePointSelect`, and passing
   the new props in both paths.
5. **ChartCell editor**: add `RowSelectionEditor`.
6. **Docs + CHANGELOG** (see Documentation).

## Files to Modify

- `analytics-web-app/src/lib/arrow-utils.ts` — `ChartPoint.rowIndex`
- `analytics-web-app/src/components/xychart-selection.ts` — **new**, pure hit-test helpers
- `analytics-web-app/src/components/XYChart.tsx` — selection props, plugin, bar rects, row lookup,
  `cursor.drag.dist`, and `TimeSeriesChart` setting `rowIndex`
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` — renderer state and publish, editor section
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts` — `rowIndex` coverage and updated `toEqual` assertions
- `analytics-web-app/src/components/__tests__/xychart-selection.test.ts` — **new**
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/ChartCell.test.tsx` — **new**
- `mkdocs/docs/web-app/notebooks/cell-types.md`, `mkdocs/docs/web-app/notebooks/variables.md`
- `CHANGELOG.md`

## Trade-offs

- **Hit-testing with uPlot's bar `each` callback vs. recomputing bar geometry.** `each` reports
  the exact rects uPlot drew, including the multi-series `align` offsets and the `size`/`gap`
  math. Recomputing that geometry would duplicate uPlot internals and drift from them.
- **Nearest-point radius for lines vs. snapping to the nearest X.** Snapping to the cursor's X index
  would select something on every click, so "click empty space to clear" would never trigger and
  a stray click would re-run the notebook. An 8px radius makes clearing possible and matches how
  people aim at points.

## Decisions

- Highlight style: dim the unselected marks (mockup option B), chosen by the user over the
  outline (A) and band (C) mockups.
- Downstream type resolution for `$chart.selected.col` reads the column type from the cell's
  first query table (`cellResults[cell]` = `data[0]`). Type-sensitive formatting (e.g.
  timestamps) is correct only when the column has the same name and type in query 1; other values
  substitute as plain strings. Documented as "alias X columns identically across queries".
  Changing `cellResults` to hold per-selection schemas is out of scope.
- No synthetic `series` key in the multi-query payload; publishers emit only source-row columns,
  matching table/map.
- Hiding the selected series through the legend keeps the selection published and only hides
  the highlight. The row is still valid data, and clearing would re-run downstream cells as a
  side effect of a view toggle.
- Toggling chart type (line ↔ bar) keeps the selection, because the data is unchanged. Re-execution
  clears it, as it does for table cells.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md`, Chart section:
  - add `options.selectionMode` (`'none'` | `'single'`, default `'none'`) to the config table
  - add a "Click selection" feature bullet: click a bar or point to publish
    `{x_col, y_col, color?}` as `$cell.selected.*`, click again or click empty space to clear,
    drag-to-zoom is unaffected
  - note the multi-query aliasing guidance from Decisions
- `mkdocs/docs/web-app/notebooks/variables.md`:
  - macro table (line 46): reword to "the selected row in an upstream cell"
  - "Row Selection" section (line 116): scope the radio-button sentence to table cells, and add that
    chart and map cells select by clicking a mark
- `CHANGELOG.md` → Unreleased: `* **Notebooks:** Chart cells support \`selectionMode: 'single'\` … (#1717)`.
  This is an additive config option, not a breaking change.

## Testing Strategy

Every unit test below runs without a DB.

- `arrow-utils.test.ts`:
  - time/numeric input given out of order: after the sort, each point's `rowIndex` still points at
    its source row
  - rows with null or non-finite values are skipped, and later points' `rowIndex` is unaffected
  - categorical single-series keeps SQL order and `rowIndex`
  - multi-series categorical: the second-pass remap sets `rowIndex` per table
- `xychart-selection.test.ts`:
  - `hitTestBars`: inside, edge, outside, overlapping rects across grouped series
  - `findNearestPoint`: within radius, beyond radius, tie-breaking, empty candidate list
  - `isClick`: below and above the threshold
  - `nextSelection`: select, re-click clears, click another switches, miss clears
  - `fadeCss`: `#rrggbb`, `#rrggbbaa` (the existing alpha is multiplied), `rgba(...)`, factor 1
    returns the same color
  - `fadeStops`: every stop's color is faded and its `x` is unchanged.
  - `barColors`: no selection keeps the base colors,
    a selection fades every bar but the selected one, and a selection in another series fades all
    bars of this series.
- `ChartCell.test.tsx`: mock `@/components/XYChart` to capture props, build tables inline with
  `tableFromArrays` (as `MapCell.test.tsx` / `PieChartCell.test.tsx` do), and mock
  `@/lib/data-sources-api` as `HorizontalGroupCell.test.tsx:63-66` does, then check:
  - with no `selectionMode`, `onPointSelect` is undefined (the feature is off, so behavior is unchanged)
  - with `selectionMode: 'single'`, calling the captured `onPointSelect({ seriesIndex: 0, rowIndex: 2 })`
    publishes `rowValues(data[0], 2)` and passes `selectedPoint` back
  - multi-query: `seriesIndex: 1` publishes from `data[1]`, not `data[0]`
  - `onPointSelect(null)` publishes `null`
  - re-rendering with `selectionMode` undefined after a selection passes `selectedPoint` as null
  - new `data` clears both the state and the published selection
  - editor: click the "Row Selection" header to expand it first (`RowSelectionEditor` starts
    collapsed when the mode is `'none'`), then toggling it on a v1 config writes a v2 config with
    `options.selectionMode: 'single'`

The uPlot wiring (the canvas `each` callback, DOM mouse events on `u.over`, `valToIdx`/`posToVal`/`valToPos`,
and the draw hook) needs real canvas layout, which jsdom does not provide. If it breaks, the next
click on a chart shows it right away, so it is verified by hand below.

## Manual Verification

Run `python3 local_test_env/ai_scripts/start_services.py --monolith`, then `yarn dev` in
`analytics-web-app/`. Open http://127.0.0.1:3000 and create a notebook with these cells:

1. Chart `by_level`, bar type, `selectionMode: single`:
   `SELECT CAST(level AS VARCHAR) AS level, COUNT(*) AS n FROM log_entries GROUP BY level ORDER BY level`.
   Below it, a table:
   `SELECT time, msg FROM log_entries WHERE CAST(level AS VARCHAR) = '$by_level.selected.level' LIMIT 50`.
   → The table shows "Select a row in "by_level"…". Clicking a bar dims the other bars and fills
   the table with that level's logs. Clicking the same bar clears it, and the table goes back to
   waiting. Clicking empty plot area also clears it.
2. Switch the chart to line type → clicking near a point (within about 8px) enlarges it to a dot of radius 6 CSS px, fades
   the other points, and publishes it. A click far from any point clears the selection. The cursor becomes a pointer
   over points.
3. Time-axis line chart with selection on (`SELECT date_bin(INTERVAL '1 minute', time) AS t, COUNT(*) AS n FROM log_entries GROUP BY 1 ORDER BY 1`):
   → a horizontal drag still zooms the time range and does not select. A plain click on a point
   still selects it.
4. Add a second query to the bar chart (same `level` X alias, Y aliased `n2`) → bars are grouped.
   Clicking the second bar in a group leaves only that bar at full strength. Every other bar in
   both series dims, and `$by_level.selected.n2`
   resolves downstream.
5. Add a `color` column (`CASE WHEN COUNT(*) > 100 THEN rgba(0.8,0.1,0.1,1) ELSE rgba(0.2,0.7,0.2,1) END AS color`)
   → after selecting, the unselected bars stay red or green, just faded.
