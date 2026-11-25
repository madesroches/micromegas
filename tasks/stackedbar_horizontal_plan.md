# Bar Chart Cell: Horizontal Orientation, Optional Series, Selection Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1820

## Overview
A ranked bar chart with many categories (e.g. cost per user across ~50 users) is unreadable in the `chart` cell. The bars get thin, the labels crowd, and uPlot always puts categories on the x axis. This plan extends the existing `stackedbar` cell instead of adding a cell type. It adds a `horizontal` orientation (fixed-height rows, a label column, vertical scroll, a sticky value axis), accepts a 2-column `category, value` query as a plain single-series bar chart, adds click selection (`selectionMode: 'single'`) that drives downstream cells, and adds optional bar-total value labels. The cell type id stays `stackedbar`. Its display label becomes "Bar Chart".

## Current State
- **Cell**: `analytics-web-app/src/lib/screen-renderers/cells/StackedBarCell.tsx` (655 lines). It hand-rolls SVG, and its layout math is pure and exported for tests:
  - `resolveSeriesColors` (`:35`) assigns a SQL color first, then the next `SERIES_COLORS` entry.
  - `niceStep` (`:77`) picks tick steps.
  - `truncateLabel` (`:129`) truncates labels.
  - `buildStackedBarLayout` (`:144`) computes bar and segment rects, y ticks, rotated labels and the 12px bar-width floor. The floor grows `plotWidth` past the container to force horizontal scroll.
  - `roundedTopRectPath` (`:224`) draws the rounded cap on the top segment.
  - `StackedBarView` (`:263`) owns the `ResizeObserver`, hover state, the SVG, the side legend and the fixed tooltip.
  - `StackedBarCell` (`:439`) handles empty and error states and the header stats. `StackedBarCellEditor` (`:538`) holds the SQL and the unit. `stackedBarMetadata` (`:618`) has the label `'Stacked Bar'`.
  - Options: `unit` only.
- **Extraction**: `extractStackedBarData` (`src/lib/arrow-utils.ts:772`) requires exactly 3 non-color columns `(category, series, value)`. It pivots them into `{ categories: string[], series: {name, color?}[], values: number[][] }`, sums duplicates, drops null, negative and non-finite rows, and stringifies numeric categories. The `color` column is per series, and the first non-null value wins.
- **Selection plumbing** already exists and is generic:
  - `NotebookRenderer.tsx:653-686` and `HorizontalGroupCell.tsx:185-214` read `options.selectionMode` and pass `selectionMode` + `onSelectionChange` to any renderer.
  - `useCellExecution.updateCellSelection` (`useCellExecution.ts:521`) stores the row and re-runs downstream cells.
  - `RowSelectionEditor` (`src/components/RowSelectionEditor.tsx`) is the editor section.
- **Selection precedent: the Chart cell**:
  - `ChartCell.tsx:216-238` keeps a `selected` hit in state. It clears the hit *during render* when `data` changes, so the chart never draws a stale hit, and an effect notifies the parent with `null`. `useLatestRef` keeps the inline `onSelectionChange` out of effect deps. The cell publishes `rowValues(table, rowIndex)`, which is keyed by SQL column names.
  - `src/components/xychart-selection.ts` holds `nextSelection` (clicking the selected mark again clears it) and the dim constants (`DIM_FILL = 0.25`).
  - Clicking empty space clears the selection.
- **Why keys must be SQL column names**: `$cell.selected.col` resolves its value type with `fieldType(ctx.cellResults[cell], col)` (`macro-resolve.ts:86-91`). That type controls how the value is quoted or formatted in downstream SQL. `validateMacros` (`macro-substitution.ts:187-199`) lists the keys of the live selection as the "Available" columns.
- **Docs**: the Stacked Bar section is in `mkdocs/docs/web-app/notebooks/cell-types.md:709-775`. The Row Selection paragraph is in `mkdocs/docs/web-app/notebooks/variables.md:114-121` and lists "Table, chart, and map cells".
- **Precedent plan**: `tasks/completed/stacked_bar_cell_plan.md`. Its Decisions said "vertical only" and "no selection" as user calls for v1. This issue reverses both on purpose.

## Design

### Options (`QueryCellConfig.options`)
| Key | Type | Default | Meaning |
|---|---|---|---|
| `unit` | `string` | — | unchanged |
| `orientation` | `'vertical' \| 'horizontal'` | `'vertical'` | Bar direction. A missing or unknown value is treated as vertical, so existing notebooks render unchanged |
| `valueLabels` | `boolean` | `false` | Draw each bar's total at its end |
| `selectionMode` | `'none' \| 'single'` | `'none'` | Click a segment to publish it as `$cell.selected.*` |

