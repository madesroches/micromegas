# Markdown Cell: Query-Backed Templates for Headline Values Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1509

## Overview

Dashboards have no good way to show a single headline value ("p99 latency", "active players",
"healthy"/"degraded"), the need Grafana's Stat panel covers. Rather than adding a new cell type,
this plan makes the markdown cell query-backed: every markdown cell runs a query (default
`SELECT 1` on the local `notebook` WASM source), row 0 of the result binds to the template as
bare `$col`, optional `color` / `background_color` columns tint the text and fill the cell, and an
optional **Fit to cell** mode scales the rendered markdown to fill the cell. Thresholds, value
mappings and gradients are written in SQL; units reuse `format_value`.

## Current State

- **`MarkdownCell.tsx`** (`analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`)
  - Renderer (`:16-31`) evaluates `content` with `evaluateTemplate` only when
    `status === 'success'`, inside a `prose` div whose per-element colors are fixed classes
    (`prose-headings:text-theme-text-primary prose-p:text-theme-text-secondary ...`).
  - Metadata (`:81-100`) has no `execute`, sets `canRun: true` (the only cell type that uses
    it), `canBlockDownstream: false`, and `createDefaultConfig` returns only `content`.
  - Editor (`:37-75`) shows the content `SyntaxEditor` and validates macros with
    `validateMacros`.
- **`notebook-types.ts:128-131`** — `MarkdownCellConfig` extends only `CellConfigBase`
  (`content`), not `QueryBackedCellConfig`.
- **Markdown special cases elsewhere:**
  - `cell-registry.ts:298` — `createDefaultCell` skips the default data source for markdown.
  - `notebook-utils.ts:170-172` — `shouldShowDataSource` excludes markdown.
  - `notebook-utils.ts:415-419` — `shouldShowTimeRange` returns false for markdown.
  - `useCellExecution.ts:140-144` — cells without `execute` complete as `success` with no data.
  - `cell-registry.ts:139-146, 260-262` — `canRun` / `cellCanRun` exist solely for markdown.
  - `NotebookRenderer.tsx:712` — the auto-run toggle is gated on `meta.execute`.
  - `useCellManager.ts:228-232` — comment says `content` is markdown-only and presentation-only.
- **Data source resolution** — `resolveCellDataSource` (`notebook-utils.ts:338-349`) falls back
  to the notebook-level default when a cell has no `dataSource`; `CellEditor.tsx:137` displays
  the same fallback.
- **Row-bound templates already exist in the map cell.** `EventDetailContent.tsx:64-78` calls
  `evaluateTemplate` with `row`, `columnTypes` and `bareColumnsFromRow: true`
  (`macro-resolve.ts:109-121`: columns win over variables, keep their Arrow type so timestamps
  render RFC3339 and `format_value` gets the raw value). The row/type helpers `rowValues` and
  `columnTypeMap` live in `components/map/overlay.ts:522-536`. `MapCell.tsx:803-813` avoids
  "Unknown variable" edit-time errors for bare columns by merging empty placeholders for
  `availableColumns` into the variables passed to `validateMacros`.
- **Color column convention** — `resolveColorColumn` (`lib/arrow-utils.ts:245-266`) finds the
  field named `color` (case-insensitive) and classifies it integer/string/binary;
  `cellColorToCss` (`lib/color-utils.ts:97-114`) decodes one value to `#rrggbbaa` (null when
  malformed).
- **Layout** — `CellContainer.tsx:461` wraps the renderer in a `px-1 pb-1 flex flex-col`
  div with a fixed height and `overflow: auto`. `StackedBarCell.tsx:263-277` is the
  `ResizeObserver` precedent.
- **Engine readiness** — notebook execution starts only once the WASM engine is loaded
  (`useCellExecution.ts:426-437`); a load failure shows a notebook-level banner
  (`NotebookRenderer.tsx:748-750`) and no cell executes.

## Design

### Config type

```ts
export interface MarkdownCellConfig extends CellConfigBase, QueryBackedCellConfig {
  type: 'markdown'
  content: string
  /** Optional for saved cells predating query-backed markdown; see DEFAULT_SQL.markdown. */
  sql?: string
  options?: { fit?: boolean }
}
```

