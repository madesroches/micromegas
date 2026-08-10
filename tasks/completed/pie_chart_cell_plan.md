# Pie Chart Cell Plan

## Overview
Add a `piechart` notebook cell type to the analytics web app so users can visualize proportions/breakdowns (share of events by category, error-type distribution, etc.) — a shape that doesn't fit the existing X/Y `chart` cell. The cell runs a single SQL query returning `(category, value[, color])`, renders a pie or donut with a legend, and follows the same registration pattern as every other cell type (`FlameGraphCell`, `MapCell`, `ImageCell`).

Source issue: [#1339](https://github.com/madesroches/micromegas/issues/1339).

## Current State
- Cell types are a closed union (`CellType` in `analytics-web-app/src/lib/screen-renderers/notebook-types.ts:99`) with 14 members. Query-backed cells share `QueryCellConfig` (`notebook-types.ts:117-121`): `{ sql, options?, dataSource?, timeRange? }`.
- Each cell type is one file under `src/lib/screen-renderers/cells/` exporting a `CellTypeMetadata` object (renderer, editor, icon, `createDefaultConfig`, `execute`, `getRendererProps`), wired into `CELL_TYPE_METADATA` in `cell-registry.ts:191-206`. No other file needs to know about a new type by name — `shouldShowDataSource`/`shouldShowTimeRange` (`notebook-utils.ts:125-127`, `:342-353`) default to "show" unless a type opts out.
- The existing `ChartCell.tsx` (`src/lib/screen-renderers/cells/ChartCell.tsx`) is line/bar only, built on `XYChart.tsx` (uPlot-based, inherently X/Y). There is no pie/donut charting primitive in the app and no plotting dependency beyond `uplot` (`analytics-web-app/package.json`) — uPlot doesn't do radial charts, so this needs a small hand-rolled SVG renderer, not a new dependency.
- Column resolution + SQL-driven color decoding already exists for X/Y charts: `resolveChartColumns`/`validateChartColumns`/`cellColorToCss` in `src/lib/arrow-utils.ts:213-312` and `src/lib/color-utils.ts:57-74`. These accept an optional `color` column (packed RGBA u32, hex string, or 4-byte binary) and are exactly the shape a pie chart needs (first non-color column = category, second = numeric value) — just relabeled.
- `SERIES_COLORS` (`src/components/chart-constants.ts`) is the app's one fixed-order categorical palette, already used by `ChartCell` for per-series legend colors. It's the natural default palette for pie slices too, for visual consistency across cell types.
- Docs: `mkdocs/docs/web-app/notebooks/cell-types.md` documents all 14 types (one `##` section each, with an SVG icon from `mkdocs/docs/assets/images/cell-icons/`); `index.md` says "14 available cell types".

## Design

### Config shape
No new config interface — add `'piechart'` to the existing type unions:
- `CellType` (`notebook-types.ts:99`)
- `QueryCellConfig['type']` (`notebook-types.ts:118`)

`options` (all optional, same bag pattern as `ChartCellConfigV2.options`):
| Key | Type | Default | Meaning |
|---|---|---|---|
| `unit` | `string` | — | Value unit, formatted via `formatValueWithUnit` (same as Chart cell) |
| `chart_type` | `'pie' \| 'donut'` | `'donut'` | Shape toggle, same UX pattern as Chart cell's line/bar toggle |
| `max_slices` | `number` | `8` | Cap on visible slices before folding the tail into "Other" |

### Data extraction — reuse, don't duplicate
Add `extractPieData(table)` to `arrow-utils.ts`, built on the existing `validateChartColumns`/`resolveChartColumns` (same 2-or-3-column contract, just read as category/value instead of x/y — no need for `detectXAxisMode`/categorical-index remapping, since a pie has no axis):

```ts
export interface PieSlice { label: string; value: number; color?: string }

export function extractPieData(table: Table):
  | { ok: true; slices: PieSlice[] }
  | { ok: false; error: string }
```

Logic: validate via `validateChartColumns` (rejecting non-numeric value column, wrong column count, exactly as Chart does today); iterate rows, skip null/non-finite values (a negative value is also dropped — a negative slice has no geometric meaning and would silently invert the arc math), decode `color` via `cellColorToCss` when present. Values are trusted to already be aggregated by the query's own `GROUP BY` (matching every existing chart example's convention) — no client-side merge of duplicate labels.

### "Other" grouping (client-side, in the renderer)
Given `slices` and `max_slices` (N):
1. Sort descending by `value`.
2. If `slices.length > N`, keep the top `N - 1` and fold the remainder into one `Other` slice (`value` = sum of the tail, tooltip shows how many categories were folded).
3. Assign colors: each visible slice gets `slice.color` if the query supplied one, else the next unused `SERIES_COLORS` entry in fixed order; `Other` is always a fixed muted gray (`theme-text-muted`, not the rotating palette) — the dataviz skill's guidance ("more than ~7 classes → fold into Other") capped at 8 (7 real + Other) by default, editable per cell.