No new config interface or `CellType` change. The cell keeps `QueryCellConfig` with `type: 'stackedbar'`.

### SQL contract: optional series column
`extractStackedBarData` accepts **2 or 3** non-color columns:

| Non-color columns | Shape | `color` column applies to |
|---|---|---|
| 3 | `category, series, value` (unchanged) | series (first non-null wins, unchanged) |
| 2 | `category, value` → one implicit series named after the value column | **each bar** (first non-null per category wins) |

With a single implicit series, a per-series color could only paint every bar the same color. A per-bar color is the one useful reading (e.g. highlight one user). The 2-column validation errors say "category and value columns (optionally category, series, value)". The rules for dropping and summing rows are unchanged.

`StackedBarData` gains:
```ts
export interface StackedBarData {
  categories: string[]
  /** First-seen raw (un-stringified) category value per category, for selection payloads. */
  categoryValues: unknown[]
  series: { name: string; color?: string }[]
  values: number[][]
  /** Per-bar SQL color, 2-column shape only; undefined entries fall back to the series color. */
  categoryColors?: (string | undefined)[]
  /** Source column names; `series` is absent for the 2-column shape. */
  columns: { category: string; series?: string; value: string }
  singleSeries: boolean
}
```
`singleSeries` is true only for the 2-column shape. A 3-column query that happens to yield one distinct series keeps its legend, so the legend doesn't appear and disappear as data changes.

### Layout: two builders, one shared output shape
The vertical builder stays. A horizontal builder is added. Both return the same shape, so segment drawing, in-segment labels, total labels, selection dimming and the tooltip are shared code. Only the axis and the category labels differ by orientation.

Shared pieces, factored out of `buildStackedBarLayout`:
- `buildValueScale(maxTotal, unit, divisions)` → `{ top, step, ticks: {value, label}[] }`. It is the current max × 1.05 and `niceStep` logic, unchanged for vertical.
- A segment-stacking loop parameterized by a value→pixel mapping and the stacking axis. It produces the existing `StackedBarSegmentLayout` (`x, y, width, height, rounded, labelFits`). In horizontal mode `labelFits` checks only the segment *width* against the text.
- `rounded` marks the end segment. The path helper becomes `roundedEndRectPath(x, y, w, h, r, end: 'top' | 'right')`.

Additions to the output:
- `orientation`
- `contentWidth` and `contentHeight`, replacing `plotWidth` and the implicit height
- `valueTicks: { value, pos, label }[]`, where `pos` is y for vertical and x for horizontal. `yTicks` is renamed to this.
- Per bar: `totalLabel: { x, y, text } | null`

**Horizontal geometry** (`buildHorizontalBarLayout`):
- `ROW_HEIGHT = 28`, `BAR_THICKNESS = 18`. The bar is centered in its row.
- `AXIS_HEIGHT = 24`. The axis is a separate sticky SVG, see Rendering.
- Label column width = `min(LABEL_COLUMN_MAX = 180, max estimated label width + 16)`. Short labels don't waste width. Longer labels go through `truncateLabel` and keep the full text in `<title>`.
- Value plot width = `Math.max(0, width − labelColumn − RIGHT_MARGIN − totalLabelRoom)`. `totalLabelRoom` = widest total label + 8 when `valueLabels` is on, else 0. The longest bar never runs under its own label.
- Divisions = `clamp(floor(plotWidth / (widestTickLabel + 24)), 2, 5)`. Tick labels sit side by side here, so a narrow cell needs fewer of them. The widest tick label is estimated from `formatValueWithUnit(top)`.
- `contentHeight = categories.length × ROW_HEIGHT + 8`. `contentWidth = width`. There is no horizontal scroll, and rows don't stretch to fill a tall cell.
- Rows go top to bottom in first-appearance order, the same order vertical uses left to right. `ORDER BY cost DESC` puts the largest bar at the top.

**Vertical `valueLabels`**: the label sits centered above the bar. `TOP_MARGIN_PX` grows by 14 when the option is on, so the tallest label isn't clipped. The label is drawn only if its estimated width fits the band. Neighbors would collide otherwise, and the tooltip still shows the total.

