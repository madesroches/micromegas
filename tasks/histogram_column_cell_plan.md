# Histogram Column Cell Plan

**GitHub Issue**: [#1512](https://github.com/madesroches/micromegas/issues/1512)

## Overview

Add default inline-histogram rendering for `Table` and `Transposed Table` cells: any
column whose result value is a `Histogram` struct (the type already produced by the
`make_histogram()` SQL aggregate) renders as a small per-row bar chart — bucket bars
shaped to the row's own distribution, with a gold tick mark and trailing label
showing the estimated median — instead of the raw struct. No new SQL, no new
per-column config is required to turn this on: it activates automatically from the
column's Arrow type, the same way `Chart` and `Map` cells already treat a `color`
column specially by type. Hovering a bucket shows its range and count/frequency in a
tooltip. Bar color defaults to the flat brand color used elsewhere, with an optional
per-column override to a named perceptual colormap or a custom color gradient,
driven by each bucket's own height ratio.

The existing "Overrides" panel gains exactly one new idea: a histogram-typed column's
override card gets a **Render as: Markdown / Histogram** toggle — Markdown is
today's only option (a format template); Histogram exposes the color mode above. No
separate mechanism is needed to inspect the raw struct for debugging, either — since
a histogram-typed value now formats as its raw fields wherever a markdown template
references it (`$row.duration_dist`), "debug view" is just a Markdown override
pointed at that column, no dedicated toggle required.

## Current State

### The SQL layer already has everything the visualization needs

`rust/datafusion-extensions/src/histogram/` implements a complete histogram pipeline,
already documented in `mkdocs/docs/query-guide/functions-reference.md:925-1120`
("Histogram Functions"):

- `make_histogram(start, end, bins, values)` — an aggregate UDF
  (`histogram_udaf.rs:220-235`) that reduces a column of numeric samples into one
  `Histogram` struct value per group.
- The `Histogram` Arrow type is a `Struct` with exactly these 8 fields, in this order
  (`histogram/accumulator.rs:329-344`, `state_arrow_fields()`):
  ```
  start: Float64, end: Float64, min: Float64, max: Float64,
  sum: Float64, sum_sq: Float64, count: UInt64, bins: List<UInt64>
  ```
  `bins[i]` is the sample count in bucket `i`; bucket `i`'s range is
  `[start + i*bw, start + (i+1)*bw)` where `bw = (end - start) / bins.length`
  (mirrors `expand.rs:103-116`).
- `quantile_from_histogram(histogram, ratio)` (`quantile.rs:15-41`,
  `estimate_quantile`) estimates a quantile by walking `bins` until the cumulative
  count crosses `count * ratio`, then linearly interpolates within that bucket. This
  is a pure function of fields already on the struct (`start`, `end`, `count`,
  `bins`) — **no extra SQL column is needed to get the median**; it can be
  recomputed client-side from the same struct value the cell already has.
- `expand_histogram(histogram)` (table function, `expand.rs`) turns one histogram
  into `(bin_center, count)` rows for full-size bar-chart rendering via a regular
  `Chart` cell — this is the existing "big" histogram visualization. This plan adds
  the compact **per-row, per-table-cell** counterpart the issue asks for.

Since the struct already carries `start`/`end`/`bins`/`count`, the issue's suggested
rough shape (`{ column, format: 'histogram', bucketsColumn: ... }`, a separate column
naming per-row bucket boundaries) is unnecessary — one `Histogram`-typed column *is*
the full per-row bucket/boundary payload already. This significantly simplifies the
issue's proposed config surface (see Design below).

### Table / Transposed Table cell architecture

- Renderers: `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` and
  `TransposedTableCell.tsx` (notebook cells), plus
  `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` (the standalone
  screens table — a third `TableBody`/`OverrideEditor` host, outside the notebook
  cell-editor pipeline `CellEditorProps`/`CellEditor.tsx` covers); all four build
  on shared logic in `analytics-web-app/src/lib/screen-renderers/table-utils.tsx`
  (1058 lines) — column management, `TableBody`, `formatCell`, `OverrideCell`. A
  fourth host, `cells/ReferenceTableCell.tsx`, also calls `<TableBody
  data={slicedData} columns={visibleColumns} compact … />`
  (`ReferenceTableCell.tsx:132`), but it's CSV-only: `execute` is
  `csvToArrowIPC(refConfig.csv)`, and `csv-to-arrow.ts` infers only `Float64` or
  `Utf8` per column — never the `Histogram` struct shape — so a histogram-typed
  column can never occur there and this plan's default histogram rendering is
  unreachable on that host.
- **Per-cell rendering switch** (the place a new render path plugs in):
  `table-utils.tsx:711-739` (`TableBody`, one branch per column) and
  `TransposedTableCell.tsx:122-134` (same idea, transposed). Today it's binary: if
  the column has a `ColumnOverride`, render `OverrideCell`; otherwise render
  `formatCell(value, col.type)` as plain text in a `<td>`.
- **`ColumnOverride`** (`table-utils.tsx:31-37`):
  ```ts
  export interface ColumnOverride {
    column: string
    format: string   // markdown template, e.g. "[View]($row.process_id)"
  }
  ```
  `format` is expanded via `evaluateTemplate` (`notebook-utils.ts` /
  `template-evaluator.ts`) and rendered with `react-markdown`. `$row.col` resolves
  to `ctx.row[col]` (`macro-resolve.ts:94-99`, case `'rowCol'`) — the **raw** cell
  value, whatever type it is — then `formatArrowValue(value, dataType)`
  (`macro-substitution.ts:26-32`) stringifies it for output: special-cased for
  timestamps, otherwise a bare `String(value)`. For a `Struct` value (like a
  histogram), `String(value)` already produces a readable field dump today: the
  cell value is a `StructRowProxy` whose `get()` trap resolves prototype methods
  first (`Reflect.has(row, key)`, `node_modules/apache-arrow/row/struct.js:98-101`),
  so `String(value)` calls `StructRow.toString()` (same file, lines 42-44) and
  yields `{"start": 0, "end": 50, "min": …, "bins": ["3","7",…]}` — not
  `[object Object]`. `$row.col` on a histogram column therefore already dumps the
  struct today; this plan's only change to the override pipeline itself (Design
  §5) makes that dump more compact/consistent, not functional.
- Both cell types configure `overrides`/`hiddenColumns`(`hiddenRows` for Transposed)
  as string-array/array fields in `options` (`QueryCellConfig.options?:
  Record<string, unknown>`, `notebook-types.ts:120`).
- Docs: `mkdocs/docs/web-app/notebooks/cell-types.md:694-780` (Table / Transposed
  Table sections) documents `overrides`, `hiddenColumns`/`hiddenRows` today.

### Cell editor panel

The right-side editor panel (`components/CellEditor.tsx`) is a fixed shell — header
(type badge + cell name + close), a scrollable content area (Cell Name input, Data
Source field, optional time range, then `<meta.EditorComponent>` — the
cell-type-specific body, e.g. `TableCellEditor`), and a footer (Run / Delete). It's
rendered in a resizable right panel (`NotebookRenderer.tsx:798-808`,
`useEditorPanelWidth.ts`: default `350px`, range `280–800px`). The Overrides section
itself is `components/OverrideEditor.tsx` — an accordion of per-column cards (column
`<select>` + Format `<textarea>`), used identically by `TableCellEditor` and
`TransposedTableCellEditor`.

`CellEditorProps.availableColumns` (`cell-registry.ts:90`) is `string[]` — **names
only**, no `DataType`. It's populated at the single call site
(`NotebookRenderer.tsx:833`) via
`cellStates[...].data[0]?.schema.fields.map((f) => f.name)` — the full `Field`
(name + type) is right there before `.map` strips it down. Every existing consumer
(`OverrideEditor`, `MapCell`, `HorizontalGroupCell`, …) only needs names, so this has
never mattered before. `OverrideEditor` needs types now, to know which column's card
should show the "Render as" toggle (Design §6).

`NotebookRenderer.tsx` doesn't call `meta.EditorComponent` directly for a top-level
cell, though — it renders `<CellEditor>` (`components/CellEditor.tsx`), which
declares its own local `CellEditorProps` interface (lines 11-25, `availableColumns?:
string[]` among them) and explicitly forwards a fixed prop list to
`<meta.EditorComponent … availableColumns={availableColumns} … />` (line 161).
Anything not named in `CellEditor.tsx`'s own interface and forwarded there never
reaches `TableCellEditor`/`TransposedTableCellEditor` via this path, so the new
`availableColumnTypes` prop needs the same two touches here as `availableColumns`
already has: added to `CellEditor.tsx`'s local `CellEditorProps`, destructured, and
passed through to `<meta.EditorComponent>`.

`CellEditor.tsx` is not the only forwarding point into `meta.EditorComponent`,
though: `HorizontalGroupCell.tsx`'s `ChildEditorView` (lines ~276-397) also renders
`<meta.EditorComponent … availableColumns={availableColumns} … />` for a
horizontal-group child, which includes Table and Transposed Table cells nested
inside a group. That path is reached through a *separate* top-level branch in
`NotebookRenderer.tsx` — `HgEditorPanel` (declared and called around lines 133-190
and 804-820), not `CellEditor` — and `HgEditorPanelProps`/its call site never
declare or pass `availableColumns` at all, so `ChildEditorView`'s own
`availableColumns` prop is always `undefined` for every child editor today,
regardless of this plan. This is a pre-existing gap, not something this plan
introduces or regresses, but it means an HG-nested Table/Transposed Table cell's
"Render as" toggle (Design §6) is unreachable no matter what threading this plan
adds — see Design §6 / Files to Modify for how this plan scopes around it.

### Color conventions (Chart and Map cells)

There is no dedicated "Heatmap" cell; the closest existing "heatmap-style" surface is
the Map cell's density-overlay use case (`cell-types.md:324` — orthographic camera
mode "better for flat heatmap-style data"). Colors across Chart and Map cells follow
one shared convention:

- **Categorical/default palette**: `analytics-web-app/src/components/chart-constants.ts`
  — `SERIES_COLORS` (12-color brand palette, Rust Orange first) and
  `DEFAULT_SERIES_COLOR = SERIES_COLORS[0]` (`#bf360c`). `XYChart.tsx` resolves each
  series' color as `s.color ?? SERIES_COLORS[i % SERIES_COLORS.length]`
  (`XYChart.tsx:664,763,1131`); `ChartCell.tsx` seeds new queries from the same
  palette. The Swimlane cell's per-segment color similarly falls back to the CSS
  variable `var(--chart-line)` (`#bf360c`) when no explicit color is supplied
  (`tasks/completed/1127_swimlane_cell_color_plan.md`).
- **Per-row/value-driven color**: an optional SQL `color` column, decoded by the
  shared `analytics-web-app/src/lib/color-utils.ts` (`cellColorToCss`) — packed RGBA
  `u32`, `#rrggbb(aa)` hex string, or 4-byte binary, all following the
  `0xRRGGBBAA` convention shared with the `rgba()`/`color_scale()` SQL UDFs
  (`rust/datafusion-extensions/src/color/`). `color_scale(name, t, alpha)`
  (`color_scale.rs`) is the codebase's sequential-colormap convention (viridis,
  magma, plasma, inferno, cividis, turbo) for SQL-computed heat-style gradients —
  computed entirely server-side, not with a client color-scale library.
- **Theming**: the app has **one theme only** — a hardcoded dark palette in
  `analytics-web-app/src/styles/globals.css` (no `[data-theme]`, no
  `prefers-color-scheme`). All chart/tooltip styling uses CSS custom properties
  (`--chart-line`, `--app-bg`, `--panel-bg`, `--border-color`, `--brand-gold`, …)
  rather than hardcoded hex, so this feature should do the same — no dark/light
  branching needed.
- **Tooltips**: Chart cell tooltips are custom **uPlot** plugins
  (`createMultiSeriesTooltipPlugin`, `XYChart.tsx:424-513`) — vanilla-DOM divs
  appended to `document.body`, positioned in fixed coordinates to escape cell
  `overflow:hidden` clipping, flipped above/below the cursor near viewport edges.
  That machinery is uPlot-specific and overkill for a small per-cell chart. The
  **Swimlane cell's** tooltip is the better-fitting precedent — plain React state
  (`SwimlaneCell.tsx:195-202`, `useState<{x,y,...} | null>`), `onMouseEnter` /
  `onMouseMove` / `onMouseLeave` per segment, and a `position: fixed` div
  (`SwimlaneCell.tsx:402-417`) styled with `bg-app-bg border border-theme-border
  rounded-md ... shadow-lg`, clamped to the viewport. This plan follows that pattern.

## Design

### 1. Detecting a histogram column (the "default behavior" trigger)

Add to `arrow-utils.ts` a structural-shape check, not a nominal type tag (Arrow has
no extension-type metadata on the `Histogram` struct today, so detection must be
structural):

```ts
const HISTOGRAM_FIELD_NAMES = ['start', 'end', 'min', 'max', 'sum', 'sum_sq', 'count', 'bins']

export function isHistogramStructType(dataType: DataType): boolean {
  if (!DataType.isStruct(dataType)) return false
  const fields = (dataType as Struct).children
  if (fields.length !== HISTOGRAM_FIELD_NAMES.length) return false
  return fields.every((f, i) => f.name === HISTOGRAM_FIELD_NAMES[i]) &&
    DataType.isList(fields[fields.length - 1].type)
}
```

Field-name-and-order equality (plus "last field is a `List`") is specific enough
that an arbitrary user struct is very unlikely to collide, while staying robust to
incidental width differences (the check doesn't hard-code `Float64` vs `Float32` for
every subfield). This is a deliberate trade-off — see Trade-offs.

Calling this function is cheap, but the render loop must not call it once per
cell: `TableBody` maps `columns` inside a per-row `Array.from`, and
`TransposedTableCell` maps `row.values` inside a per-row loop too, so a naive
per-cell call would run it row × column times instead of once per column.
Design §3 specifies the memoization that keeps the actual per-render cost at
once per column.

### 2. `ColumnOverride` gains a second render kind: `'markdown'` or `'histogram'`

One column, one override card, one "how does this render" answer — a histogram
column's card doesn't move to a separate section, it just gets a second option next
to the format template it already had:

```ts
export interface ColumnOverride {
  column: string
  /** 'markdown' (default — every override before this change already behaves
   *  this way) or 'histogram'. 'histogram' only takes effect when the column
   *  is histogram-typed (Design §1); on any other column it's inert, same as
   *  a markdown override targeting a column that no longer exists. */
  kind?: 'markdown' | 'histogram'
  format?: string          // markdown template — used when kind is 'markdown'
  histogramColor?: string  // used when kind is 'histogram' — see Design §6
}
```

`format` becomes optional; `kind` omitted is the existing behavior, so every
override stored before this change still parses and behaves identically. No config
entry at all (the common case) → the column still gets the default histogram
rendering with flat `var(--chart-line)` bars; a `kind: 'histogram'` card only needs
to exist when a user wants non-default bar colors — "no override for this column"
already means default, so there's no `'default'` mode variant to represent.
`histogramColor` is a single field, not a nested mode object — its one string does
double duty (Design §6): a recognized colormap name, or a literal CSS color.

**Changing an existing card's column** (`OverrideEditor.handleColumnChange`,
`OverrideEditor.tsx:62-69`) currently does `newOverrides[index] = {
...newOverrides[index], column }` — every other field, including `kind`, survives
untouched. Once histogram columns exist, that's wrong by default: switching a
markdown card onto a histogram column would leave `kind` unset, which Design §3
rule 1 still routes to `OverrideCell` with a template now referencing the wrong
column, instead of the histogram rendering a user picking that column would
expect. `handleColumnChange` must re-derive `kind` from the newly selected
column's type (via the new `availableColumnTypes` prop) whenever that derived
`kind` differs from the card's current one — a same-kind column change (e.g.
markdown column → another markdown column) is unaffected, that's today's
behavior and out of scope here:
- New column `isHistogramStructType` and current `kind !== 'histogram'` → set
  `kind: 'histogram'`, clear `format` (`undefined`) since the old template
  referenced the previous column and is stale; leave `histogramColor` as-is
  (`undefined` stays `undefined` → default flat color; a value from a prior
  histogram-column selection is harmless to keep, and lets a user bounce
  between two histogram columns without losing their color choice).
- New column not histogram-typed and current `kind === 'histogram'` → set
  `kind: 'markdown'`, clear `histogramColor` (`undefined` — inert once `kind`
  is `'markdown'` anyway, cleared for cleanliness) and seed `format` with the
  same `[Link](/path?id=$row.<col>)` template `handleAddOverride` uses for a
  brand-new override, unconditionally overwriting whatever `format` the card
  currently holds. This is a column *change*, not a toggle: any existing
  template referenced the previous column and is stale for the new one (even if
  it happens to still be non-empty, e.g. carried over from an earlier Markdown
  stint before a Histogram toggle — Design §6 — followed by this column swap),
  so `handleColumnChange` always reseeds rather than preserving it. This is
  deliberately the opposite of the toggle's rule in Design §6, which preserves
  an existing template because the column hasn't changed underneath it.