`DEFAULT_SQL.markdown = 'SELECT 1'` in `notebook-utils.ts`. The query executed is
`config.sql?.trim() ? config.sql : DEFAULT_SQL.markdown`, so saved cells (no `sql`) and a
cleared editor both run the default. No config migration.

### Per-type default data source

Saved markdown cells have no `dataSource`, and must run on `notebook`, not on the notebook-level
default. One table in `notebook-utils.ts` states this:

```ts
const CELL_TYPE_DEFAULT_DATA_SOURCE: Partial<Record<CellType, string>> = { markdown: 'notebook' }
export function cellTypeDefaultDataSource(type: CellType): string | undefined
```

(It lives in `notebook-utils`, not on `CellTypeMetadata`, because `notebook-utils` can't import
the registry without an import cycle through the cell modules.) Consumers:

- `resolveCellDataSource`: `cell.dataSource || cellTypeDefaultDataSource(cell.type) || notebookDataSource`
  (the `$var`-unresolved fallback is unchanged).
- `createDefaultCell`: `cellTypeDefaultDataSource(type) ?? defaultDataSource`, still skipped for
  `referencetable` / `hg`. The `type !== 'markdown'` clause goes away, and new markdown cells
  persist `dataSource: 'notebook'` explicitly.
- `CellEditor`'s `DataSourceField` value uses the same fallback chain, so a saved markdown cell
  shows `notebook` rather than the notebook default. Extract that chain into a small
  `configuredCellDataSource(cell, notebookDataSource)` helper so `resolveCellDataSource` and
  the editor can't drift apart.

Because `shouldShowTimeRange` goes through `resolveCellDataSource`, a markdown cell on `notebook`
hides the time-range field automatically, and one switched to a remote source shows it.

### Removing the special cases

- `shouldShowDataSource`: drop `markdown`.
- `shouldShowTimeRange`: drop the `case 'markdown'` (falls to the data-source check).
- `markdownMetadata`: add `execute`, remove `canRun: true`. `canRun` then has no users, so remove
  the `canRun` field and `cellCanRun` (callers in `CellEditor`, `CellContainer`, `HgChildPane`,
  `HorizontalGroupCell` go back to `!!meta.execute`). Auto-run / "Run from here" now appear for
  markdown through the existing `meta.execute` gates.
- `useCellManager.ts` comment: `content` stays in `nonExecKeys` (still presentation-only), but the
  "no query-backed cell type has it" wording is updated.

### Execution

```ts
execute: async (config, { variables, cellResults, cellSelections, timeRange, runQuery }) => {
  const md = config as MarkdownCellConfig
  const sql = substituteMacros(effectiveMarkdownSql(md), variables, timeRange, cellResults, cellSelections)
  const table = await runQuery(sql)
  if (table.numRows === 0) throw new Error('Query returned no rows')
  return { data: [table] }
}
```

`useCellExecution` needs no change. The standard error path shows the zero-rows error
(`CellContainer.tsx:295-304`), and the result is registered under the cell name like any other
cell's, so downstream cells can use it. `canBlockDownstream` stays `false`. Extra rows are
ignored. On engine load failure, markdown cells behave like every other cell (never run; the
notebook banner explains).

`getRendererProps` returns `content`, `data`, `status`, `options`.

### Rendering

`MarkdownCell` derives from `data[0]`:

- `row = rowValues(table, 0)` and `columnTypes = columnTypeMap(table)`. Both helpers move from
  `components/map/overlay.ts` to `lib/arrow-utils.ts` (the map imports them from there), so the
  markdown cell doesn't import the map overlay module.
- `evaluateTemplate(content, { variables, timeRange, cellResults, cellSelections, row, columnTypes, bareColumnsFromRow: true })`.
- Colors: `resolveColorColumn` gains a `name` parameter (default `'color'`, error text uses it)
  and is called for `color` and `background_color`. A new pure helper in `MarkdownCell.tsx`,
  `resolveMarkdownColors(table, row) → { color?: string; backgroundColor?: string; warnings: string[] }`,
  decodes each present column with `cellColorToCss`. A null or malformed value means no tint. An
  unsupported column type adds a warning shown in the existing `TemplateWarningBanner` and
  doesn't fail the cell.