### Rendering (`StackedBarView`)
- Vertical: unchanged structure, with the plot div scrolling on x.
- Horizontal: the plot div is `overflow-y-auto` and holds two SVGs.
  - First, an axis SVG with `position: sticky; top: 0` and a panel background. It draws the tick labels and a baseline. Tick labels are centered on their tick, except the last, which is end-anchored (`text-anchor: end`) so it stays inside the cell when the last tick falls at or near the plot's right edge.
  - Second, a body SVG of `contentHeight`. It draws the vertical gridlines, the category labels (end-anchored, right-aligned in the label column), the segments and the labels.
  - The `ResizeObserver` reads the content-box width, which excludes the scrollbar, so both SVGs line up.
  - The height is the content height, not the measured height.
- Single series: the cell hides the legend column and the header's "series" stat. The tooltip shows the category, a swatch, the value column name and the value. It drops "share of bar" and "bar total", which add nothing for one series.
- Segment color: `categoryColors?.[c] ?? series[s].color`, resolved once per segment and used for the fill, the hover swatch and the in-segment label's `contrastingTextColor`.

### Selection
The hit is `{ categoryIndex, seriesIndex }`. `StackedBarCell` copies the Chart cell's pattern (`ChartCell.tsx:216-238`). The state, the during-render reset and the null-notify effect all live in `StackedBarCell` above its early returns, keyed on `data`. The hit and the click handlers are passed down to `StackedBarView`:
- `selected` state is reset during render when `data` changes. An effect calls `onSelectionChangeRef.current?.(null)` on data change.
- Clicking a segment calls `nextSelection` semantics: clicking the selected segment again clears it. Write a small local `sameBarHit`. `ChartPointHit` has different fields, so reuse only the toggle idea and `DIM_FILL`.
- A click on the SVG background (not a segment) clears the selection. The background handler clears only when `e.target === e.currentTarget`, so a segment click does not bubble into a clear. In horizontal mode the body SVG carries the handler, not the sticky axis SVG.
- Payload keys are the SQL column names, as in table, chart and map. The issue wrote `.category`, and a query that names its columns `category, series, value` still gets exactly the issue's macros.
- Selection is enabled only when `selectionMode === 'single'`. Otherwise clicks do nothing, as today.
- While a selection exists, every other segment and its in-segment label render at `DIM_FILL` opacity, applied with inline `style={{ opacity: DIM_FILL }}` so it wins over the segments' `hover:opacity-85` class. Total labels stay full opacity.

Published payload, keyed by **SQL column names**:
```ts
{
  [columns.category]: categoryValues[c],             // raw value, e.g. number stays number
  ...(columns.series ? { [columns.series]: series[s].name } : {}),
  [columns.value]: values[c][s],                      // the (summed) segment value
}
```
For `SELECT user, model, sum(cost) AS cost …` this gives `$cell.selected.user`, `.model` and `.cost`. For the 2-column shape the payload has only the category and the value. Every click lands on a segment, so in stacked mode the series is always present.

### Editor
`StackedBarCellEditor` gains:
- An **Orientation** select (Vertical / Horizontal). It uses the same field styling as the Unit input.
- A **Show value labels** checkbox.
- `RowSelectionEditor`, after the Unit/Orientation/value-labels fields and before the validation errors and `AvailableVariablesPanel`, as in `ChartCell`.

The SQL hint text is rewritten to describe both shapes (2 or 3 columns, and what `color` means in each). The SyntaxEditor placeholder becomes `SELECT category, [series,] value FROM ...`.

### Metadata
- `label: 'Bar Chart'`
- `description: 'Vertical or horizontal bars, one per category, optionally stacked by series'`
- Icon unchanged. The type id `stackedbar` is unchanged, since stored notebooks reference it.

## Mockups
- `tasks/stackedbar_horizontal_mockups/horizontal-layout.html` shows the horizontal layout in two states, using the app's dark theme tokens and `SERIES_COLORS`:
  - left: a single-series ranked cost per user, 30 rows, with value labels and a selected row
  - right: a stacked-by-series version with the legend and in-segment labels

  Clicking bars toggles selection, and the text below shows the published `$cell.selected.*` values.

## Implementation Steps

### Phase 1 — Extraction (`src/lib/arrow-utils.ts`)
1. Accept 2 or 3 non-color columns in `extractStackedBarData`, and update the error strings.
2. For the 2-column shape:
   - Build the one implicit series `{ name: valueField.name }`. It gets no series color.
   - Collect `categoryColors` per category (first non-null wins).
   - Validate the category and value types as today.
3. For both shapes, record `categoryValues` (first-seen raw value), `columns` and `singleSeries`.