Both branches above only apply when the newly selected column's type is present
in `availableColumnTypes` — which, per Design §6, can be empty or missing an
entry even for a histogram-typed column (e.g. before a query has run, or the
prop simply hasn't loaded yet). When the new column's type is unknown,
`handleColumnChange` cannot tell whether it's histogram-typed, so it leaves
`kind`, `format`, and `histogramColor` untouched — only `column` changes, same
as today's behavior — rather than guessing and risking an incorrect flip to
`'markdown'` on what is actually a histogram column.

Two existing `string`-typed consumers of `.format` need a matching fallback, since
`ColumnOverride.format` is now `string | undefined` while they aren't:
`OverrideCellProps.format` (`table-utils.tsx:236-237`, kept `string`, not widened)
and `validateFormatMacros(template: string, …)` (`table-utils.tsx:118`). Both stay
required-`string` — it's the two call sites feeding them that change, to
`override.format ?? ''`: `OverrideEditor.tsx:33` (`validateFormatMacros(override.format
?? '', …)`) and the `<OverrideCell format={…}>` sites in `TableBody`
(`table-utils.tsx:719`, once `overrideMap` holds full entries per Design §3) and
`TransposedTableCell.tsx` (line ~124). `OverrideEditor`'s `<textarea
value={override.format}>` (line 147) and `handleFormatChange` get the same
treatment (`value={override.format ?? ''}`) so the textarea doesn't flip
uncontrolled when a `kind: 'histogram'` card has no `format`. In practice
`OverrideCell` and the textarea are only reached for `kind: 'markdown'` (or unset)
entries once Design §3/§9's UI is in place, but the `?? ''` guard is what makes
that true at the type level too, not just by convention.

### 3. Per-cell render selection (in `TableBody` and `TransposedTableCell`)

Extend the existing binary switch, evaluated in this order per column:

1. Column has a `ColumnOverride` with `kind: 'markdown'` (or no `kind` — an
   existing override) → `OverrideCell`. This is a presence check on the map
   entry, not on `.format`: a saved override with `format: ''` now renders an
   empty `OverrideCell` here rather than falling through to case 3. That's a
   deliberate behavior change from today's `TableBody`, which reads `override
   = overrideMap.get(col.name)` as the format string itself and tests `if
   (override)` — a blank format is falsy there today, so it falls through to
   `formatCell`. Switching the map to full entries makes the lookup always
   truthy once an entry exists, and this plan accepts that rather than adding
   a `.format`-truthiness special case, since it unifies `TableBody` with
   `TransposedTableCell`'s existing `overrideMap.has(row.name)` check (already
   presence-based today, unaffected by this plan) and a blank saved format is
   a degenerate edge case, not a meaningful configuration. If the format
   references a histogram-typed column via `$row.col`, it already resolves to a
   readable struct dump rather than an unhelpful stringification (see below) — this
   is the whole debugging story, no separate mechanism.
2. Column `isHistogramStructType(col.type)` **and** (no override **or**
   `kind: 'histogram'`) → new `HistogramCell` component (bars + median +
   tooltip), passed `override?.histogramColor` for bar coloring (`undefined` →
   default flat color).
3. Otherwise → `formatCell(value, col.type)`, unchanged — this branch never sees a
   histogram-typed value in practice, since case 2 already claims every histogram
   column that isn't explicitly overridden to markdown.

```
override?.kind is markdown (or unset)  → OverrideCell        (existing; $row.col already
                                                                dumps histogram structs
                                                                usefully — Design §5)
histogram && (no override | histogram) → <HistogramCell color={override?.histogramColor}>
else                                    → formatCell           (existing, unchanged)
```

Building `overrideMap` (`table-utils.tsx:677-683`) changes from `Map<string,
string>` (column → `format`) to `Map<string, ColumnOverride>` (column → full
entry), since `HistogramCell` needs `.histogramColor`, not just `.format`.
`OverrideCell`'s call site adjusts to read `.format` off the looked-up entry instead
of using the map's value directly — a one-line change at each of its two call sites.
The truthiness check that gated case 1 (`if (override)`) becomes `if (override &&
override.kind !== 'histogram')` — the `kind` exclusion is what routes a
`kind: 'histogram'` entry to case 2 instead, and the remaining `override` part is
now a presence check on the map entry rather than on the format string — see the
note on case 1 above for why that's now presence-based, and the accepted behavior
change that follows for a saved `format: ''`.

**Memoization (keeps §1's per-column cost real):** `TableBody` maps `columns`
inside a per-row loop, and `TransposedTableCell` maps `row.values` inside a
per-row loop, so the type check above must not be called per cell. `TableBody`
builds a `useMemo`'d `Set<string>` of histogram-typed column names once from
`columns` (recomputed only when `columns` changes), alongside the existing
`overrideMap` memo, and the per-cell switch tests membership in that set
instead of calling `isHistogramStructType` per cell. `TransposedTableCell`
calls `isHistogramStructType(row.type)` once per row, hoisted above the
`row.values.map` (`row.type` is constant across a transposed row's values), and
reuses that single boolean for every value in the row instead of recomputing it
per value.

This only keeps the *routing* check (`isHistogramStructType`) cheap — it says
nothing about `HistogramCell`'s own per-render cost once a cell is routed there.
`TableBody` builds `const row = data.get(rowIdx)` fresh inside its per-row
`Array.from`, and `TransposedTableCell` does the equivalent per row too, so the
histogram struct value `HistogramCell` receives has a fresh identity on every
parent render: Arrow's getters allocate a new `StructRowProxy` from `getStruct`
and a new `Vector` from `getList` on every access, the exact trap `OverrideCell`
had before #1092 (`tasks/completed/1092_override_cell_memo_fix_plan.md`; see the
"Do NOT replace `[cacheKey]` with the obvious prop list" comment at
`table-utils.tsx:384-386`). A `useMemo(..., [value])` inside `HistogramCell`
around `toHistogramValue`/`estimateHistogramQuantile`/per-bucket
`resolveHistogramBarColor` would therefore never hit. See Design §4 for
`HistogramCell`'s own memoization plan.

`formatCell` itself needs **no changes** — unlike an earlier draft of this plan,
which added a dedicated debug-string branch there. It's unreachable for histogram
values now (case 3 above never actually sees one), so there's nothing to special-case.

**Interaction with `TableRenderer`'s lack of pagination:** case 2 above routes every
row of a histogram-typed column to `HistogramCell`, and `TableRenderer.tsx` passes
the *full* result table to `TableBody` (`data={table}`) with no page slicing —
unlike `cells/TableCell.tsx`, which caps rendering at `DEFAULT_PAGE_SIZE = 100`
rows. A screens table grouping by a high-cardinality key can therefore route
thousands of rows through case 2 at once. Design §4 bounds each routed cell's own
DOM/listener footprint to a small, row-count-independent constant (one `<svg>`
element and one pointer listener, regardless of bucket count) specifically so this
routing decision doesn't multiply an unbounded row count by a per-bucket element
count; it does not add pagination to `TableRenderer`, which stays out of scope for
this plan.

### 4. `HistogramCell` component (new file, e.g. `components/HistogramCell.tsx`)

Renders inside a `<td>`, replacing the plain-string cell content (same pattern as
`OverrideCell` — a component, not a formatted string).

- **Memoization**: the struct value `HistogramCell` receives is a fresh
  `StructRowProxy` on every parent render — `TableBody` builds `const row =
  data.get(rowIdx)` fresh inside its per-row `Array.from`, and Arrow's
  `getStruct`/`getList` getters allocate a new proxy/`Vector` on every access —
  so keying a `useMemo` on the value's identity (`[value]`) never hits, exactly
  the trap #1092 fixed for `OverrideCell`
  (`tasks/completed/1092_override_cell_memo_fix_plan.md`; see the "Do NOT
  replace `[cacheKey]` with the obvious prop list" comment at
  `table-utils.tsx:384-386`). `HistogramCell` is exported wrapped in
  `React.memo` with a custom comparator, not the default shallow-prop
  comparison (which would also fail, for the same reason): the comparator
  normalizes each side's value via `toHistogramValue` (cheap — one `Number()`
  and one `Array.from`) and compares the resulting `{start, end, count,
  bins}` plus the `color` prop by content, rather than comparing the raw
  `StructRowProxy`/`Vector` references. When the comparator reports equal,
  React skips re-invoking the component entirely, so `estimateHistogramQuantile`,
  `bucketRange`, and the per-bucket `resolveHistogramBarColor` calls — the
  actual expensive work — never re-run for a row whose histogram content is
  unchanged. This lives in `HistogramCell`/`histogram-utils.ts`, not in
  `table-utils.tsx`'s `stableStringify`/`buildEvaluateKey` (importing those
  would create a circular dependency, since `table-utils.tsx` is what renders
  `HistogramCell`) — a small structural equality check plays the same role
  here that the stringified cache key plays for `OverrideCell`.
- **Cell dimensions**: the histogram cell itself is `width: 168px; height: 28px`
  (matching `option-b-tick-median.html`'s `.histo-cell`), but that 168px is the
  whole cell, not the bar track — it splits into a `120px` bar track
  (`.histo-track`, fixed, not `flex: 1 1 auto`) + `6px` gap
  (`.histo-bars-wrap`'s `gap`) + a `42px` fixed-width trailing median label
  (`.histo-median-label`, `flex: 0 0 42px`, `text-align: right`), so
  `120 + 6 + 42 = 168`. The mockup already uses these fixed bases —
  `.histo-track { flex: 0 0 120px }` and `.histo-median-label { flex: 0 0 42px;
  text-align: right }` — and the component matches those outer proportions
  exactly; a content-sized label would instead make the track width, every bar's
  width, and the median tick's x-position all vary row-to-row with the digit
  count of the formatted median. All of this has to be fixed-size, not
  content-derived: `TableBody` renders every cell as `<td className="...
  truncate max-w-xs">` (`table-utils.tsx:672-674`) and `TransposedTableCell.tsx`
  uses `<td className="px-3 py-1.5 ...">` (line 120) — neither `<td>` sets a
  width, and HTML's default auto table layout sizes an unfixed column to its
  content's max-content contribution. The bar track itself is a single inline
  `<svg width={120} height={28} viewBox="0 0 120 28" preserveAspectRatio="none">`
  (see "Bars" below) — an `<svg>` with explicit `width`/`height` attributes has a
  fixed intrinsic size regardless of its children, so the zero-max-content-
  contribution hazard a flex row of unsized bar `<div>`s would have had doesn't
  apply here.
- **Bars — one `<svg>` per cell, not two `<div>`s per bucket**: every bucket is
  rendered, but as a single `<rect>` inside one shared `<svg width={120}
  height={28} viewBox="0 0 120 28" preserveAspectRatio="none">` bar track, not as
  a pair of full-height hover-wrapper + bar `<div>`s. This bounds the DOM/listener
  cost of a histogram cell to a small constant independent of bucket count — one
  `<svg>` element, `bins.length` `<rect>` children, and a single `onPointerMove`/
  `onPointerLeave` pair of listeners on the `<svg>` itself — rather than two
  elements and three mouse handlers (`onMouseEnter`/`onMouseMove`/`onMouseLeave`)
  per bucket; see the note on `TableRenderer`'s lack of pagination at the end of
  Design §3 for why that bound matters. Bucket `i`'s `<rect>` is positioned at `x
  = i * (120 / bins.length)`, `width = 120 / bins.length` (so `bins.length` rects
  always tile the 120-unit track exactly, for any bin count — no gap, no
  overflow, no per-threshold gap logic needed the way a flex `gap` would have
  required), `height = max > 0 ? Math.max(2, (bucket_count / max) * 100 * 28 /
  100) : 0` — i.e. the same `min-height: 2px` floor as before, just computed in
  SVG user units instead of CSS `%`/`min-height` — and `y = 28 - height`, where
  `max` is `max(bucket_count in this row)` — **per-row normalization**, matching
  `option-b-tick-median.html`'s own `const h = max > 0 ? Math.max(2, (v / max) *
  100) : 0`. The point of the cell is to show *shape*, not to compare magnitude
  row-to-row (the issue frames this as "spot rows with unusual spread, multiple
  modes, or outliers" — a shape question), so each row's own tallest bucket
  reaches the full 28-unit height. The `2`-unit floor keeps a 0-count bucket
  rendered as a visible stub rather than a zero-height rect; because hit-testing
  happens via the pointer's x-position across the whole `<svg>` (see "Tooltip"
  below), not via each `<rect>`'s own geometry, a 2-unit-tall stub is just as
  hoverable as a full-height bar — the per-bucket tooltip works on empty buckets
  too, exactly the buckets a user would probe when reading spread. This "visible
  stub" guarantee is about the bar's *height*, not its *color* — a 2-unit-tall
  stub filled in a near-black colormap color against the app's near-black
  background would technically render but not actually be visible; Design §6
  floors the colormap sampling ratio so that doesn't happen, keeping the
  guarantee true in every color mode. When the row's max bucket is 0 (see "Null
  and degenerate values" below), skip the ratio entirely — it would otherwise be
  `0/0 = NaN` — and render the degenerate case instead. Fill color defaults to
  `var(--chart-line)` (same default as Chart/Swimlane single-series color) unless
  the override supplies a `histogramColor` — see Design §6.
- **Bucket count vs. cell width**: every bucket is always rendered — no cap, no
  downsampling, and no bucket is ever dropped or merged. Because each `<rect>`'s
  `x`/`width` are computed as an exact fraction of the fixed 120-unit `viewBox`
  (see "Bars" above) rather than laid out via flexbox, `bins.length` bars always
  fit exactly within the track for any bin count — there's no flex-`gap`-vs-
  negative-free-space failure mode to guard against, so no conditional gap
  threshold is needed either. `make_histogram`'s bin count is caller-chosen in
  SQL (typically 15–30 for this use case) with no server-side cap
  (`configure_from_params` only enforces `nb_bins >= 1`); at very high bin counts
  individual rects become sub-pixel and visually imperceptible, but no bucket is
  ever dropped, merged, or clipped — every bucket still occupies its exact
  proportional share of the track width, and the `<svg>`/`<rect>` count for the
  cell stays exactly `1 + bins.length` regardless. There is no `MAX_RENDERED_BARS`
  constant and no bucket-merging step — the bound here is on DOM node count
  overhead (one `<svg>` and its listeners, not two elements and three handlers
  per bucket), not on how many buckets get drawn.
- **Median overlay (locked in: Option B, tick-mark)**: compute via the same
  linear-interpolation as `estimate_quantile` in `quantile.rs:15-41`, ported to TS
  (ratio fixed at `0.5`), operating on `start`/`end`/`count`/`bins` already on the
  cell's value — no new SQL column, including that function's `return end` fallback
  when no bucket's cumulative count reaches the target ratio (`quantile.rs:40`).
  Drawn as a vertical gold (`var(--brand-gold)`) `<line>` inside the same
  `120×28` bar-track `<svg>` (not a separate element — see "Bars" above), at `x1
  = x2 = ((median - start) / (end - start)) * 120`, `y1 = 0`, `y2 = 28` — except
  when `Math.abs(end - start) < Number.EPSILON` (the degenerate point-histogram
  case, `start == end` is legal input and has its own Rust test,
  `histogram_runtime_bounds_tests.rs:423-438` "Test 5: Degenerate point
  histogram"), where that division is `0/0 = NaN`; pin the tick to `0%` instead,
  mirroring the same `Math.abs(end - start) < Number.EPSILON → bin_width = 1.0`
  epsilon-guarded fallback already ported for `bucketRange`
  (`expand.rs:103-107` — `(end - start).abs() < f64::EPSILON`) — with
  a single point mass, the median is that point, at the start of the (unit-width)
  bucket. The numeric value renders in a fixed-width (`42px`) label trailing the
  chart, formatted with the mockup's own bounded-precision rule — `median.toFixed(1)`
  (`option-b-tick-median.html`), not `toLocaleString()` (unbounded fraction digits
  plus grouping separators, e.g. `1,234.568`, routinely wider than 42px at the
  label's `9.5px` font). `toFixed(1)` caps output at one fraction digit, which fits
  42px for the value ranges this feature targets; on the rare value that still
  overflows (e.g. a very large magnitude), the label's existing `overflow: hidden;
  text-overflow: ellipsis` (`.histo-median-label`) truncates it — no separate
  handling needed. No unit is appended, consistent with `formatCell`'s numeric
  default (`table-utils.tsx:767-774`); per-column unit hints are a possible
  follow-up, out of scope here. Chosen over
  overlaying the number directly on the bars (Option A, kept in
  `option-a-text-median.html` for reference) because the tick communicates *where*
  the median sits relative to the spread, not just its value, and a trailing
  fixed-width label can't collide with a tall bucket underneath it.
- **Tooltip**: reuse the Swimlane cell's `position: fixed` tooltip-div pattern for
  content/styling, but not its per-segment handler wiring — local `useState<{x, y,
  bucket} | null>`, and a single `onPointerMove`/`onPointerLeave` pair on the
  `<svg>` itself (Design §4 "Bars" above), not `onMouseEnter`/`onMouseMove`/
  `onMouseLeave` per bucket. `onPointerMove` reads the pointer's x-position via
  `event.clientX - svgRect.left` (from a `getBoundingClientRect()` cached in a
  ref, or `nativeEvent.offsetX`), maps it into a bucket index with `Math.floor(x /
  (trackWidthPx / bins.length))`, clamped to `[0, bins.length - 1]`, and updates
  state only when that index changes; `onPointerLeave` clears it. The tooltip
  content itself is unchanged — `position: fixed` div styled `bg-app-bg
  border border-theme-border rounded-md shadow-lg`. Content: bucket range
  (`[start, end)` computed the same way as
  `expand_histogram`, including its `Math.abs(end - start) < Number.EPSILON →
  bin_width = 1.0` fallback, `expand.rs:103-107`) and count + percentage of the row's total
  (`bucket_count / count * 100`, only when `count > 0` — see below).
- **Null and degenerate values**: a `null` histogram value renders `-`, matching
  `formatCell`'s existing null convention (`table-utils.tsx:755`). The same `-`
  (empty track, no median tick) also covers three degenerate-but-non-null cases the
  Rust side can actually produce: `bins` empty (`new_non_configured` + all-null
  input leaves `start = end = 0`, `bins: Vec::new()` — `accumulator.rs:46-57`),
  `count === 0` (every group whose sampled values are all null), or every bucket at
  0 (equivalent to `count === 0`, since `update_batch_scalars` clamps every
  non-null sample into range, so `sum(bins) === count` — `accumulator.rs:120-130`).
  Rendering these as `-` avoids computing `bucket_count / max(...) = 0/0 = NaN` bar
  heights and `bucket_count / count * 100 = 0/0` tooltip percentages.
- **Value shape from Arrow JS**: apache-arrow (`^21.2.0`) surfaces a `Struct` column
  cell as a `StructRowProxy` with named field access. `start`/`end`/`min`/`max`/
  `sum`/`sum_sq` are `Float64` fields, which decode to plain `number`
  (`type.d.ts:222-223`, `TArray: Float64Array; TValue: number`). `count` and every
  element of `bins` are `UInt64`, which decode to `bigint` in this arrow version
  (`type.d.ts:142-143`, `TArray: BigUint64Array; TValue: bigint`) — `row.count` is a
  `bigint`, and `row.bins` (`List<UInt64>`) is a `Vector` (`getList`,
  `visitor/get.js:169-176`), not an indexable array — it has no numeric index
  signature, so it must be read via `Array.from(row.bins, Number)` or iteration, not
  `bins[i]`. This plan normalizes once at the read boundary: a
  `toHistogramValue(raw: StructRowProxy): HistogramValue` helper (Implementation
  Steps §2) does `Number(raw.count)` and `Array.from(raw.bins, Number)`, so every
  downstream helper and component works with plain `number`s only — no `bigint`
  arithmetic anywhere else in the histogram code.

### 5. Debugging: reuse Markdown, no dedicated feature (already works today)

The task asked for a way to fall back to the raw struct for debugging. The
*existing* Markdown override path already covers this for free, with zero code
changes required: because a histogram-typed cell value is a `StructRowProxy`,
`formatArrowValue`'s existing fallback `String(value)` already resolves to
`StructRow.toString()` (Current State above) and renders a readable field dump
(`{"start": 0, "end": 50, "min": …, "bins": […]}`). What follows is an optional
polish, not a fix: `formatArrowValue` (`macro-substitution.ts:26-32`), the function
that renders a resolved `$row.col`/`$variable.col`/etc. macro to text — shared by
both the SQL-side `substituteMacros` and the display-side `evaluateTemplate` — can
gain one more case to make that dump more compact/consistent (fixed field order, no
per-value quoting) than Arrow's own pretty-printer produces:

```ts
export function formatArrowValue(value: unknown, dataType?: DataType): string {
  if (dataType && isTimeType(dataType)) {
    const date = timestampToDate(value, dataType)
    if (date) return date.toISOString()
  }
  if (dataType && isHistogramStructType(dataType)) {
    const h = toHistogramValue(value as StructRowProxy) // normalizes bigint count/bins to number
    return `{start:${h.start}, end:${h.end}, count:${h.count}, bins:[${h.bins.join(',')}]}`
  }
  return String(value)
}
```

With that in place, "show me the raw data" is: open Overrides, add (or already have)
a card for the histogram column, leave "Render as" on **Markdown**, and set Format
to `$row.duration_dist` (or embed it in a larger template — `**debug:**
$row.duration_dist` works too). The column then renders that struct dump instead of
the chart — exactly the same mechanism as any other override, not a parallel
feature with its own UI, state, or context-menu item. Switching back to Histogram
(or removing the card) restores the chart.

This is strictly less surface than a dedicated `textColumns` toggle would have been:
no new `options` field, no context-menu changes to `SortHeader`/`RowContextMenu`, no
`formatCell` branch — and even the one `formatArrowValue` case above is optional,
since the debug path already works without it.

### 6. Custom bar color

The default flat `var(--chart-line)` fill is a fine default but users profiling a
"top N by cost" table may want a heat-style cue baked into the bars themselves.
`ColumnOverride.histogramColor` is a single string, and what it means is inferred
from its value — no mode selector needed:

- **A recognized colormap name** (`viridis`, `magma`, `plasma`, `inferno`,
  `cividis`, `turbo` — the same six `color_scale(name, t, alpha)` supports,
  `rust/datafusion-extensions/src/color/color_scale.rs`) → each bucket samples that
  colormap at its own normalized height ratio `t = bucket_count /
  max(bucket_count in this row)` — the same value already driving bar height
  (Design §4). There's no per-bucket "value" column to plug in here (unlike
  Chart/Map's row-level `color` column, a single `Histogram` struct has no
  secondary per-bucket signal beyond the counts it already carries), so color and
  height deliberately reinforce the same signal. Sampled at raw `t`, this would
  break Design §4's "visible stub" guarantee: `t = 0` (or close to it) is
  near-black for magma/inferno, indistinguishable against `--app-bg: #0a0a0f`,
  so a zero- or low-count bucket's min-height stub would be present in the DOM
  but not actually visible. To keep the guarantee true in colormap mode too,
  `t` is floored into `[0.15, 1]` before sampling — `t' = 0.15 + t * 0.85` — so
  every bucket, however low its count, lands past the near-black end of the
  scale; only the top of the range still reaches each colormap's brightest stop.
- **Anything else** (a `#rrggbb`/`#rrggbbaa` hex string, or any valid CSS color) →
  every bar in the cell gets that one flat color, `t` unused. This replaces flat
  `var(--chart-line)` with a flat color of the user's choosing — a simpler ask than
  a gradient, and the common case for "just make it a different color."

One field covers both because the two cases are trivially distinguishable (string
lookup against six known names) and a user picking a color never has to declare
which kind they mean — they click a colormap swatch, or they pick a custom color
(Design §6's Editor UI below covers how the value gets set without anyone typing a
name). An earlier draft of this plan also offered a "Custom" 2+ stop gradient mode
(mirroring `lerp_color(c1, c2, t)`); dropped per direction in favor of this simpler
two-case field — see Trade-offs.

**Client-side color math** (new `lib/histogram-colors.ts`):

```ts
const COLORMAP_NAMES = new Set(['viridis', 'magma', 'plasma', 'inferno', 'cividis', 'turbo'])

export function resolveHistogramBarColor(color: string | undefined, t: number): string {
  if (!color) return 'var(--chart-line)'
  if (COLORMAP_NAMES.has(color)) {
    // Floor into [0.15, 1]: a zero/low-count bucket must not sample the
    // near-black end of magma/inferno-style colormaps against --app-bg
    // (#0a0a0f), or Design §4's "visible stub" guarantee breaks silently.
    return colormapInterpolators[color](0.15 + t * 0.85) // d3-scale-chromatic
  }
  return color // literal CSS color — flat fill, t unused
}
```

Named colormaps need real colormap data, which the Rust side gets from the
`colorous` crate specifically to avoid hand-maintaining color tables (rationale in
`tasks/completed/1069_color_scale_udf_plan.md`, Trade-offs). The same reasoning
applies here: **add `d3-scale-chromatic@^3.1.0`** (ISC license, small — two
transitive deps, `d3-color` and `d3-interpolate`, both also ISC;
`interpolateViridis` / `interpolateMagma` / `interpolatePlasma` /
`interpolateInferno` / `interpolateCividis` / `interpolateTurbo`, each returning a
`rgb(...)` string directly usable as a CSS color) rather than vendoring stops by
hand; the package ships no types, so `@types/d3-scale-chromatic@^3.1.0` is needed
alongside it. This is a new frontend dependency — approved, see Open Questions §4;
note its output won't be byte-identical to
`colorous`'s (different source implementations of the same published colormap
data), which is fine for a client-only visual but is called out here in case exact
SQL/client color parity ever matters.

