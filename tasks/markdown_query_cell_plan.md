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
  to the notebook-level default when a cell has no `dataSource`; `CellEditor.tsx:141` displays
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
`effectiveMarkdownSql(config) = config.sql?.trim() ? config.sql : DEFAULT_SQL.markdown`, so saved
cells (no `sql`) and a cleared editor both run the default. No config migration.

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
- `CellEditor`'s `DataSourceField` value (`CellEditor.tsx:141`) and `HorizontalGroupCell`'s child
  editor `DataSourceField` value (`HorizontalGroupCell.tsx:356-358`, the same fallback chain) both
  need this fallback, so a saved markdown cell shows `notebook` rather than the notebook default.
  Extract that chain into a small `configuredCellDataSource(cell, notebookDataSource)` helper so
  `resolveCellDataSource` and both editors can't drift apart.

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

`useCellExecution` needs no logic change. The standard error path shows the zero-rows error
(`CellContainer.tsx:295-304`), and the result is registered under the cell name like any other
cell's, so downstream cells can use it. `canBlockDownstream` is `true`, like the other
query-backed cell types: when an upstream blocking cell fails, `executeFromCell` marks the
markdown cell `blocked` with `data: []` (`useCellExecution.ts:398-420`) instead of showing a
stale cached headline, and a markdown query failure halts downstream execution the same way.
Extra rows are ignored. On engine load failure, markdown cells behave like every other cell
(never run; the notebook banner explains).

`getRendererProps` returns `content`, `data`, `status`, `options`.

### Rendering

`MarkdownCell` derives from `data[0]`:

- `row = rowValues(table, 0)` and `columnTypes = columnTypeMap(table)`. Both helpers move from
  `components/map/overlay.ts` to `lib/arrow-utils.ts` (the map imports them from there), so the
  markdown cell doesn't import the map overlay module.