### Phase 2 — Layout (`StackedBarCell.tsx`, pure section)
4. Extract `buildValueScale` and the segment-stacking helper from `buildStackedBarLayout`. Rename `yTicks` to `valueTicks` (`y` → `pos`) and `plotWidth` to `contentWidth`, and add `contentHeight`, `orientation` and `totalLabel`. Existing tests are updated mechanically and must keep passing with the same numbers.
5. Add the `valueLabels` option to `buildStackedBarLayout`, covering the top-margin growth and the fit check.
6. Add `buildHorizontalBarLayout`.
7. Replace `roundedTopRectPath` with `roundedEndRectPath(…, end)`.

### Phase 3 — Renderer
8. `StackedBarView`:
   - branch the axis and category-label drawing on `layout.orientation`
   - add the horizontal sticky-axis structure
   - draw the total labels
   - resolve one color per segment, `categoryColors?.[c] ?? series[s].color`, and use it for the fill, the hover/tooltip swatch and the in-segment label's `contrastingTextColor`
   - hide the legend and slim the tooltip for single series
9. Selection: in `StackedBarCell` (above its early returns), add the state, the clear-on-data-change logic and the null-notify effect, and pass the hit and the segment and background click handlers down to `StackedBarView`, which does the dimming. Add the payload builder. Make the payload builder a small exported pure function, `buildBarSelection(data, hit)`, so it is unit-tested directly.
10. `StackedBarCell`: read `orientation`, `valueLabels`, `selectionMode` and `onSelectionChange`, and hide the series stat when `singleSeries`. Build `resolvedData` by spreading `extraction.data` and overriding `series`, so `categoryColors` and the other new fields aren't dropped.

### Phase 4 — Editor and metadata
11. Add the Orientation select, the value-labels checkbox, `RowSelectionEditor` and the hint/placeholder text to the editor.
12. Change the label and description in `stackedBarMetadata`. New cells default to the name "Bar Chart" instead of "Stacked Bar", since `createDefaultCell` seeds the name from `meta.label`.

### Phase 5 — Docs and changelog
13. Update `mkdocs/docs/web-app/notebooks/cell-types.md`:
    - rename the section heading to "Bar Chart" and move it before "Chart" (:11), since the `## ` headings are alphabetical
    - add the new options rows (`orientation`, `valueLabels`, `selectionMode`)
    - document the 2-column SQL shape and the per-bar `color` meaning
    - add horizontal layout and selection bullets under Features
    - add a ranked horizontal example (`SELECT user, sum(cost) AS cost … ORDER BY cost DESC`, `orientation: horizontal`)
    - revise the existing text that no longer holds: the intro (:711, not only vertical bars stacked from series); the `sql` and Series rows (:717, :726; series is optional, 3-column shape only); the `color` row (:727, per series or per bar); "Value (3rd non-color)" (:727) becomes "Value (2nd or 3rd non-color)"; "Duplicate `(category, series)` rows are **summed**" (:734) also says duplicate categories are summed in the 2-column shape; the ordering bullet (:733) for horizontal; "Y-axis" (:740) wording; the legend bullet (:742, hidden for a single series) and tooltip bullet (:743, no share/total for a single series); the scroll bullet (:744; vertical scrolls horizontally, horizontal scrolls vertically)
    - note the cell type id is `stackedbar`
14. Update `mkdocs/docs/web-app/notebooks/variables.md:116`: add bar chart cells to the cells with selection (including the second clause, "For table and chart cells, selection is enabled via the **Row Selection** section…", and the :118 "**Selection mode** (table and chart cells)" bullet), and say a click on a segment publishes the category, series and value columns.
14a. Update `claude-plugin/skills/micromegas-notebooks/SKILL.md:64` and its identical copy `rust/public/src/servers/mcp/skills/micromegas-notebooks/SKILL.md:64`: where `$cell.selected.column` is described as the selected row of an upstream table or chart with `selectionMode: "single"`, also mention bar chart cells (and map) as selection sources.
15. `CHANGELOG.md`: add a **Notebooks** entry under Unreleased, citing "(#1820)" like the existing entries. It covers no SQL changes and no breaking change, and notes that new cells default to "Bar Chart" instead of "Stacked Bar". Notebook config is additive, and existing `stackedbar` cells render identically.