**Editor UI** (see `option-b-cell-editor.html` mockup, revised): a swatch picker,
not a text field a user has to type a name into (or a paragraph explaining what
names exist). Each override card gets one addition — when the selected column
`isHistogramStructType`, **or** the card's own `override.kind === 'histogram'`
already, a two-way "Render as" toggle appears above the existing Format field:
**Markdown** (today's only option; format textarea shown, unchanged) or
**Histogram** (format textarea hidden; a color-swatch row shown instead). The two
amber validation-warning blocks ("Unknown macro" / "Unknown column") that
`OverrideEditor` renders as siblings of the Format textarea move with it: they, and
the `validateFormatMacros` call that feeds them, are gated on the same effective-kind
check the toggle itself uses (stored `kind`, defaulting to `'markdown'` when unset).
A `kind: 'histogram'` card therefore validates and shows nothing about its preserved
`format` — otherwise a card carrying a stale template from an earlier Markdown stint
(Design §2/§6) would keep surfacing warnings about a field the user can no longer see
or edit. The `kind === 'histogram'` fallback matters because the new `availableColumnTypes`
prop (see below) can be empty or missing the column even for a histogram-typed
one: `OverrideEditor` already treats an empty column list as a
real "no results yet" state (`hasResults = availableColumns.length > 0`,
`OverrideEditor.tsx:25`), both `TableRenderer.tsx` (`availableColumns` is `[]`
before a query has run, line 203) and `NotebookRenderer.tsx` (`availableColumns`
is `undefined` before the cell has results, line 833) supply that empty state
before a successful run, and an orphaned override targets a column no longer in
the result set at all. Without the `kind` fallback, a card already saved with
`kind: 'histogram'` would show the Format textarea instead of the toggle/swatches
in any of those states, and editing it there would write a stray `format` onto a
histogram card.

Clicking the toggle writes `kind`, but its `format`/`histogramColor` side effects
are deliberately different from `handleColumnChange`'s (Design §2), not the same
rule: a column change invalidates any existing template, because it referenced
the *old* column and is stale regardless of whether it's still displayed, so
`handleColumnChange` always overwrites `format` when the new column's histogram-
ness flips the effective kind. A toggle click doesn't change which column is
selected, so the same template (if any) still refers to the right column and
there's no staleness to force — it's preserved rather than overwritten:

- **To Markdown**: set `kind: 'markdown'`; clear `histogramColor` (`undefined` —
  inert once `kind` is `'markdown'` anyway); seed `format` with
  `handleAddOverride`'s `[Link](/path?id=$row.<col>)`-style default template
  **only when `format` is currently unset** — a card added on a histogram column
  has no `format` yet, so this is exactly the case that matters, including the
  debugging flow Design §5 documents (flip to Markdown, then type
  `$row.dist`). A card that already carries a `format` from an earlier Markdown
  stint (e.g. toggled back and forth) keeps it untouched, so a deliberate edit is
  never silently overwritten.
- **To Histogram**: set `kind: 'histogram'`; leave `format` untouched (it's simply
  not read while `kind === 'histogram'` — Design §3 case 2 — so a stale template
  is harmless and reappears if the user later toggles back to Markdown) and leave
  `histogramColor` untouched (a value from a prior Histogram stint on this same
  card is preserved, same as the column-change case in Design §2).

- A leading **Default** swatch, rendered as a flat `var(--chart-line)` square (the
  same fallback `resolveHistogramBarColor` returns for `undefined`), comes first
  in the row. Clicking it sets `histogramColor: undefined` — this is the only
  control in the row that *clears* the field rather than setting it, and it's
  what makes the unset state reachable again once any other swatch has been
  clicked: without it, the only way back to the default bar color is deleting
  the whole override card, which for a card toggled over from Markdown (Design
  §6's toggle rules above) would also discard the preserved `format`. Clicking
  Default only clears `histogramColor`; it doesn't touch `format`.
- Six preset swatches, one per colormap, each rendered as its own actual
  mini-gradient (a small `linear-gradient(...)` sampled at 5-6 stops per colormap —
  see `buildColormapPreviewGradient` below) — the swatch *is* the documentation, no
  name has to be read or typed. Clicking one sets `histogramColor` to that name.
- One more swatch for a custom flat color, backed by a native `<input
  type="color">` (so the OS color picker does the picking — no hex-typing
  required either, though the resulting hex is still what's stored). Clicking it
  opens the picker; the swatch shows whatever was last chosen.
- Whichever swatch matches the card's current `histogramColor` gets a highlighted
  ring border, so the active choice is visible at a glance, not inferred from
  text — including Default, which carries the ring exactly when `histogramColor`
  is `undefined` (a brand-new histogram card's starting state, per Implementation
  Steps §9).
- A live bar preview below the row, using the exact same normalized-height math as
  the real cell, updates immediately on selection.

For a column that's neither histogram-typed (by `availableColumnTypes`) nor already
`kind: 'histogram'`, the toggle doesn't render at all — the card looks exactly as it
does today.

`buildColormapPreviewGradient(name: ColormapName): string` (new, in
`histogram-colors.ts`) builds the swatch's CSS gradient by sampling the same
`d3-scale-chromatic` interpolator `resolveHistogramBarColor` uses — e.g. 6 stops
at `t = 0, 0.2, ..., 1` fed through `colormapInterpolators[name](t)` and joined
into a `linear-gradient(to right, ...)` string. This is not a second, hand-copied
table of color data: it derives the preview from the same interpolator function
that colors the real bars, so the swatch can never drift from what clicking it
actually produces. It samples the raw `t = 0..1` range, not
`resolveHistogramBarColor`'s floored `[0.15, 1]` (Design §6 above) — the swatch
is meant to show "this is the viridis colormap" end-to-end so it's recognizable,
not to reproduce a specific bucket's exact pixel color.

**`availableColumnTypes` addition**: extend `CellEditorProps` (`cell-registry.ts:90`)
with a new, additive sibling prop:
```ts
availableColumnTypes?: Record<string, DataType>
```
populated at the same call site as `availableColumns`
(`NotebookRenderer.tsx:833`, `Object.fromEntries(fields.map(f => [f.name, f.type]))`)
and threaded through `CellEditor.tsx`'s own local `CellEditorProps`/destructuring/
forwarding (same two touches `availableColumns` already gets there — see Current
State above) — purely additive, so none of the other seven `availableColumns`
consumers need to change. `OverrideEditor` needs it (new prop, threaded from
`TableCellEditor`/`TransposedTableCellEditor`, same as `availableColumns` already
is) to know which column the "Render as" toggle should appear for.

### 7. What does *not* change

- SQL / Rust: nothing. `make_histogram`, `quantile_from_histogram`, and the
  `Histogram` struct type already exist and are reused as-is.
- `formatCell`: no changes at all (Design §3) — it never sees a histogram-typed
  value once the render-selection switch is in place.
- Every non-histogram column's Overrides card: unchanged appearance and behavior —
  the "Render as" toggle is conditional on the column being histogram-typed.
- `ReferenceTableCell.tsx`: not touched by this plan. It's CSV-only
  (`Float64`/`Utf8` columns only, Current State above), so a histogram-typed
  column can never occur there and this plan's rendering is unreachable on that
  host.

## Mockups

Styled against the app's real dark theme and brand palette (`--chart-line`,
`--brand-gold`, `--panel-bg`, etc.):

- `tasks/histogram_column_cell_mockups/option-b-tick-median.html` — **chosen.**
  Median as a vertical gold tick drawn at its x-position over the bars, with the
  numeric value trailing the chart in a fixed-width label. Last row shows the debug
  view — an ordinary Markdown override with `$row.duration_dist`.
- `tasks/histogram_column_cell_mockups/option-a-text-median.html` — considered, not
  chosen. Median as a small text label overlaid in the corner of the bar area;
  kept for reference since it's more compact but can visually collide with a tall
  bucket directly underneath it.
- `tasks/histogram_column_cell_mockups/option-b-cell-editor.html` — the Overrides
  panel with the "Render as: Markdown / Histogram" toggle on the histogram column's
  card, showing the six-colormap swatch row + custom-color swatch (viridis
  selected) and the live bar preview, styled to match `CellEditor.tsx`'s real
  header/footer chrome and `OverrideEditor.tsx`'s existing card layout exactly.

Median encoding: Option B was chosen over Option A because the tick communicates
*where* the median sits relative to the bucket spread (not just its value), and a
trailing fixed-width label can't collide with a tall bar underneath it — relevant
given the issue's emphasis on spotting "unusual spread" at a glance.

## Implementation Steps

1. **`arrow-utils.ts`**: add `isHistogramStructType(dataType): boolean` (structural
   check per Design §1). No existing helper to extend — this is new.
2. **`lib/histogram-utils.ts`** (new file): pure functions shared across the render
   and debug-format paths —
   - `type HistogramValue = { start: number; end: number; min: number; max: number; sum: number; sum_sq: number; count: number; bins: number[] }`
     — plain numbers only (Design §4); nothing downstream of `toHistogramValue`
     touches a `bigint`.
   - `toHistogramValue(raw: StructRowProxy): HistogramValue` — reads the raw Arrow
     struct cell once, converting `count` (`bigint`) via `Number(raw.count)` and
     `bins` (`Vector<bigint>`) via `Array.from(raw.bins, Number)` (Design §4).
   - `estimateHistogramQuantile(h: HistogramValue, ratio: number): number` — a
     straight port of `quantile.rs::estimate_quantile`, including its `return end`
     fallback (`quantile.rs:40`) when no bucket's cumulative count reaches `count *
     ratio` (e.g. `count === 0`). No `start === end` epsilon guard here: with zero
     width the ported logic already returns `start` unchanged, which is the
     correct median for a point histogram — the `0/0 → NaN` hazard only shows up
     in the tick's *position* formula, not in this value, and is guarded there
     instead (Step 5, Design §4).
   - `bucketRange(h: HistogramValue, bucketIndex: number): [number, number]` — port
     of `expand.rs`'s bin-center math (return the boundaries, not the center, since
     the tooltip wants a range), including its `Math.abs(end - start) <
     Number.EPSILON → bin_width = 1.0` fallback (`expand.rs:103-107` —
     `(end - start).abs() < f64::EPSILON`).
3. **Add `d3-scale-chromatic` dependency** (approved, Open Questions §4):
   `analytics-web-app/package.json` — `d3-scale-chromatic@^3.1.0` +
   `@types/d3-scale-chromatic@^3.1.0` (the package ships no types of its own).
   Done before `histogram-colors.ts` below, which imports it.
4. **`lib/histogram-colors.ts`** (new file): a `COLORMAP_NAMES` set and
   `resolveHistogramBarColor(color: string | undefined, t: number): string` —
   `undefined` → `var(--chart-line)`; a recognized name → the matching
   `d3-scale-chromatic` `interpolateXxx`; anything else → returned as-is (flat
   color, `t` unused). `HistogramCell` calls this once per bucket with that
   bucket's height ratio.
5. **`components/HistogramCell.tsx`** (new file): the bar-chart + median + tooltip
   component per Design §4, accepting an optional `color` prop
   (`ColumnOverride['histogramColor']`), using `histogram-utils.ts` /
   `histogram-colors.ts` and the Swimlane tooltip pattern. Exported wrapped in
   `React.memo` with the custom structural comparator from Design §4 (compares
   normalized `{start, end, count, bins}` plus `color`, not raw prop identity) —
   this, not a `useMemo` keyed on the value prop, is what makes the memoization
   real (Design §3/§4). Used by both Table and Transposed Table paths. The
   median tick's x-position — not
   `estimateHistogramQuantile`'s return value (Step 2) — is where the `start ===
   end` epsilon guard lives: `Math.abs(end - start) < Number.EPSILON → tick at 0%`,
   since it's this component's `((median - start) / (end - start)) * 100%` formula
   that divides by zero on a degenerate point histogram, not the quantile helper
   itself (Design §4). Done before `table-utils.tsx` below, which renders it.
6. **`table-utils.tsx`**: extend `ColumnOverride` with `kind?: 'markdown' |
   'histogram'` and `histogramColor?: string`, making `format` optional (Design
   §2); update `overrideMap` to store the full entry, not just `format` (Design
   §3); add a `useMemo`'d `Set<string>` of histogram-typed column names built
   once from `columns` (Design §3's memoization) so the per-cell switch tests set
   membership instead of calling `isHistogramStructType` per cell; extend
   `TableBody`'s per-cell switch (lines 711-739) per Design §3's ordering, passing
   `format={entry.format ?? ''}` to `<OverrideCell>` (line 719) —
   `OverrideCellProps.format` stays required `string`, unchanged. The case-1
   guard becomes `if (override && override.kind !== 'histogram')` (the `override`
   part now tests presence of the looked-up entry, not the format string; the
   `kind` exclusion is what lets case 2 fire for a `kind: 'histogram'` entry) —
   per Design §3, a saved override with `format: ''`
   renders an empty `OverrideCell` instead of today's fall-through to
   `formatCell`, an accepted behavior change that brings `TableBody` in line
   with `TransposedTableCell`'s existing `.has()` check. **`warning-reporter.tsx`**:
   update the one-line comment at `warning-reporter.tsx:40` (`// small array of
   { column, format } objects`), which documents the shape being content-hashed,
   to reflect the new `{ column, kind?, format?, histogramColor? }` shape — it's
   still a small array safe to `JSON.stringify`, just no longer only two fields.
7. **`macro-substitution.ts`**: add the `isHistogramStructType` branch to
   `formatArrowValue` (Design §5) — the one change that makes debugging "just work"
   through the existing Markdown override path.
8. **`CellEditorProps`** (`cell-registry.ts:90`): add `availableColumnTypes?:
   Record<string, DataType>`; populate it at `NotebookRenderer.tsx:833` alongside
   the existing `availableColumns` line (Design §6). **`components/CellEditor.tsx`**:
   add the same field to its own local `CellEditorProps` interface (lines 11-25),
   destructure it, and forward it to `<meta.EditorComponent>` (line 161) alongside
   `availableColumns` — without this, `TableCellEditor`/`TransposedTableCellEditor`
   never receive it. Then thread it through `TableCellEditor`/
   `TransposedTableCellEditor` into `OverrideEditor`'s new prop of the same name.
   **Out of scope**: `HorizontalGroupCell.tsx`'s `ChildEditorView` (Current State
   above) is a second forwarding point into `meta.EditorComponent`, reached via
   `HgEditorPanel` rather than `CellEditor`, but `HgEditorPanelProps` doesn't carry
   `availableColumns` today, so an HG-nested Table/Transposed Table cell's editor
   never receives column names at all — a pre-existing gap this plan doesn't fix.
   Threading `availableColumnTypes` through that already-broken path is not part of
   this plan; the "Render as" toggle stays unreachable for HG-nested table cells,
   same as it is for `availableColumns`-dependent behavior there today.
9. **`components/OverrideEditor.tsx`**: accept the new `availableColumnTypes` prop;
   in each override card, when the selected column `isHistogramStructType` **or**
   the card's own `kind === 'histogram'` already (Design §6 — falls back to the
   stored `kind` whenever `availableColumnTypes` is empty or missing the column, so
   a saved histogram card never silently reverts to a plain Format textarea), render
   the "Render as" toggle (Markdown / Histogram) above the Format field; when
   Histogram is selected, swap the Format textarea for a swatch-picker row (six
   colormap swatches from `buildColormapPreviewGradient` + one custom-color swatch
   backed by `<input type="color">`, plus the leading Default swatch, active
   swatch ring-highlighted) + live bar preview, per `option-b-cell-editor.html`.
   `handleAddOverride`'s seeded template (`[Link](...)`) only applies when the
   newly added column isn't histogram-typed; for a histogram column, default the
   new entry to `kind: 'histogram'` with no `histogramColor` set (i.e. the
   Default swatch starts ring-highlighted, until the user picks another one — and
   clicking Default again at any point returns to this same unset state). Since
   `format` is now optional (Design §2), guard
   its two remaining consumers here: `validateFormatMacros(override.format ?? '',
   …)` (line 33) and the Format `<textarea value={override.format ?? ''}>` (line
   147), so a `kind: 'histogram'` card with no `format` doesn't throw off
   validation or flip the textarea uncontrolled. Additionally, gate the
   `validationWarnings` computation (the `validateFormatMacros` call itself) and
   both amber warning-block renders ("Unknown macro" / "Unknown column",
   currently siblings of the Format textarea) on the card's effective kind being
   `'markdown'` — the same stored-`kind`-defaulting-to-`'markdown'` check the
   toggle uses. Without this, a `kind: 'histogram'` card that still carries a
   stale template from an earlier Markdown stint (Design §2/§6 both preserve
   rather than clear `format` in different cases) would keep computing and
   showing warnings about a field the user can no longer see or edit.
   `handleColumnChange`
   (lines 62-69) re-derives `kind` (and resets `format`/`histogramColor` as
   applicable) from the newly selected column's type per Design §2, only when
   that type is present in `availableColumnTypes` — otherwise it leaves
   `kind`/`format`/`histogramColor` untouched and only updates `column`. The toggle's
   own `onClick` handler writes `kind` plus the matching `format`/`histogramColor`
   side effects per Design §6's Editor UI rules: to Markdown, seed `format` with
   `handleAddOverride`'s template only if `format` is currently unset, and clear
   `histogramColor`; to Histogram, leave both `format` and `histogramColor`
   untouched.
10. **`TableRenderer.tsx`** (`lib/screen-renderers/TableRenderer.tsx`, the screens
    table — a second `TableBody`/`OverrideEditor` host outside the notebook cell
    editor pipeline): build an `availableColumnTypes` map next to the existing
    `availableColumns` line (`Object.fromEntries(table.schema.fields.map(f =>
    [f.name, f.type]))`, line 203) and pass it to its `<OverrideEditor>` call
    (~lines 355-359) alongside `availableColumns`. It already passes
    `tableConfig.overrides` to `TableBody` (line 408), so it picks up step 6's
    render-selection switch for free — this step only wires up the editor half.
11. **`TableCell.tsx`**: pass `availableColumnTypes` through to `OverrideEditor`.
12. **`TransposedTableCell.tsx`**: pass `availableColumnTypes` through to
    `OverrideEditor`, same as above. It does **not** use `TableBody`, so step 6's
    change doesn't reach it — separately, change its own `overrideMap`
    (`useMemo`, line 42) from `Map<string, string>` (column → `format`) to
    `Map<string, ColumnOverride>` (column → full entry), and apply Design §3's
    three-way switch at its per-cell render site (lines 122-134), replacing the
    current binary `overrideMap.has(row.name) ? <OverrideCell .../> :
    formatCell(...)` — mirroring the `TableBody` change exactly, just against
    `row.name`/`row.type`/`originalRows[colIdx]` instead of `col.name`/`col.type`/
    `row`. Per Design §3's memoization, call `isHistogramStructType(row.type)`
    once per row, hoisted above the `row.values.map`, rather than once per value.
13. **Docs**: `mkdocs/docs/web-app/notebooks/cell-types.md` — add a subsection under
    both Table (`:694-746`) and Transposed Table (`:749-780`) describing the
    automatic histogram rendering (trigger condition: column type, not config), the
    median calculation, the bucket tooltip, the Overrides panel's "Render as:
    Histogram" mode (a swatch row — six colormap gradient swatches plus one
    custom-color swatch, not a typed field), and the Markdown-override debugging
    trick. Cross-link to the
    existing `make_histogram()`/`quantile_from_histogram()`/`color_scale()`/
    `lerp_color()` docs in `functions-reference.md`. Also update the `overrides`
    option-table row in both sections — `:713` (Table) and `:762` (Transposed
    Table) — from `{ column, format }` to `{ column, kind?, format?,
    histogramColor? }` to match Design §2. **`variables.md`**: add a one-line
    cross-reference from the existing "Column Format Overrides" section
    (`mkdocs/docs/web-app/notebooks/variables.md:102`) — which currently presents
    overrides as markdown-only — to the new "Render as: Histogram" documentation
    in `cell-types.md`, noting that a histogram-typed column's override card
    offers a second, non-markdown render mode.
14. **`CHANGELOG.md`**: add one bullet under `## Unreleased` → `**Web App:**`
    describing the automatic histogram-column rendering for Table/Transposed
    Table cells, the Overrides panel's "Render as: Histogram" mode, the
    `overrides` entry shape change (`{ column, format }` → `{ column, kind?,
    format?, histogramColor? }`), and the accepted behavior change that a saved
    override with a blank `format` now renders an empty override cell instead of
    falling through to the default formatter (Design §3 case 1).
15. **Tests** — see Testing Strategy.

## Files to Modify

| File | Change |
|---|---|
| `analytics-web-app/src/lib/arrow-utils.ts` | new `isHistogramStructType` |
| `analytics-web-app/src/lib/histogram-utils.ts` | new file — quantile/bucket-range math |
| `analytics-web-app/src/lib/histogram-colors.ts` | new file — `resolveHistogramBarColor` (colormap-name vs. literal-color dispatch), `buildColormapPreviewGradient` (swatch CSS gradient, sampled from the same `d3-scale-chromatic` interpolators) |
| `analytics-web-app/src/lib/screen-renderers/macro-substitution.ts` | `formatArrowValue` histogram-struct branch |
| `analytics-web-app/src/components/HistogramCell.tsx` | new file — bar chart + median + tooltip |
| `analytics-web-app/src/components/OverrideEditor.tsx` | `availableColumnTypes` prop; per-card "Render as" toggle + Histogram color fields; guard `validateFormatMacros`/`<textarea>` against optional `format` |
| `analytics-web-app/src/components/CellEditor.tsx` | add `availableColumnTypes` to its own local `CellEditorProps`, destructure, forward to `<meta.EditorComponent>` |
| `analytics-web-app/src/lib/screen-renderers/table-utils.tsx` | `ColumnOverride.kind`/`histogram`, `format` optional; `overrideMap` full-entry lookup; `TableBody` render-mode switch; `<OverrideCell format={entry.format ?? ''}>` (`OverrideCellProps.format` itself unchanged, still required `string`) |
| `analytics-web-app/src/lib/screen-renderers/warning-reporter.tsx` | update the stale `{ column, format }` shape comment to match the new `ColumnOverride` shape |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | build `availableColumnTypes` next to existing `availableColumns` (line 203); pass to `OverrideEditor` |
| `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` | thread `availableColumnTypes` to `OverrideEditor` |
| `analytics-web-app/src/lib/screen-renderers/cells/TransposedTableCell.tsx` | thread `availableColumnTypes`; `overrideMap` → `Map<string, ColumnOverride>`; apply Design §3's three-way render switch (lines 122-134) |
| `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` | new `availableColumnTypes` prop on `CellEditorProps` |
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | populate `availableColumnTypes` alongside `availableColumns` |
| `analytics-web-app/package.json` | new dependency: `d3-scale-chromatic@^3.1.0` (+ `@types/d3-scale-chromatic@^3.1.0`) |
| `mkdocs/docs/web-app/notebooks/cell-types.md` | document new default behavior, the Overrides "Render as: Histogram" mode, and the Markdown-debug trick |
| `mkdocs/docs/web-app/notebooks/variables.md` | cross-reference "Column Format Overrides" section to the new "Render as: Histogram" docs |
| `CHANGELOG.md` | `## Unreleased` → `**Web App:**` bullet for the new default histogram rendering, the "Render as: Histogram" override mode, and the `overrides` entry shape change |

`analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` and
`NotebookRenderer.tsx`'s `HgEditorPanel` are **not modified** by this plan: threading
`availableColumnTypes` there would only reach `ChildEditorView`'s
`meta.EditorComponent` forwarding, which is already unreachable today because
`HgEditorPanelProps` never carries `availableColumns` either (Current State /
Implementation Steps §8) — an HG-nested Table/Transposed Table cell's "Render as"
toggle stays out of scope for this plan rather than layering new plumbing onto that
pre-existing gap.

No Rust changes — the SQL/Arrow side is complete already.

## Trade-offs

- **Structural (name+order) detection vs. an Arrow extension-type tag on
  `Histogram`.** Structural detection needs no server-side change and works today;
  an extension type would be more robust against name collisions but requires
  adding Arrow metadata to `make_histogram`'s output type and is a larger, separate
  change to a UDF whose return type is otherwise stable. Structural detection is
  chosen; revisit if a real collision surfaces in practice.
- **Debugging reuses the Markdown override path vs. a dedicated `textColumns`
  toggle.** An earlier draft of this plan proposed a separate `options.textColumns`
  list and a "Show as Text" context-menu item (mirroring "Hide Column"), reasoning
  that a debug flip is a quick reflex better suited to a context menu than a
  multi-field editor. Revised per direction: the *existing* Markdown override path
  already does the job with zero code changes — `$row.col` on a histogram column
  already dumps its fields via Arrow's own `StructRow.toString()` (Current State
  above), not "stringifying uselessly" as an earlier draft of this plan assumed.
  The optional `formatArrowValue` branch (Design §5) only tidies that dump's
  formatting. Either way this is strictly less code than a dedicated toggle (at
  most one function gains one `if`, vs. a new `options` field, two hooks, and two
  context-menu items) and one fewer concept for a user to learn — "Overrides"
  already is the place to change how a column renders, debug view included.
- **`ColumnOverride.kind: 'markdown' | 'histogram'` vs. a separate
  `histogramColors` option/component.** An earlier draft proposed a standalone
  option and a sibling `HistogramColorEditor` component, reasoning that
  `OverrideEditor` validates and renders one thing (a markdown `format` string) and
  a color-mode config doesn't fit that shape without conditionals. Revised per
  direction: one column has one "how does this render" answer, and users look in
  one place — "Overrides" — to find it, regardless of which flavor applies to a
  given column. The cost is that `OverrideEditor`/`ColumnOverride` are no longer
  single-purpose (`kind` now discriminates `format` vs. `histogram`, and
  `handleAddOverride`/`validateFormatMacros` need a histogram-column branch that
  skips markdown-specific logic) — accepted in exchange for not splitting one
  per-column concern across two editor sections.
- **Per-row bar normalization vs. a shared max across the whole column.** Per-row
  chosen because the issue's stated goal is shape/outlier-spotting within a row, not
  magnitude comparison across rows (a "top N by cost" table already sorts by
  magnitude via its own column). Column-wide normalization would also require a
  full-table prepass before rendering any row, adding complexity for a comparison
  the feature isn't optimizing for.
- **Client-side median estimate vs. asking users to add a `quantile_from_histogram`
  SQL column.** Computing the median from fields already on the struct means zero
  required SQL changes for the default behavior to work — matching this plan's goal
  of "default behavior, no config." The estimate is the same interpolation the SQL
  UDF uses, so the two never disagree if a user does add the SQL column for other
  reasons (e.g. sorting by median).
- **`t` fixed to bucket-height ratio vs. a user-supplied per-bucket color
  expression.** A free-form expression (e.g. arbitrary JS) would need either an
  `eval`-like sandbox (a real security surface for a feature this small) or
  extending the existing markdown-template evaluator with a new bucket-scoped
  context distinct from its row-scoped one — both bigger and riskier than the
  problem calls for. Since a `Histogram` struct has exactly one per-bucket signal
  (the count), there's nothing a custom expression could compute that isn't already
  `t`; "custom" is scoped to *which colors*, not *what drives them*.
- **`d3-scale-chromatic` dependency vs. hand-vendoring colormap stops.** The Rust
  side already made this exact call for `color_scale()` — depend on `colorous`
  rather than hand-maintain color tables (`1069_color_scale_udf_plan.md`
  Trade-offs) — for the same reason: a handful of hardcoded RGB stops per colormap
  is a second place the data can drift from the canonical source, with no owner
  keeping it in sync. `d3-scale-chromatic` is small (two ISC-licensed transitive
  deps, `d3-color` and `d3-interpolate`; the package itself is also ISC, not MIT),
  and already the standard JS source for these exact colormaps. Flagged as an Open
  Question rather than silently added, since it's a new runtime dependency someone
  should sign off on, however small. This reasoning applies equally to the swatch
  previews in Design §6: `buildColormapPreviewGradient` samples the same
  interpolators rather than hand-maintaining a second, separately-sourced gradient
  table that could drift from the colors the bars actually render.
- **Single `histogramColor: string` (colormap name or literal color) vs. a
  `mode: 'colormap' | 'custom'` selector with `colormapName`/`customStops`.** An
  earlier draft kept the two cases as explicit, separately-shaped fields — closer
  to how the config would look if it were strongly typed against a discriminated
  union — plus a "Custom" 2+ stop gradient (mirroring `lerp_color(c1, c2, t)`) for
  users who wanted something between "one flat color" and "a named colormap".
  Revised per direction to a single string: which case applies is unambiguous from
  the value alone (one of six known names, or not), so a mode selector was
  asking the user to declare something the input already implies. Custom
  multi-stop gradients are dropped, not just hidden — "a flat color, or a named
  colormap" covers the two things people actually reach for (recolor the bars;
  or get a heat-style gradient), and the multi-stop case was speculative scope
  beyond what was asked. If a real need for custom gradients surfaces later,
  `histogramColor` can grow a `linear-gradient(...)`-flavored literal-string case
  without a breaking change to the field itself.
