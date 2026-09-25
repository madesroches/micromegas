# Stacked Bar Cell Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1511

## Overview
Add a `stackedbar` notebook cell type for "breakdown of X across several things" comparisons: one vertical bar per category, each bar stacked from named series sharing a single legend and color mapping (e.g. time per startup phase across test scenarios, resource usage per component across builds). The query returns long/tidy rows `(category, series, value[, color])`. The cell has no normalize toggle. A 100%-stacked view is just a different query: the SQL computes each row's share with a window function and sets the unit to `percent`. The cell follows the Pie Chart cell's pattern: one query, hand-rolled SVG, registered like every other cell type.

Source issue: [#1511](https://github.com/madesroches/micromegas/issues/1511).

## Current State
- **Chart cell** (`src/lib/screen-renderers/cells/ChartCell.tsx`, on uPlot via `src/components/XYChart.tsx`) draws one value per x position per query. Its 2-column contract (`validateChartColumns`, `src/lib/arrow-utils.ts:274`) has no series column, and uPlot has no native stacking. Stacking there means pre-accumulated data, band fills and tooltip un-stacking, all mixed into a component that is already about 1,300 lines.
- **Pie Chart cell** (`src/lib/screen-renderers/cells/PieChartCell.tsx`) is the direct precedent: one query, `QueryCellConfig` with an `options` bag, inline SVG, a side legend, a fixed-position tooltip div, and `contrastingTextColor` for labels drawn inside fills. `extractPieData` (`arrow-utils.ts:687`) reuses `validateChartColumns`.
- **Color column handling** lives inside `resolveChartColumns` / `validateChartColumns` (`arrow-utils.ts:236-341`). Both detect the case-insensitive `color` field, classify it as integer/string/binary and validate its type. `cellColorToCss` (`src/lib/color-utils.ts:97`) decodes a cell. That logic is inline and hard-coded to "exactly 2 non-color columns", so a 3-column contract can't call it as-is.
- **Palette**: `SERIES_COLORS` (`src/components/chart-constants.ts`) holds 12 hues in fixed order.
- **Registration touch points**: the `CellType` and `QueryCellConfig['type']` unions (`notebook-types.ts:104`, `:123`), `DEFAULT_SQL` (`notebook-utils.ts:~220`), the comment listing default-branch types in `shouldShowTimeRange` (`notebook-utils.ts:423`), and `CELL_TYPE_METADATA` (`cell-registry.ts:~230`).
- **Formatting**: `formatValueWithUnit` (`src/lib/format-value.ts:54`) already handles `percent` (and its `%` alias via `units.ts:83`). `estimateLabelWidth` / `ROTATE_DEG` (`src/components/xychart-axis.ts`) cover x-label fit and rotation.
- **Docs**: `mkdocs/docs/web-app/notebooks/cell-types.md` says "15 cell types" (line 3), and `index.md` repeats the count twice (lines 28, 89).

## Design

### Config shape
No new config interface. Add `'stackedbar'` to `CellType` and `QueryCellConfig['type']`.

`options`:
| Key | Type | Default | Meaning |
|---|---|---|---|
| `unit` | `string` | — | Value unit, macro-substituted and formatted via `formatValueWithUnit`. Use `percent` for SQL-normalized queries |

There is deliberately no `normalize` option and no series cap (see Decisions).

### SQL contract
| Column | Required | Meaning |
|---|---|---|
| 1st non-color | yes | Category (one bar each). String, dictionary string, or numeric (stringified) |
| 2nd non-color | yes | Series (one stack segment each, shared legend). String or dictionary string |
| 3rd non-color | yes | Numeric value |
| `color` | no | Per-series color. The first non-null value seen for a series wins |

- **Order**: categories appear left to right, and series stack bottom-up and list in the legend, in first-appearance order of the query rows. The query's `ORDER BY` controls both. A series missing from one bar leaves a gap in that bar and keeps its place everywhere else.
- **Dropped rows**: rows with a null category or series, or a null, non-finite or negative value, are dropped, as the Pie cell does. Stacking negatives needs a diverging baseline, which is out of scope. A zero value draws no segment.
- **Duplicates**: duplicate `(category, series)` rows are **summed**. This differs from the Pie cell, where duplicates stay separate slices. In a stack, two same-colored neighboring segments would read as one segment with a spurious gap, so summing is the only faithful rendering.

Normalized example (documented, not built in):
```sql
SELECT scenario, phase,
       100.0 * sum(duration_ms) / sum(sum(duration_ms)) OVER (PARTITION BY scenario) AS share
FROM ...
GROUP BY scenario, phase
ORDER BY scenario, phase
```
Set the unit to `percent`.

### Data extraction — factor out the color-column logic
1. In `arrow-utils.ts`, extract `resolveColorColumn(fields) → { index, name?, kind?, error? }` from the inline code in `resolveChartColumns` / `validateChartColumns`. Both then call it, with no behavior change; the existing tests pin that.
2. Add:
```ts
export interface StackedBarData {
  categories: string[]
  series: { name: string; color?: string }[]
  /** values[c][s]; 0 where absent */
  values: number[][]
}
export function extractStackedBarData(table: Table):
  | { ok: true; data: StackedBarData }
  | { ok: false; error: string }
```
It validates exactly 3 non-color columns (category: string/dictionary/numeric; series: string/dictionary; value: numeric), with error messages in the same style as `validateChartColumns`. It then pivots in one pass using `Map` indices, applying the ordering, dropping and summing rules above.

### Series colors (pure, in the cell file)
`resolveSeriesColors(series)`: a SQL-supplied color wins. Otherwise each series takes the next `SERIES_COLORS` entry in series order, wrapping after 12 the way the Chart cell does. This is the same rule as `groupPieSlices`, minus the folding. Every series the query returns is drawn. A query author who wants fewer segments folds the tail in SQL (for example a `CASE` mapping minor series to `'Other'`), the same way normalization lives in SQL.

### Layout math (pure, unit-testable)
`buildStackedBarLayout(resolved, { width, height, unit })` returns bar and segment rectangles, y ticks and x-label placement:
- **Y scale**: `step = niceStep(maxTotal / 5)` (1/2/2.5/5/10 × 10ⁿ). Compute `top = ceil(maxTotal / step − 1e-9) × step`; the epsilon keeps a floating-point 100.0000001% from rounding up to a 120% axis. Tick labels use `formatValueWithUnit`. The axis gutter width comes from `estimateLabelWidth` over the tick labels.
- **Bars**: band = plot width / categories. Bar width = `min(56px, band × 0.62)`, floored at 12px. Below the floor, the plot area gets a minimum width and the plot div scrolls horizontally rather than squashing bars.
- **Segments**: a 2px surface gap (panel background) separates stacked segments. Only the top segment of each bar gets a 4px rounded cap; the baseline stays square. These follow the dataviz mark spec, as in the Pie cell.
- **X labels**: centered under each bar. They rotate to `ROTATE_DEG` when any label's `estimateLabelWidth` exceeds the band, and are truncated with a `<title>` beyond a max width.
- **Width**: measured with a `ResizeObserver` on the plot container, the same approach as `XYChart` / `FlameGraphCell`.

### Rendering (`StackedBarCell.tsx`)
- **Header**: stats `categories`, `series`, and `max total`, unit-formatted. This mirrors the Pie and XY header rows. There is no toggle in the header.
- **Plot**: inline SVG with hairline gridlines, a y axis, stacked `<path>`s, and value labels inside segments (Option A). A label is drawn only when the segment is at least 18px tall and the measured text plus padding fits the bar width. It is never clipped, and its ink is chosen by `contrastingTextColor`.
- **Tooltip** (per segment, pointer events): category, series swatch and name, value, **share of bar**, and bar total. The share is a display-only derived value, not normalization: it is always correct whatever the query returned.
- **Legend**: always shown, one row per resolved series in stack order.
- **States**: loading, empty, error and all-zero, copied from `PieChartCell`.

### Editor
It mirrors `PieChartCellEditor`:
- SQL `SyntaxEditor` with a contract note under it that includes the window-function tip for 100% stacking
- `Unit` field, with its macros validated
- `AvailableVariablesPanel` and `DocumentationLink`

The Data Source selector comes from `CellEditor`'s shared chrome, as it does for the Pie cell.

### Metadata
- `label: 'Stacked Bar'`, `icon: <ChartColumnStacked />` from lucide-react (the export exists in the installed version)
- `defaultHeight: 360`, `canBlockDownstream: true`
- `execute` and `getRendererProps` identical in shape to `pieChartMetadata`

The two `execute` bodies are the same 8 lines. Folding them into a shared helper is tempting, but `ImageCell` and `MapCell` carry near-copies too. That refactor would span five cells, so it's left out of scope here (see Trade-offs).

## Mockups
In `tasks/stacked_bar_cell_mockups/`. All are vertical, show the absolute query and the SQL-normalized query as two states of the same cell, have working hover tooltips, and were checked in headless Chromium:

- `option-a-side-legend-segment-labels.html` — The issue's reference shape: y axis, a value inside every segment that fits, and a plain side legend. Most information at a glance, but the busiest; it bends the dataviz "label selectively" guidance, which the issue explicitly asks for.
- `option-b-top-legend-bar-totals.html` — The legend wraps in a row above the plot so bars get the full cell width, and only bar totals appear on the caps (segment values in the tooltip). Quietest, and best for many categories or narrow cells. The percent view still uses in-segment labels, since every total is 100%.
- `option-c-summary-legend-series-focus.html` — Side legend as a summary table: each series' total and share (average share in percent view). Hovering a legend row dims all other series, which helps with the hardest part of reading stacked bars: comparing a middle segment across bars. Bar totals on the caps, no in-segment labels.

**Chosen: Option A** — side legend and in-segment labels, the same layout as the Pie Chart cell. B and C stay as reference only; C's legend-hover fade and per-series legend totals are not built.

## Implementation Steps

### Phase 1 — Data layer
1. `src/lib/arrow-utils.ts`: extract `resolveColorColumn`, rewire `resolveChartColumns` and `validateChartColumns` to use it, then add `StackedBarData` and `extractStackedBarData`.
2. `src/lib/__tests__/arrow-utils.test.ts`: tests for `extractStackedBarData`, covering:
   - pivot with first-appearance ordering, and a series missing from one bar
   - duplicate `(category, series)` rows summed
   - null, negative and non-finite rows dropped
   - dictionary-encoded strings and numeric categories accepted
   - wrong column count and non-numeric value errors
   - `color` decoding and first-non-null-per-series
   - empty table

   The existing chart-column tests must still pass unchanged after the refactor.

### Phase 2 — Types and registration
3. `notebook-types.ts`: add `'stackedbar'` to both unions.
4. `notebook-utils.ts`: add a `DEFAULT_SQL.stackedbar` entry that works on any install, for example log count per level across processes:
   ```sql
   SELECT exe, <level-name CASE as in DEFAULT_SQL.piechart>, count(*) FROM log_entries GROUP BY 1, 2 ORDER BY 1, 2
   ```
   Also add `stackedbar` to the `shouldShowTimeRange` default-branch comment.
5. `cell-registry.ts`: register `stackedbar: stackedBarMetadata`.

### Phase 3 — Cell
6. New `src/lib/screen-renderers/cells/StackedBarCell.tsx`: `resolveSeriesColors`, `niceStep`, `buildStackedBarLayout` (exported for tests), the `StackedBarCell` renderer, `StackedBarCellEditor`, and `stackedBarMetadata`.
7. New `src/lib/screen-renderers/cells/__tests__/StackedBarCell.test.tsx`, modeled on `PieChartCell.test.tsx`:
   - **`resolveSeriesColors`**: SQL colors override the palette; the palette doesn't skip an entry for SQL-colored series; it wraps after 12 series
   - **`niceStep` / `buildStackedBarLayout`**:
     - clean tick steps
     - a total of 100.0000001 with `percent` gives a 100 top, not 120
     - segments stack contiguously with the 2px gap
     - rounded cap only on the top segment
     - the minimum bar width triggers a scroll width
     - rotation when labels exceed the band
     - an in-segment label is skipped when the segment is too short or narrow
   - **Renderer**: loading, empty, error and all-zero states; legend rows in stack order; tooltip content (including share of bar) on `pointerMove`
   - **Editor**: `unit` macro validation error display

### Phase 4 — Docs
8. `mkdocs/docs/web-app/notebooks/cell-types.md`: add a `## Stacked Bar` section between Reference Table and Swimlane (alphabetical). It covers:
   - the config table and the SQL column contract
   - ordering, dropping and summing rules
   - the normalization-via-SQL example
   - an absolute example

   Add the icon `mkdocs/docs/assets/images/cell-icons/chart-column-stacked.svg`, exported from lucide like the others. Bump "15 cell types" to 16.
9. `mkdocs/docs/web-app/notebooks/index.md`: bump both counts (lines 28 and 89) to 16.
10. `CHANGELOG.md`: add an entry under Unreleased.

## Files to Modify
- `analytics-web-app/src/lib/arrow-utils.ts`
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/StackedBarCell.tsx` (new)
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/StackedBarCell.test.tsx` (new)
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `mkdocs/docs/web-app/notebooks/index.md`
- `mkdocs/docs/assets/images/cell-icons/chart-column-stacked.svg` (new)
- `CHANGELOG.md`

## Trade-offs
- **New cell vs. a `stacked` mode on the Chart cell.** The Chart cell's contract (x, y), its multi-query v1/v2 config, its time/numeric x axes, P99/max scale modes and uPlot all assume one value per x per series. Stacking would need a third column meaning "series", which conflicts with multi-query as the existing way to get several series. It would also need stacked-data preprocessing plus tooltip un-stacking inside uPlot. A separate cell keeps both contracts simple, and it's the open/closed route the Pie cell already took.
- **Hand-rolled SVG vs. uPlot or a new library.** uPlot's stacking is a demo-level recipe, not an API, and the categorical axis is simple to build directly. The Pie cell set the precedent of keeping charting dependencies at uPlot only.
- **Long/tidy vs. wide format** (one numeric column per series). Tidy works with `GROUP BY category, series` without the query author knowing the series set in advance, and it's what the issue proposes. Wide format would require pivot SQL for every new series.
- **Bar width cap of 56px vs. the dataviz 24px cap.** In-segment labels (Option A) need room for text like `42.6%`. At 24px nothing fits, and the issue asks for those labels. The cap only applies when bands are wide. With many categories, bars shrink toward the 12px floor anyway.
- **Not sharing `execute` / `getRendererProps` across single-query cells now.** The duplication predates this feature and spans five cells. Folding it into this change would widen the diff and the review surface for no functional gain. It's worth its own cleanup.

## Decisions
- Normalization is done in SQL, not by a UI toggle or option (user call). The cell renders the values it's given, and the docs and editor hint show the window-function recipe with `unit: percent`.
- Vertical bars only (user call). No horizontal orientation option.
- No series cap or "Other" folding in the cell (user call). The query decides how many series there are; past 12 the palette wraps, as in the Chart cell.
- No segment click or cell selection (user call). The cell exposes no `$cell.selected.*` values.
- Option A layout (user call): side legend with names only, in-segment labels where they fit; no legend-hover fade.

## Documentation
- `mkdocs/docs/web-app/notebooks/cell-types.md` — new Stacked Bar section and cell-type count (step 8)
- `mkdocs/docs/web-app/notebooks/index.md` — counts (step 9)
- `CHANGELOG.md` — entry (step 10)

## Testing Strategy
All behavior is reachable with constructed Arrow tables and props, so everything is covered by Vitest unit and component tests (Phases 1 and 3):
- extraction, pivoting and validation
- series color assignment
- tick and layout math, including the percent-rounding edge case
- label-fit decisions
- renderer states, legend and tooltip
- editor wiring

No live-DB or service test: this is a new feature, not a pinned bug. Run `yarn lint`, `yarn type-check` and `yarn test` before the PR.

## Manual Verification
Only for what needs eyes: visual polish that no assertion captures (label legibility on each palette hue, rotated-label spacing, the scroll threshold feel).
1. `python3 local_test_env/ai_scripts/start_services.py --monolith`, then open http://127.0.0.1:3000 and add a Stacked Bar cell. Expected: `DEFAULT_SQL.stackedbar` renders bars per process with level segments and a legend.
2. Replace the query with the window-function form and set the unit to `percent`. Expected: every bar reaches exactly the 100% gridline, and the axis tops out at 100%.
3. Resize the cell narrow, and query more than 30 categories. Expected: x labels rotate, then the plot scrolls horizontally; bars never go below 12px.