### Rendering — hand-rolled inline SVG (no new dependency)
`PieChartCell.tsx` renders a `<svg>` with one `<path>` per slice, arcs computed the standard way (`M/L/A/Z` for pie, `M/A/L/A/Z` ring segment for donut) — see the two mockups for the exact math, it's ~15 lines. Per the `dataviz` skill's mark spec, each slice gets a 2px stroke in the panel-background color (`#12121a`) as a "surface gap" separating touching slices, since there's no other boundary between adjacent fills.

- **Direct labels**: percentage shown inside slices ≥ 8% share; smaller slices rely on the legend/tooltip (avoids label collision on thin slices).
- **Tooltip**: `pointermove`/`pointerleave` per slice — category, value (via `formatValueWithUnit`), and percentage; matches `XYChart`'s tooltip conventions (fixed-position `div`, not a library).
- **Legend**: always rendered (never color-only identification — see palette note below), one row per visible slice: swatch, label, value, percentage. Static (no click-to-isolate/toggle-visibility — that's a Chart-cell affordance for multi-series XY data, not obviously useful for a static proportion snapshot; can be added later if requested).
- **Donut center**: when `chart_type: 'donut'`, the ring's center shows the total (sum of all values, formatted with `unit`) and a small "total" caption — see mockup Option A.
- **Header**: category count + total, mirroring `XYChart`'s stats row (`min`/`p99`/`max`/`avg`/`count`), plus the Pie/Donut toggle in the same header slot Chart uses for its Line/Bar toggle.

### Palette note (known constraint, not fixed here)
`SERIES_COLORS` was validated against the app's dark panel surface (`#12121a`) using the `dataviz` skill's `validate_palette.js`: it **fails** the lightness-band check (Wheat `#ffb300` too light) and **warns** on contrast for 2-3 hues (Violet Dusk, Pink Dusk) at low counts. This is a pre-existing, app-wide palette already used by Chart-cell legends — out of scope to redesign here. It matters more for a pie than a line chart because a pie identifies categories by fill color *alone* (no secondary position encoding). Mitigation, per the skill's non-dismissable-WARN guidance: the legend is always visible (never optional), the 2px surface-gap stroke keeps slice boundaries legible even between low-contrast adjacent hues, and the default slice cap (8) keeps adjacent-hue collisions rare. See Open Questions for whether the palette itself should be revisited app-wide.

## Mockups
Two standalone HTML mockups in `tasks/pie_chart_cell_mockups/` (both verified rendering correctly in headless Chromium):

- `option-a-donut-side-legend.html` — Donut with a center "total" readout and a side legend (swatch/label/value/%). Best when the total itself is a meaningful number (e.g. total error count).
- `option-b-pie-legend-below.html` — Full pie with the legend wrapped below, and demonstrates the top-N + "Other" grouping (9 raw categories → top 5 + Other). Reads as a cleaner disc shape; legend-below suits narrower cells better than a side legend.

**Decision: Option A** — donut with center total + side legend is the layout being built. The `chart_type: 'pie' | 'donut'` toggle still ships (same arc math, one parameter, cheap to keep), but the legend is always side-placed; Option B's legend-below layout is not implemented. Option B's other property — top-N + "Other" folding — is adopted regardless of shape, since it's independent of layout.

## Implementation Steps

### Phase 1 — Data layer
1. `src/lib/arrow-utils.ts`: add `PieSlice` interface and `extractPieData(table)`, built on `validateChartColumns`/`resolveChartColumns`/`cellColorToCss`.
2. `src/lib/__tests__/arrow-utils.test.ts`: tests for `extractPieData` — happy path, wrong column count, non-numeric value column, `color` column decoding (integer/string/binary), null/negative value rows dropped, empty table.

### Phase 2 — Types & registration
3. `src/lib/screen-renderers/notebook-types.ts`: add `'piechart'` to `CellType` (line 99) and to `QueryCellConfig['type']` (line 118).
4. `src/lib/screen-renderers/notebook-utils.ts`: add a `piechart` entry to `DEFAULT_SQL` (e.g. `SELECT level, count(*) AS count FROM log_entries GROUP BY level ORDER BY count DESC`); update the comment on `shouldShowTimeRange`'s default branch to list `piechart` (no logic change — the `default: return true` already covers it, same for `shouldShowDataSource`).
5. `src/lib/screen-renderers/cell-registry.ts`: import `pieChartMetadata` from `./cells/PieChartCell`; add `piechart: pieChartMetadata` to `CELL_TYPE_METADATA`.

### Phase 3 — Cell component
6. New `src/lib/screen-renderers/cells/PieChartCell.tsx`:
   - `PieChartCell` renderer: `extractPieData` → top-N/"Other" folding → inline SVG pie/donut + tooltip + legend + header stats, following `ImageCell`/`ChartCell`'s loading/empty/error states.
   - `PieChartCellEditor`: SQL editor (`SyntaxEditor`, reusing the `color`-column doc note pattern from `ChartCellEditor`), Data Source selector (`DataSourceSelector`, cell-level like `TableCell`/`MapCell` — single query, unlike Chart's per-query selector), `Unit` text field, Pie/Donut toggle, `Max slices` numeric input, `AvailableVariablesPanel` + `DocumentationLink`, macro validation on `unit` (reuse `validateMacros`).
   - `pieChartMetadata`: `label: 'Pie Chart'`, `icon: <PieChart />` (lucide-react — already exported, confirmed), `description`, `defaultHeight: 320`, `canBlockDownstream: true`, `createDefaultConfig`, `execute` (single `runQueryAs`/`runQuery` call, same shape as `ImageCell.execute`), `getRendererProps`.
7. `src/lib/screen-renderers/cells/__tests__/PieChartCell.test.tsx`: renderer states (loading/empty/error/happy path), "Other" folding at the `max_slices` boundary, pie/donut toggle persists via `onOptionsChange`, editor field wiring — mirroring `MapCell.test.tsx`/`FlameGraphCell.test.tsx` structure.

### Phase 4 — Docs
8. `mkdocs/docs/web-app/notebooks/cell-types.md`: add a `## Pie Chart` section (config table, SQL columns, features, example) in the same format as the existing `## Chart` section; add a `pie-chart.svg` icon asset under `mkdocs/docs/assets/images/cell-icons/` (no generator script exists — export from `lucide-react`'s `PieChart` icon, same as the other hand-added icons in that folder).
9. `mkdocs/docs/web-app/notebooks/index.md`: bump "14 available cell types" → "15".

## Files to Modify
- `analytics-web-app/src/lib/arrow-utils.ts` (new `extractPieData`)
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/PieChartCell.tsx` (new)
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/PieChartCell.test.tsx` (new)
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `mkdocs/docs/web-app/notebooks/index.md`
- `mkdocs/docs/assets/images/cell-icons/pie-chart.svg` (new)

## Trade-offs
- **Hand-rolled SVG vs. a charting library**: uPlot (the app's only charting dependency) doesn't support radial charts, and every other visually-custom cell in this app (flame graph, map, property timeline) already hand-rolls its rendering rather than pulling in a new dependency for one chart type. A pie/donut's arc math is ~15 lines; not worth a new dependency.
- **Single query vs. multi-query (like Chart cell)**: Chart's `ChartCellConfigV2`/multi-query machinery exists because multiple X/Y series can share one time axis. A pie chart has no shared axis to merge series onto — "multiple pies" isn't a coherent single chart. Kept single-query, matching `ImageCell`/`MapCell`/`FlameGraphCell` rather than `ChartCell`'s heavier v1/v2-migration pattern.
- **Client-side "Other" folding vs. requiring the query to do it**: SQL could do `LIMIT N` + a `UNION` for "everything else", but that pushes chart-presentation logic into every query author's SQL. Folding client-side (like the cap is just a cell option) keeps the SQL simple and the cap user-adjustable without touching the query.
- **Donut default vs. plain pie**: donut wins by a small margin (free "total" readout, same complexity) but the toggle exists so it isn't a one-way decision.

## Documentation
- `mkdocs/docs/web-app/notebooks/cell-types.md` — new `## Pie Chart` section (Phase 4, step 8).
- `mkdocs/docs/web-app/notebooks/index.md` — cell-type count (Phase 4, step 9).

## Testing Strategy
- Unit tests for `extractPieData` covering the column-validation and color-decoding paths shared with `ChartCell` (Phase 1).
- Component tests for `PieChartCell` covering loading/empty/error rendering, the "Other" folding boundary, and pie/donut toggle persistence (Phase 3).
- Manual verification in the running app (`yarn dev` + backend): add a Pie Chart cell, confirm it against the `DEFAULT_SQL.piechart` query, toggle Pie/Donut, exceed `max_slices` to confirm "Other" grouping, and add a `color` column to a query to confirm SQL-driven per-slice colors work like they do for Chart cells.
- `yarn lint` / `yarn type-check` / `yarn test` before PR (per project CLAUDE.md).

## Open Questions
1. Is `max_slices` (default 8) the right default cap, or should it be lower (e.g. 6, matching the dataviz skill's stricter "≤6 segments" guidance for pie specifically)?
2. Should clicking a slice/legend row set a variable or emit a selection (like `TableCell`'s `selectionMode`), enabling drill-down into the selected category? Not requested by the issue; left out of this plan's scope.
3. Separately from this feature: should `SERIES_COLORS` itself be revisited app-wide to pass the `dataviz` skill's palette validator against the dark panel surface? Flagged here because it's most visible in a pie chart, but the palette is shared by every chart type — likely its own follow-up issue rather than something to fix as a side effect of adding pie charts.
4. If a future notebook needs the legend-below layout (mockup Option B, e.g. for a very narrow cell), should that become a second layout mode, or is manually widening the cell sufficient? Not needed for the initial implementation.