- **Swatch picker vs. a free-text Color field.** A first pass at the editor UI used
  a plain text input for `histogramColor`, backed by a help-text paragraph
  spelling out the six colormap names. Revised per direction ("I should not have
  to type them or to read the block of text... don't tell, show"): typing a
  colormap name correctly requires already knowing/remembering it, and a prose
  paragraph is the "tell" the feedback rejected. The swatch row shows all six
  colormaps as actual small gradients — recognizable and clickable, no memorization
  or reading required — plus one native-color-picker swatch for a custom flat
  color, so *no* value in `histogramColor` ever has to be typed by hand. The data
  model (Design §2, a single string) is unchanged; only the input widget is.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` — Table and Transposed Table
  sections: document automatic histogram-column rendering, median calculation
  ("estimated client-side, matches `quantile_from_histogram(h, 0.5)`"), bucket
  tooltip content, the Overrides panel's "Render as: Histogram" option (a swatch
  row, not a typed field — six colormap gradient swatches, one per name
  `color_scale()` also supports, plus one custom-color swatch backed by a native
  color picker; clicking a swatch is the only way to set the color, no value is
  ever typed by hand), and the debugging trick (Markdown override,
  `$row.col` on a histogram column dumps its raw fields). Update the `overrides`
  option-table row in both sections (`:713`, `:762`) from `{ column, format }` to
  `{ column, kind?, format?, histogramColor? }`.
- `mkdocs/docs/web-app/notebooks/variables.md` — add a one-line cross-reference
  from the existing "Column Format Overrides" section (`:102`, which currently
  presents overrides as markdown-only) to the new "Render as: Histogram"
  documentation in `cell-types.md`.
- No changes needed to `functions-reference.md` — the SQL functions this feature
  consumes are already documented there.

## Dependencies

- New: `d3-scale-chromatic@^3.1.0` (ISC license; two transitive deps, `d3-color`
  and `d3-interpolate`, both also ISC) — client-side named colormaps (`viridis`,
  `magma`, `plasma`, `inferno`, `cividis`, `turbo`) for when `histogramColor`
  matches one of those names. Ships no types, so also add
  `@types/d3-scale-chromatic@^3.1.0`. Approved (Open Questions §4).

## Testing Strategy

- **Histogram fixtures must use explicit `UInt64`/`List<UInt64>` types.** Every
  test below that builds a `Histogram`-typed Arrow value (this file's
  `histogram-utils.test.ts`, plus `table-utils.test.tsx`, `HistogramCell.test.tsx`,
  and `macro-substitution.test.ts`) has to construct `count` and `bins` with
  explicit field types — `new Uint64()` / a `List` of `Uint64` — the same way
  existing fixtures already do for other typed columns (`vectorFromArray(...,
  new Utf8View())` in `arrow-ipc-fixtures.ts:213`, `new BinaryView()` in
  `arrow-ipc-fixtures.ts:219` and `table-utils.test.tsx:1097`, `new Float64()` in
  `table-utils.test.tsx:738,758,781`). A naive `vectorFromArray([{count: 5, bins: [1,
  2, 3]}])` infers `Float64`/`List<Float64>` from the plain JS numbers, which
  still satisfies `isHistogramStructType` (Design §1 only checks field
  names/order and that the last field is a `List`) but never produces a single
  `bigint` — silently skipping the one runtime hazard Design §4 calls out (the
  `bigint`/`Vector` read boundary `toHistogramValue` exists to normalize). Factor
  this into one shared fixture helper, `makeHistogramVector`, in a new
  `src/lib/screen-renderers/__tests__/histogram-fixtures.ts` — next to its
  consumers, rather than in `arrow-ipc-fixtures.ts` (`src/lib/__tests__/`, a
  different directory whose existing exports are IPC byte-stream builders used
  only within that same folder) — reused by all four test files rather than
  duplicated per file.
- `arrow-utils.test.ts`: `isHistogramStructType` — true for a struct built with
  the exact field/type shape; false for a struct missing a field, with fields out
  of order, with an extra field, or with a non-`List` `bins` field; false for
  unrelated Struct/List/primitive columns.
- New `histogram-utils.test.ts`: `toHistogramValue` against the shared
  `UInt64`/`List<UInt64>` fixture — asserts `count` (a `bigint` on the raw
  struct) decodes to a plain `number`, and `bins` (a `Vector<bigint>` with no
  numeric index signature) decodes to a plain `number[]` via `Array.from`, not
  `[object BigInt]` or a `Vector` instance. `estimateHistogramQuantile` against
  hand-computed expectations (mirror a couple of cases from
  `expand_histogram_tests.rs` / `histogram_runtime_bounds_tests.rs` for
  cross-checking against the Rust UDF's behavior).
- New `histogram-colors.test.ts`: `resolveHistogramBarColor` — each of the six
  colormap names dispatches to its `d3-scale-chromatic` interpolator and varies
  with `t`; an unrecognized string (hex color, CSS name) is returned unchanged
  regardless of `t`; `undefined` → `var(--chart-line)`.
- New `macro-substitution.test.ts` (no test file for this module exists today —
  `formatArrowValue` is currently covered indirectly via
  `__tests__/notebook-utils.test.ts`; add the histogram case there instead if
  splitting out a dedicated file isn't warranted): `formatArrowValue` renders a
  histogram-struct value as its compact field dump
  (`{start:..., end:..., count:..., bins:[...]}`),
  asserted against Arrow's current `StructRow.toString()` output as the baseline
  (what `String(value)` already produces without this change) rather than
  `[object Object]`, confirming the new branch is a formatting improvement, not a
  functional fix; non-histogram struct/list/primitive values unaffected.
- `table-utils.test.tsx`: `TableBody` renders `HistogramCell` for a histogram-typed
  column by default and when `kind: 'histogram'`; renders `OverrideCell` when
  `kind: 'markdown'` (or unset) even on a histogram column, and that its resolved
  markdown text reflects the new `formatArrowValue` behavior when the template
  references the column; `HistogramCell` receives the resolved `histogramColor`
  from a `kind: 'histogram'` override; null histogram value renders `-`.
- New `HistogramCell.test.tsx` (or colocated in `__tests__/`): bar count always
  matches `bins.length` (no downsampling, including a large bin count); hover on a
  bucket's wrapper surfaces the correct range/count/percentage; median label
  matches `estimateHistogramQuantile`; bar fill color matches
  `resolveHistogramBarColor` for a colormap name, a literal color, and no `color`
  prop at all; a 0-count bucket's bar has an inline/computed `min-height` of `2px`
  (not `0px` and not a `%`-only height, since jsdom performs no layout and can't
  assert a rendered pixel size otherwise); that bucket's full-height hover wrapper
  still fires `onMouseEnter`/`onMouseLeave` and shows the correct range/count in
  the tooltip, confirming the hover target is the wrapper and not the (visually
  thin) bar itself.
- New `OverrideEditor.test.tsx` (no test file exists for this component today):
  "Render as" toggle appears only when the selected
  column is histogram-typed (via a mock `availableColumnTypes`); switching to
  Histogram hides the Format field and shows the swatch row; column dropdown for
  a new override still lists all columns (the toggle is per-card, not a dropdown
  filter — this component still serves every column, histogram or not); clicking a
  colormap swatch sets `histogramColor` to that name and ring-highlights it;
  changing the custom-color swatch's `<input type="color">` sets `histogramColor`
  to the picked hex and moves the highlight to the custom swatch; the Default
  swatch is ring-highlighted when `histogramColor` is unset (a brand-new
  histogram card); clicking the Default swatch after a colormap or custom color
  has been picked sets `histogramColor` back to `undefined`, moves the highlight
  back to Default, and leaves `format` untouched.
- Manual: `yarn dev`, build a notebook Table cell against
  `SELECT call_site, make_histogram(0, 50, 24, duration_ms) AS dist FROM ...
  GROUP BY call_site`, verify bars/median/tooltip render by default with no
  override configured; in Overrides, add a card on the histogram column, switch
  "Render as" to Histogram, click the `viridis` swatch and then pick a custom
  color via the color-picker swatch, verify the cell's bars update to match each
  and the active swatch highlight follows; switch that same card back to Markdown
  with Format `$row.dist`, verify the cell now shows the raw struct dump instead
  of the chart; repeat with a Transposed Table cell (single-row query).
- `yarn lint` / `yarn type-check` / `yarn test` from `analytics-web-app/`.

## Open Questions

1. ~~**Median encoding**~~ **Resolved: Option B (tick-mark), locked in.**
2. ~~**Per-bucket color scale**~~ **Resolved: implemented as the Overrides panel's
   "Render as: Histogram" option** (Design §6) — a single `histogramColor` string
   that's either a named colormap (matching `color_scale()`) driven by each
   bucket's own height ratio, or a literal CSS color applied flat to every bar.
   Default stays `var(--chart-line)` when no override is configured. (A
   multi-stop custom-gradient mode was considered and dropped — see Trade-offs.)
3. ~~**Debug view mechanism**~~ **Resolved: reuse the Markdown override path**
   (Design §5) — no dedicated toggle, `options` field, or context-menu item.
4. ~~**`d3-scale-chromatic` dependency**~~ **Resolved: approved.** Needed for the
   colormap-name case of `histogramColor` (Design §6); ISC license, two
   transitive deps (`d3-color`, `d3-interpolate`, both also ISC) — see
   Dependencies and Implementation Steps §3.