- When `data[0]` is absent (e.g. the existing tests' `data: []`), `row` and `columnTypes` are left
  `undefined`, which makes `bareColumnsFromRow: true` inert (`macro-resolve.ts:112` only reads
  `ctx.row` when it's defined), so no colors are applied either. This keeps the existing
  `MarkdownCell.test.tsx` renderer tests, which default to `data: []` with `status: 'success'`,
  valid.
- `evaluateTemplate(content, { variables, timeRange, cellResults, cellSelections, row, columnTypes, bareColumnsFromRow: true })`.
- Colors: `resolveColorColumn` gains a `name` parameter (default `'color'`, error text uses it)
  and is called for `color` and `background_color`. A new pure helper in `MarkdownCell.tsx`,
  `resolveMarkdownColors(table, row) → { color?: string; backgroundColor?: string; warnings: string[] }`,
  decodes each present column with `cellColorToCss`. A null or malformed value means no tint. An
  unsupported column type adds a warning shown in the existing `TemplateWarningBanner` and
  doesn't fail the cell.

Render gate: a `blocked` status renders nothing (its `data` is cleared to `[]` anyway). Otherwise,
evaluate the template only when `status === 'success'`, and keep the last successful
`{ text, warnings }` in `useState`, updating it during render when a fresh `status === 'success'`
evaluation differs from what's stored — the `cacheInputsKey` pattern `PerfettoExportCell.tsx:57`
uses, not a ref (reading/writing a ref during render trips `react-hooks/refs`, which
`eslint-plugin-react-hooks` 7.1.1's `reactHooks.configs.recommended` enables). While `status` is
`'loading'` or `'idle'` with `data.length > 0`, render that stored output rather than
re-evaluating: `getAvailableCellResults` (`NotebookRenderer.tsx:514-523`) only includes upstream
cells with `status === 'success'`, and `executeFromCell` resets every cell from the restart point
to `idle` up front (`useCellExecution.ts:386-397`), so while a markdown cell is idle-with-data its
`cellResults` prop lacks upstream results that were already resolved in the cached output —
re-evaluating against that stripped-down `cellResults` would flash upstream macros as unresolved.
First paint stays deferred until the cell's first successful run, as today.

DOM structure:

```
<div root  class="flex-1 [bg: rounded-sm] [fit: min-h-0 overflow-auto flex text-center]"
           style={{ backgroundColor }}>
  <div prose class={`${proseClasses(tinted)} ${fit ? 'm-auto' : ''}`} style={{ color }}>
    <TemplateWarningBanner/> <Markdown/>
  </div>
</div>
```

- The root has no padding in any mode, so existing markdown cells keep rendering flush, as
  before, and `background_color` fills the root edge to edge. When `background_color` is set the
  root also gets `rounded-sm`, so the fill's corners match the cell. Fit mode adds no padding of
  its own. The root is `flex-1` inside `CellContainer`'s flex column, so it fills the cell height
  even when the text is short. In non-fit mode it grows past
  the cell and `CellContainer` scrolls, as today. Inside a horizontal group, the same applies only
  if `HgChildPane`'s content wrapper is also a flex column: its wrapper is `flex-1 overflow-auto
  px-1 pb-1` (`HgChildPane.tsx:215`), missing the `flex flex-col` that `CellContainer.tsx:461` has,
  so the markdown root's `flex-1` has no effect there and Fit to cell has no definite height to
  fit against. Add `flex flex-col` to that wrapper to match `CellContainer`.
- `proseClasses(tinted)`: when `color` is present, the per-element color modifiers for headings,
  p, strong, li, em, blockquote, th/td and list markers become `text-inherit` (`marker:` too), so
  the inline `color` on the prose div applies. Links and inline code keep their accent colors.
- `useFitFontSize` writes the fitted size directly to `prose.style.fontSize` (see Fit to cell), which
  overrides typography's `font-size: 1rem`. All prose typography is `em`-relative, so everything
  scales and still wraps. Typography already zeroes the first child's top margin and the last
  child's bottom margin.

### Fit to cell

A `useFitFontSize(rootRef, proseRef, enabled, deps)` hook in `MarkdownCell.tsx`:

- A `ResizeObserver` on the root, attached only while `enabled` is true and disconnected when it
  turns false (jsdom has no `ResizeObserver` and `src/test-setup.ts` doesn't stub one, so
  non-fit tests never construct it), stores its content-box size in state. A `useLayoutEffect`
  keyed on size, rendered text, and colors runs the search synchronously before paint.
- Search: the pure helper `fitFontSize(fits: (px: number) => boolean, min, max): number`, a
  binary search over integer px in `[MIN_FIT_FONT_PX = 12, MAX_FIT_FONT_PX = 320]`.
  `fits(px)` sets `prose.style.fontSize` and compares the prose element's
  `scrollWidth`/`scrollHeight` against the root's content-box width/height. It takes about 9
  layout passes per fit, only on resize or content change. If `min` doesn't fit, it returns `min`
  and the root scrolls.
- After the search, the hook writes the chosen px directly to `prose.style.fontSize` — not to
  React state, which would trip `react-hooks/set-state-in-effect` — and clears it when `enabled`
  is false, the same direct-write pattern the mockup's `fit()` uses (`stat-tiles.html:76`).

### Editor

`MarkdownCellEditor` shows, in order:

1. SQL `SyntaxEditor`, value `sql ?? DEFAULT_SQL.markdown`, same as other query cells with
   `DocumentationLink`.
2. Markdown content editor (unchanged).
3. **Fit to cell** checkbox (`options.fit`).
4. Validation errors, then `AvailableVariablesPanel`.

Validation covers both the SQL and the template, as `MapCell`'s editor does for its query and
detail template: `validateMacros(sql, variables, cellResults, cellSelections)` for the SQL editor
(`MapCell.tsx:792-793`, `TableCell.tsx:238`, `StackedBarCell.tsx:550`), shown alongside the content
errors.

The content validation needs the map cell's placeholder trick for bare columns. Extract it into
`validateTemplateMacros(text, availableColumns, variables, cellResults, cellSelections)` in
`macro-substitution.ts` and use it from both `MapCell` and `MarkdownCellEditor`. `CellEditor`
already passes `availableColumns` from the cell's last result.

## Mockups

- `tasks/markdown_query_cell_mockups/stat-tiles.html`: a horizontal group of four fitted markdown
  tiles (background fill, text tint, status string, untinted), one resizable tile running the
  real binary-search fit on resize, and an unfitted documentation cell showing the default look,
  flush with no added padding, as before. One direction only, since the issue settles the
  interaction model.
  - The mockup's `.md-root` uses a flat 12px padding for every tile so the tiles read consistently
    at a glance; that padding is illustrative only. The implementation uses no padding.

## Implementation Steps

1. **Shared helpers**
   - Move `rowValues` / `columnTypeMap` to `lib/arrow-utils.ts`; update imports in `MapCell.tsx`,
     `cells/__tests__/MapCell.test.tsx`, and `components/map/__tests__/EventDetailPanel.test.tsx`
     (currently `from '@/components/map/overlay'` / `from '../overlay'`). Move the
     `describe('rowValues')` and `describe('columnTypeMap')` blocks from `MapCell.test.tsx`
     (`:392-450`) to `lib/__tests__/arrow-utils.test.ts`.
   - Add the `name` parameter to `resolveColorColumn`.
   - Add `validateTemplateMacros` to `macro-substitution.ts` (re-export via `notebook-utils` like
     `validateMacros`); switch `MapCell`'s editor to it.
2. **Types and defaults**
   - `MarkdownCellConfig` per Design. Add `DEFAULT_SQL.markdown` and
     `cellTypeDefaultDataSource` / `configuredCellDataSource` in `notebook-utils.ts`.
   - Update `resolveCellDataSource`, `shouldShowDataSource`, `shouldShowTimeRange`,
     `createDefaultCell`, and the `CellEditor` and `HorizontalGroupCell` child-editor data-source
     values.
3. **Run control cleanup**
   - Remove `canRun` / `cellCanRun` from `cell-registry.ts` and its four call sites. Update the
     `cell-registry-mock.ts` markdown entry (give it an `execute`, drop the `canRun` fallback,
     flip `canBlockDownstream` to `true`, update its `createDefaultConfig` to include
     `sql: DEFAULT_SQL.markdown`, and update its mock renderer's gate from `status === 'success'`
     to also cover `loading`/`idle` with data, matching the real render gate).
   - Update the `useCellManager.ts` comment.
   - Update stale comments that predate query-backed markdown: `cell-registry.ts:16`
     (`CellRendererProps.sql` "undefined for markdown cells"), `cell-registry.ts:161` (`execute`
     doc "e.g., markdown"), `useCellExecution.ts:141` ("e.g., markdown"), `notebook-utils.ts:167`
     (`shouldShowDataSource` docstring "Markdown cells have no queries"), and
     `CellContainer.tsx:53` (`canRun` prop doc referencing the type's `canRun`).
4. **MarkdownCell**
   - `effectiveMarkdownSql`, `execute`, `getRendererProps`, `createDefaultConfig`
     (`sql: DEFAULT_SQL.markdown`, content).
   - Renderer: row binding, `resolveMarkdownColors`, `proseClasses`, root/prose structure,
     loading gate, `fitFontSize` + `useFitFontSize`.
   - Editor: SQL editor, fit checkbox, `validateTemplateMacros`.
   - `HgChildPane.tsx`: add `flex flex-col` to the content wrapper so Fit to cell and
     `background_color` fill work for a markdown cell inside a horizontal group.
5. **Docs and changelog** (see Documentation).

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts`
- `analytics-web-app/src/lib/screen-renderers/macro-substitution.ts`
- `analytics-web-app/src/lib/screen-renderers/useCellManager.ts` (comment)
- `analytics-web-app/src/lib/screen-renderers/useCellExecution.ts` (comment only)
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
  `__tests__/macro-substitution.test.ts`, `cells/__tests__/HorizontalGroupCell.test.tsx`,
  `components/__tests__/CellContainer.test.tsx`, `cells/__tests__/MapCell.test.tsx`,
  `components/map/__tests__/EventDetailPanel.test.tsx`, `lib/__tests__/arrow-utils.test.ts`
- `mkdocs/docs/web-app/notebooks/cell-types.md`, `mkdocs/docs/web-app/notebooks/execution.md`,
  `mkdocs/docs/web-app/notebooks/index.md`, `mkdocs/docs/web-app/notebooks/variables.md`
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
  so text couldn't wrap to use a tall, narrow cell. Changing the prose element's `font-size`
  reflows the `em`-relative prose. Container-query units (`cqi`) can't target "largest size whose wrapped
  height fits".
- **Unsupported color type: warning vs. cell error.** Chart cells fail on a bad `color` type
  because it breaks their marks. Here the text is still meaningful without the tint, so a
  warning banner is enough.

## Decisions

- `background_color` fills the MarkdownCell root edge to edge. `CellContainer`'s 4px
  `px-1 pb-1` gutter stays the panel color, which reads as a tile, and changing it would touch
  every cell type.
- Fitted content is centered on both axes and `text-align: center` (the issue's "centered when
  fitting"). Non-fit rendering keeps today's left-aligned layout.
- Previous output stays rendered while idle/loading with data (see Rendering).
- Markdown's Run button now runs its own query (still one cell, no downstream re-run). The old
  "local re-render only" semantics are replaced.
- A markdown cell with data gets `buildStatusText`'s row/elapsed status text and a header
  "Download CSV" item like any other query-backed cell, including the `SELECT 1` default.
- Root adds no padding in any mode (so `background_color` fills it edge to edge): notebooks
  double as dashboards, where padding wastes space. Pixel-identical rendering is otherwise not
  a goal.
- Markdown sets `canBlockDownstream: true`, like other query-backed cell types, so it never shows
  stale output after an upstream failure.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md`, Markdown section: rewrite the description to
  "documentation and headline values". Add a config table (`content`, `sql`, `dataSource`,
  `timeRange`, `options.fit`), explain row-0 binding (bare `$col`, columns win clashes,
  zero rows = error, extra rows ignored), the `color` / `background_color` columns (accepted
  types, same as the chart cells' color column), `format_value` for units, and Fit to cell.
  Add the issue's frame-time example and a `CASE`-based status example. Replace the "does not
  execute queries" bullet. Rewrite the "On initial load ..." bullet (`:474`): blank until the
  first successful run; previous output stays while idle/loading during a re-run; **Run** executes
  the cell's query (not a local re-render).
- `markdownMetadata.description` (`MarkdownCell.tsx:85`, shown in the add-cell modal) and the
  mock's `BASE_METADATA.markdown.description` in `__test-utils__/cell-registry-mock.ts` change
  from "Documentation and notes" to match the cell-types.md wording, "Documentation and headline
  values".
- `execution.md:37`: drop "(markdown cells do not)" from the auto-run sentence.
- `index.md:40`: markdown cells now have data, so drop them from the "hidden for cells with no data"
  example.
- `variables.md:137`: add markdown to the list of query-backed cell types that accept
  `timeRange`.
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
    Renders nothing before the first success, and keeps the previous output while idle or loading
    with data. Idle-with-data case: an upstream `$cell[0].col` macro whose cell is absent from
    `cellResults` (as `getAvailableCellResults` would produce while idle) still shows the
    resolved value from the cached evaluation, not a re-evaluated/unresolved macro.
  - `resolveMarkdownColors`: integer, `#rrggbb` string, `#rrggbbaa` string, 4-byte binary,
    null → absent, malformed string → absent, unsupported type → warning; `color` and
    `background_color` are independent.
  - Rendered DOM: `color` sets the inline color on the prose element and switches element
    classes to `text-inherit`. `background_color` sets the root background. With no color
    columns, today's classes and no inline styles.
  - `fitFontSize`: returns the largest fitting px for a threshold predicate. Returns `min` when
    nothing fits, and `max` when everything fits. Monotonic predicate across a range of
    thresholds.
  - Metadata: `createDefaultConfig` includes `sql: 'SELECT 1'`, and `execute` is defined. Replaces
    the old "declares `canRun: true` ... no `execute`" assertion.
  - Editor: bare-column macros present in `availableColumns` aren't flagged. Toggling Fit writes
    `options.fit`. SQL editor shows `SELECT 1` when `sql` is absent.
  - Removes the old "should not render content when status is loading" case (superseded by the
    idle/loading-with-data renderer test above).
- **`HorizontalGroupCell.test.tsx`**: replace "DataSourceField not shown for markdown type" with
  its inverse — the field is shown for a markdown child, since `shouldShowDataSource` no longer
  excludes markdown. Remove "shows the Run button for a markdown child ... (canRun fallback via
  metadata)"; markdown's Run button now comes from `meta.execute` like every other type, already
  covered by the existing non-markdown Run-button tests.
- **`CellContainer.test.tsx`**: remove "should show run button for a type with no execute but
  canRun: true (e.g. markdown)"; the metadata-driven fallback it exercised no longer exists.
- **`notebook-utils.test.ts`**: `resolveCellDataSource` returns `notebook` for a markdown cell
  without `dataSource` even with a remote notebook default, and honors an explicit markdown
  `dataSource`. `cellTypeDefaultDataSource('markdown')` returns `'notebook'`.
  `shouldShowDataSource('markdown')` is true. `shouldShowTimeRange` is false for markdown on
  `notebook` and true on a remote source (replaces the "markdown stays false" tests).
  `configuredCellDataSource` returns `notebook` for a markdown cell with no `dataSource` under a
  remote notebook default. The existing `:822-825` "should not have sql property" assertion on
  `createDefaultCell('markdown')` flips to assert `sql === 'SELECT 1'`.
- **`macro-substitution.test.ts`**: `validateTemplateMacros` accepts listed columns and still
  flags unknown ones.
- **arrow-utils tests**: `resolveColorColumn` with a custom name, including the error message;
  `rowValues` / `columnTypeMap` move with their existing coverage (the `describe('rowValues')` /
  `describe('columnTypeMap')` blocks from `MapCell.test.tsx`).
- **`useCellExecution.test.ts`**: replace "markdown immediately succeeds without SQL" with a
  routing-only test: a markdown cell with no `dataSource`, under a remote notebook default, is
  routed to `engine.execute_and_register` and not to `fetchQueryIPC`/`streamQuery`. The mock's
  `createSqlExecute` skips `runQuery` entirely when `config.sql` is absent, so give the mock's
  markdown entry its own `execute` that calls `runQuery` regardless. The `SELECT 1` fallback and
  the zero-rows error belong to `MarkdownCell.test.tsx`'s `execute` tests, not here. The existing
  test at `:726` ("should not mark markdown cells as blocked") is inverted to assert that an
  upstream failure blocks a downstream markdown cell (`status: 'blocked'`, `data: []`); a new
  test asserts that a failing markdown cell halts and blocks its own downstream cells, matching
  table/chart.
- **`NotebookRenderer.test.tsx`**: update the markdown run-control tests. Markdown now shows
  "Run from here" / "Auto-run from here", and its Run executes only its own query.

## Manual Verification

Layout measurement (`scrollWidth`/`scrollHeight`, `ResizeObserver`) and the rendered prose
typography don't exist in jsdom. Broken fitting or tinting is immediately obvious on screen, so
these checks are manual:

1. `python3 local_test_env/ai_scripts/start_services.py --monolith`, then open
   http://127.0.0.1:3000, create a notebook.
2. Open an existing notebook with markdown cells. Expected: they render the same content flush,
   with no added root padding, now with the row/elapsed status text and a "Download CSV" header
   item (see Decisions).
3. Add a markdown cell with the issue's frame-time example (data source switched to the remote
   source) and Fit on. Expected: the value fills the tile. Resizing the cell height and putting
   it in a horizontal group re-fits it, and text wraps in a narrow tile.
4. Add `color` / `background_color` columns. Expected: headings, paragraphs and bold text take
   the tint, and the background fills the tile edge to edge inside the 4px gutter.

## Open Questions

None. The issue settles the behavior; the Decisions above record the remaining layout calls.