## Files to Modify
- `analytics-web-app/src/lib/arrow-utils.ts`
- `analytics-web-app/src/lib/__tests__/arrow-utils.test.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/StackedBarCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/StackedBarCell.test.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`
- `mkdocs/docs/web-app/notebooks/variables.md`
- `claude-plugin/skills/micromegas-notebooks/SKILL.md`
- `rust/public/src/servers/mcp/skills/micromegas-notebooks/SKILL.md`
- `CHANGELOG.md`

## Decisions
- The horizontal-layout mockup (`tasks/stackedbar_horizontal_mockups/horizontal-layout.html`) is accepted (user call).
- The cell type id stays `stackedbar`, and only the display label changes to "Bar Chart" (stored notebooks reference the id).
- Horizontal mode has fixed row heights and never stretches rows to fill a tall cell (per the issue). Leftover space stays empty below the last row.
- No hit-area widening for thin segments. The 18px row thickness is enough, and stacked segments in one row would make a widened area ambiguous.
- Out of scope, per the issue: a `table` data-bar column format.
- Label is "Bar Chart": it doubles as the default cell name and badge text, so it stays short.

## Documentation
- `mkdocs/docs/web-app/notebooks/cell-types.md`: the Stacked Bar section, renamed to Bar Chart and extended (step 13)
- `mkdocs/docs/web-app/notebooks/variables.md`: the Row Selection paragraph (step 14)
- `CHANGELOG.md` (step 15)

## Testing Strategy
Everything is reachable with constructed Arrow tables and props, so the coverage is Vitest unit and component tests, using the existing local `ResizeObserver` stub in `StackedBarCell.test.tsx`:
- **Extraction** (`arrow-utils.test.ts`):
  - the 2-column shape gives one implicit series named after the value column, with `singleSeries: true`
  - per-category `categoryColors`, first non-null wins
  - `categoryValues` keeps the raw numeric category
  - `columns` names for both shapes
  - errors for 1 and 4 non-color columns
  - the existing 2-column-count error test (`src/lib/__tests__/arrow-utils.test.ts:1450-1462`, category utf8, series utf8 → "category, series, and value columns") becomes a "second column must be numeric for value" test in the 2-column shape
  - update the existing empty-table test (`arrow-utils.test.ts:1502-1514`, `toEqual({ categories: [], series: [], values: [] })`) to include the new fields
  - other existing 3-column tests stay green
- **Layout** (`StackedBarCell.test.tsx`):
  - horizontal rows at `ROW_HEIGHT` intervals in first-appearance order, with `contentHeight` growing with the category count
  - label column capped at 180, with long labels truncated
  - the value scale shrinks by `totalLabelRoom` when `valueLabels` is on
  - fewer tick divisions at narrow widths
  - the last horizontal tick label is end-anchored, the others centered
  - segments stack left to right with a 2px gap, and only the end segment is rounded
  - in-segment `labelFits` uses the segment width
  - vertical `valueLabels` grows the top margin and skips labels wider than the band
  - update `StackedBarCell.test.tsx:243-246`, which expects `/category, series, and value columns/`, to match the new column-count error wording
- **Selection**:
  - `buildBarSelection` gives SQL-column-name keys, a raw category value, the series key only in the 3-column shape, and the summed value
  - component: clicking a segment calls `onSelectionChange` with the payload and dims the other segments
  - clicking it again, or clicking the background, sends `null`
  - new `data` clears the selection and sends `null`
  - with `selectionMode` unset, clicks don't call the callback
- **Renderer**:
  - single series hides the legend and the series stat, and its tooltip has no share-of-bar row
  - horizontal renders the sticky axis SVG plus a body SVG of `contentHeight`
  - total labels render when `valueLabels` is set
- **Editor and metadata**:
  - the orientation select and value-labels checkbox write their options
  - `RowSelectionEditor` writes `selectionMode`
  - the label is `'Bar Chart'`

## Manual Verification
Sticky positioning and scrollbar alignment depend on real browser layout, which jsdom doesn't do:
1. `cd analytics-web-app && yarn dev`, open a notebook, add a Bar Chart cell with `SELECT exe, count(*) AS count FROM log_entries GROUP BY exe ORDER BY count DESC`, and set Orientation to Horizontal. Expected: one row per exe, the axis stays pinned while scrolling, and the ticks line up with the gridlines whether or not the scrollbar shows.
2. Enable Row Selection, and add a downstream table cell with `SELECT '$<cell>.selected.exe' AS picked, $<cell>.selected.count AS n`. Click a bar. Expected: the table re-runs with that exe and count, the other bars fade, and clicking the same bar again shows `Select a row in "<cell>" to view results` downstream.