Render gate: render when `status === 'success'`, or `status === 'loading'` with `data.length > 0`
(the previous result is kept while a re-run is in flight, so a remote-backed stat doesn't blank on
every refresh). First paint stays deferred until the cell's first successful run, as today.

DOM structure:

```
<div root  class="flex-1 rounded-sm p-3 [fit: min-h-0 overflow-auto flex items-center justify-center text-center]"
           style={{ backgroundColor }}>
  <div prose class={proseClasses(tinted)} style={{ color, fontSize: fit ? `${px}px` : undefined }}>
    <TemplateWarningBanner/> <Markdown/>
  </div>
</div>
```

- The root owns padding and background, so `background_color` fills the whole content area
  including the padding around fitted content. It is `flex-1` inside `CellContainer`'s flex
  column, so it fills the cell height even when the text is short. In non-fit mode it grows past
  the cell and `CellContainer` scrolls, as today.
- `proseClasses(tinted)`: when `color` is present, the per-element color modifiers for headings,
  p, strong, li, em, blockquote, th/td and list markers become `text-inherit` (`marker:` too), so
  the inline `color` on the prose div applies. Links and inline code keep their accent colors.
- Setting an inline `font-size` on the prose element overrides typography's `font-size: 1rem`.
  All prose typography is `em`-relative, so everything scales and still wraps.
  Typography already zeroes the first child's top margin and the last child's bottom margin.

### Fit to cell

A `useFitFontSize(rootRef, proseRef, enabled, deps)` hook in `MarkdownCell.tsx`:

- A `ResizeObserver` on the root stores its content-box size in state. A `useLayoutEffect`
  keyed on size, rendered text, and colors runs the search synchronously before paint.
- Search: the pure helper `fitFontSize(fits: (px: number) => boolean, min, max): number`, a
  binary search over integer px in `[MIN_FIT_FONT_PX = 12, MAX_FIT_FONT_PX = 320]`.
  `fits(px)` sets `prose.style.fontSize` and checks
  `scrollWidth <= width && scrollHeight <= height`. It takes about 9 layout passes per fit, only
  on resize or content change. If `min` doesn't fit, it returns `min` and the root scrolls.
- When `enabled` is false, the inline font size is cleared.

### Editor

`MarkdownCellEditor` shows, in order:

1. SQL `SyntaxEditor`, value `sql ?? DEFAULT_SQL.markdown`, same as other query cells with
   `DocumentationLink`.
2. Markdown content editor (unchanged).
3. **Fit to cell** checkbox (`options.fit`).
4. Validation errors, then `AvailableVariablesPanel`.

Validation needs the map cell's placeholder trick for bare columns. Extract it into
`validateTemplateMacros(text, availableColumns, variables, cellResults, cellSelections)` in
`macro-substitution.ts` and use it from both `MapCell` and `MarkdownCellEditor`. `CellEditor`
already passes `availableColumns` from the cell's last result.

## Mockups

- `tasks/markdown_query_cell_mockups/stat-tiles.html`: a horizontal group of four fitted markdown
  tiles (background fill, text tint, status string, untinted), one resizable tile running the
  real binary-search fit on resize, and an unfitted documentation cell showing the default look is
  unchanged. One direction only, since the issue settles the interaction model.

## Implementation Steps

1. **Shared helpers**
   - Move `rowValues` / `columnTypeMap` to `lib/arrow-utils.ts`; update `MapCell.tsx` imports.
   - Add the `name` parameter to `resolveColorColumn`.
   - Add `validateTemplateMacros` to `macro-substitution.ts` (re-export via `notebook-utils` like
     `validateMacros`); switch `MapCell`'s editor to it.
2. **Types and defaults**
   - `MarkdownCellConfig` per Design. Add `DEFAULT_SQL.markdown` and
     `cellTypeDefaultDataSource` / `configuredCellDataSource` in `notebook-utils.ts`.
   - Update `resolveCellDataSource`, `shouldShowDataSource`, `shouldShowTimeRange`,
     `createDefaultCell`, and `CellEditor`'s data-source value.
3. **Run control cleanup**
   - Remove `canRun` / `cellCanRun` from `cell-registry.ts` and its four call sites. Update the
     `cell-registry-mock.ts` markdown entry (give it an `execute`, drop the `canRun` fallback).
   - Update the `useCellManager.ts` comment.
4. **MarkdownCell**
   - `execute`, `getRendererProps`, `createDefaultConfig` (`sql: DEFAULT_SQL.markdown`, content).
   - Renderer: row binding, `resolveMarkdownColors`, `proseClasses`, root/prose structure,
     loading gate, `fitFontSize` + `useFitFontSize`.
   - Editor: SQL editor, fit checkbox, `validateTemplateMacros`.
5. **Docs and changelog** (see Documentation).

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
- `analytics-web-app/src/lib/screen-renderers/macro-substitution.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellManager.ts` (comment)
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/HgChildPane.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`
- `analytics-web-app/src/components/CellEditor.tsx`
- `analytics-web-app/src/components/CellContainer.tsx`
- `analytics-web-app/src/components/map/overlay.ts`
- `analytics-web-app/src/lib/arrow-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/__test-utils__/cell-registry-mock.ts`
- Tests: `cells/__tests__/MarkdownCell.test.tsx`, `__tests__/notebook-utils.test.ts`,
  `__tests__/useCellExecution.test.ts`, `__tests__/NotebookRenderer.test.tsx`,
  `__tests__/macro-substitution.test.ts`, plus the arrow-utils test file
- `mkdocs/docs/web-app/notebooks/cell-types.md`, `mkdocs/docs/web-app/notebooks/execution.md`,
  `mkdocs/docs/web-app/notebooks/index.md`
- `CHANGELOG.md`

## Trade-offs

- **Extend markdown vs. a new `stat` cell type.** A new type would duplicate the template engine,
  editor and prose rendering. It would also need its own threshold/mapping UI, which SQL
  already expresses. Extending markdown means one feature serves both notes and stats, and the
  `SELECT 1` default costs nothing, because the WASM engine is loaded for every notebook anyway.
- **Colors from SQL columns vs. a thresholds/mappings editor.** SQL `CASE` / `color_scale` /
  `lerp_color` already cover steps, gradients and value mappings. This reuses the `color` column
  convention every chart-shaped cell uses, with no new UI.
- **Per-type default data source table vs. load-time config normalization.** Normalizing saved
  configs would be a migration, and it would dirty every notebook on open. A one-entry table
  read by `resolveCellDataSource` keeps saved configs as they are.
- **Binary search on `font-size` vs. CSS `transform: scale`.** Scaling a transform doesn't reflow,
  so text couldn't wrap to use a tall, narrow cell. Changing the root `font-size` reflows the
  `em`-relative prose. Container-query units (`cqi`) can't target "largest size whose wrapped
  height fits".
- **Unsupported color type: warning vs. cell error.** Chart cells fail on a bad `color` type
  because it breaks their marks. Here the text is still meaningful without the tint, so a
  warning banner is enough.

## Decisions

- `background_color` fills the MarkdownCell root, including its own padding. `CellContainer`'s 4px
  `px-1 pb-1` gutter stays the panel color, which reads as a tile, and changing it would touch
  every cell type.
- Fitted content is centered on both axes and `text-align: center` (the issue's "centered when
  fitting"). Non-fit rendering keeps today's left-aligned layout.
- The previous result stays rendered while a re-run is loading. The first render is still deferred
  to the first successful run.
- Markdown's Run button now runs its own query (still one cell, no downstream re-run). The old
  "local re-render only" semantics are replaced.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md`, Markdown section: rewrite the description to
  "documentation and headline values". Add a config table (`content`, `sql`, `dataSource`,
  `timeRange`, `options.fit`), explain row-0 binding (bare `$col`, columns win clashes,
  zero rows = error, extra rows ignored), the `color` / `background_color` columns (accepted
  types, same as the chart cells' color column), `format_value` for units, and Fit to cell.
  Add the issue's frame-time example and a `CASE`-based status example. Replace the "does not
  execute queries" bullet.
- `execution.md:37`: drop "(markdown cells do not)" from the auto-run sentence.
- `index.md:40`: markdown cells now have data, so drop them from the "hidden for cells with no data"
  example.
- `CHANGELOG.md` (Unreleased): feature entry, noting that markdown cells now run a query and
  depend on the WASM engine. Removing `canRun` / `cellCanRun` is internal web-app code, so it
  gets no breaking-change clause.

## Testing Strategy

Unit tests (Vitest, jsdom, no services):

- **`MarkdownCell.test.tsx`**
  - `execute`: runs `config.sql` after macro substitution. Runs `SELECT 1` when `sql` is absent
    or blank. Throws on a zero-row result. Returns the table when there are several rows.
  - Renderer: bare `$col` resolves from row 0 and wins over a same-named variable.
    `format_value($value, "milliseconds")` formats the raw value. Rows past 0 are ignored.
    Renders nothing before the first success, and keeps the previous output while `loading` with
    data.
  - `resolveMarkdownColors`: integer, `#rrggbb` string, `#rrggbbaa` string, 4-byte binary,
    null → absent, malformed string → absent, unsupported type → warning; `color` and
    `background_color` are independent.
  - Rendered DOM: `color` sets the inline color on the prose element and switches element
    classes to `text-inherit`. `background_color` sets the root background. With no color
    columns, today's classes and no inline styles.
  - `fitFontSize`: returns the largest fitting px for a threshold predicate. Returns `min` when
    nothing fits, and `max` when everything fits. Monotonic predicate across a range of
    thresholds.
  - Metadata: `createDefaultConfig` includes `sql: 'SELECT 1'`, and `execute` is defined.
  - Editor: bare-column macros present in `availableColumns` aren't flagged. Toggling Fit writes
    `options.fit`.
- **`notebook-utils.test.ts`**: `resolveCellDataSource` returns `notebook` for a markdown cell
  without `dataSource` even with a remote notebook default, and honors an explicit markdown
  `dataSource`. `createDefaultCell('markdown', …, 'remote')` gets `dataSource: 'notebook'`.
  `shouldShowDataSource('markdown')` is true. `shouldShowTimeRange` is false for markdown on
  `notebook` and true on a remote source (replaces the "markdown stays false" tests).
- **`macro-substitution.test.ts`**: `validateTemplateMacros` accepts listed columns and still
  flags unknown ones.
- **arrow-utils tests**: `resolveColorColumn` with a custom name, including the error message;
  `rowValues` / `columnTypeMap` move with their existing coverage, if any.
- **`useCellExecution.test.ts`**: replace "markdown immediately succeeds without SQL" with a
  markdown cell executing through the notebook engine mock (legacy config without `sql` /
  `dataSource` runs `SELECT 1` via `execute_and_register`, with no remote fetch). Zero rows
  → `error` status, and downstream execution continues (`canBlockDownstream: false`).
- **`NotebookRenderer.test.tsx`**: update the markdown run-control tests. Markdown now shows
  "Run from here" / "Auto-run from here", and its Run executes only its own query.

## Manual Verification

Layout measurement (`scrollWidth`/`scrollHeight`, `ResizeObserver`) and the rendered prose
typography don't exist in jsdom. Broken fitting or tinting is immediately obvious on screen, so
these checks are manual:

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`, then open
   http://127.0.0.1:3000, create a notebook.
2. Open an existing notebook with markdown cells. Expected: they render as before, and the editor
   shows data source `notebook` with query `SELECT 1`.
3. Add a markdown cell with the issue's frame-time example (data source switched to the remote
   source) and Fit on. Expected: the value fills the tile. Resizing the cell height and putting
   it in a horizontal group re-fits it, and text wraps in a narrow tile.
4. Add `color` / `background_color` columns. Expected: headings, paragraphs and bold text take
   the tint, and the background fills the tile edge to edge inside the 4px gutter.
5. Change the query to return zero rows. Expected: the "Query returned no rows" error state, and
   cells below still run.

## Open Questions

None. The issue settles the behavior; the Decisions above record the remaining layout calls.
