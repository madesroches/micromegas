# Map Event Detail Markdown Template Plan

Addresses [issue #1053](https://github.com/madesroches/micromegas/issues/1053).

## Overview

Replace the Map cell's generic key/value event-detail dump with a
Markdown template authored in the cell editor. The template is rendered
through the existing notebook macro pipeline (`substituteMacros`), with
one new substitution source: the columns of the selected event row.
Every column the SQL `SELECT` produces is addressable as `$column` from
the template; `$from`/`$to`, `$variable`, `$variable.column`,
`$cell[N].column`, and `$cell.selected.column` keep their existing
behavior verbatim.

The substituted Markdown is rendered through `react-markdown` (already
in the app via `MarkdownCell`). Substitution uses `substituteMacros`
unchanged — same SQL-escape behavior MarkdownCell already relies on.
Raw HTML stays disabled (react-markdown default), and internal links
route through `AppLink` so the `/process?...` affordance keeps working
under base paths.

## Current State

The event detail panel is hard-coded:
[`analytics-web-app/src/components/map/EventDetailPanel.tsx`](../analytics-web-app/src/components/map/EventDetailPanel.tsx).
It renders a fixed Time row, then iterates `Object.entries(event.properties)`
(`EventDetailPanel.tsx:42-49`), then a Coordinates row, then an
`AppLink` to `/process?process_id=…` when `event.processId` is set.

`MapEvent` (`MapViewer.tsx:6-15`) carries `properties: Record<string, string>`,
populated by `arrowTableToMapEvents` (`MapCell.tsx:36-69`) which splits
columns: `time`, `process_id`, `x`, `y`, `z` go to typed `MapEvent`
fields and *everything else* lands in `properties`. The `RESERVED_COLUMNS`
set lives at `MapCell.tsx:34`.

Macro substitution is in `notebook-utils.ts`:

- `substituteMacros` (`notebook-utils.ts:309-384`) handles `$from`/`$to`,
  `$cell[N].col`, `$cell.selected.col`, `$var.col`, `$var`. All
  substituted values are SQL-escaped through `escapeSqlValue`.
- `validateMacros` (`notebook-utils.ts:398-484`) walks the same
  patterns and returns errors for unknown references.
- Regex patterns are factories at the top of the file
  (`cellRefRegex`, `selectedRefRegex`, `dottedVarRegex`,
  `simpleVarRegex`) to keep substitution and validation in sync.

`MarkdownCell` (`cells/MarkdownCell.tsx`) already substitutes and
renders Markdown — it calls `substituteMacros` then passes the result
into `<Markdown remarkPlugins={[remarkGfm]}>`. The map detail panel
adopts the same pattern verbatim, inheriting the same `''` apostrophe-
doubling wart for values that contain single quotes. Fixing that is
out of scope; if it ever needs fixing, the change should land in
MarkdownCell first and the panel will follow.

The Map cell stores ephemeral selection in local state (`selectedEvent`
at `MapCell.tsx:151`); it does not currently wire `onSelectionChange`,
so `cellSelections[mapCellName]` from `cell-registry.ts:54-59` is
absent for map cells. Other cells therefore cannot reference a map
cell's selection today — that wiring is enabled by this plan since the
template needs the same data.

Dependencies present in `analytics-web-app/package.json`:
`react-markdown@^10.1.0`, `remark-gfm@^4.0.1`. No sanitization
package; not needed because react-markdown v10 treats raw HTML as text
by default (no `rehype-raw`).

## Design

### Data model

Switch `MapEvent` from the derived `properties` map to the full row,
plus the typed fields the renderer still needs to place markers:

```ts
export interface MapEvent {
  /** Stable per-event id, derived as
      `${row['process_id'] ?? 'unknown'}-${rowIndex}` to match the
      current scheme so React keys stay stable across the rewrite. */
  id: string
  /** Optional — present when the query produced a `time` column.
      Not load-bearing for the current renderer (markers are placed
      from x/y/z); kept for future animated-overlay work and as a
      convenience for the template via $time. */
  time?: Date
  x: number
  y: number
  z: number
  /** Full row from the SQL result, as strings for substitution.
      Only non-null columns are present — null/undefined values are
      omitted (no `''` coercion) so unresolved `$col` stays literal
      in the rendered template rather than silently becoming empty. */
  row: Record<string, string>
}
```

`processId` is **removed** from `MapEvent` in this PR: its only
consumer is the hard-coded `View Process Logs` link, which the
rewrite deletes. Template authors read `$process_id` from `row`
instead. Dropping the field avoids leaving a typed accessor with
zero call sites.

`arrowTableToMapEvents` (`MapCell.tsx:36-69`) is simplified: no
`RESERVED_COLUMNS` split, every column is coerced to string (timestamps
to RFC3339 via `timestampToDate(...).toISOString()`) and stored in
`row`. The typed `x`/`y`/`z` fields are derived from the same columns
for marker placement; `time` is derived only when a `time` column
exists in the result (left `undefined` otherwise — the renderer does
not require it).

### Macro substitution

The selected event's `row` is exposed to `substituteMacros` as a fresh
substitution source alongside variables. Two cleanest options:

1. **Inline columns into variables**: build a synthesized
   `variables` map by spreading the row's columns over the existing
   variables map, columns winning collisions. Reuse `substituteMacros`
   unchanged.
2. **Add a `rowColumns` parameter**: thread a new parameter through
   `substituteMacros`/`validateMacros` and resolve `$bareName` against
   it before falling back to variables.

**Choose option 1.** The collision policy matches what authors expect
("`$x` in a Map template means the selected row's `x` column") and
keeps `substituteMacros` shape stable for every other caller. The
escape change (next) is the only thing that needs to land in the
shared function.

### Substitution

`substituteMacros` is unchanged in signature and behavior — the panel
reuses it exactly as `MarkdownCell` does today. Values flow through
the existing SQL-escape path, which is fine for prose in Markdown.
Raw HTML stays disabled in react-markdown (no `rehype-raw`) and
URL-protocol abuse (`[link](javascript:…)`) is filtered by
react-markdown's default `urlTransform`, which strips dangerous
schemes. Authored Markdown syntax in column values *will* render —
same trade-off MarkdownCell has today; authors who don't want that
should wrap suspect columns in code spans.

`validateMacros` is unchanged in shape — it only emits warnings,
doesn't escape anything. It already accepts variables and resolves
bare references against them; with option 1, the editor passes the
merged map (row columns ∪ variables) so unknown-bareword warnings
only fire on truly absent names.

`findUnresolvedSelectionMacro` is unchanged.

### Self-reference and `cellSelections` wiring

The Map cell starts publishing its selection to `cellSelections`. Wiring:

- `MapCell` accepts the standard `onSelectionChange` callback from
  `CellRendererProps`. When the user clicks a marker, the cell calls
  both `setSelectedEvent(event)` (local) and
  `onSelectionChange?.(event ? event.row : null)`. (Use a ref like
  `TableCell.tsx:42-43` to avoid re-render loops.)
- Selection is intrinsic to the map cell (click a marker = select an
  event), not an authoring choice — so map cells always run in
  single-selection mode. Today `NotebookRenderer.tsx:605` reads
  `options.selectionMode` per cell and falls back to `'none'`; only
  `'single'` wires `onSelectionChange` (`NotebookRenderer.tsx:631-634`).
  To keep map cells from being opt-in, add a
  `defaultSelectionMode?: 'none' | 'single'` field to
  `CellTypeMetadata` and have `NotebookRenderer` use it as the
  fallback when `options.selectionMode` is absent:

  ```ts
  const selectionMode =
    ((cell as QueryCellConfig).options?.selectionMode as 'none' | 'single' | undefined)
    ?? meta.defaultSelectionMode
    ?? 'none'
  ```

  `mapMetadata.defaultSelectionMode = 'single'`. The same change must
  land in `HorizontalGroupCell.tsx:405` (children-of-hg path) to keep
  map cells inside HG groups consistent. Every other cell's default
  stays `'none'`.

This makes `$mapcell.selected.col` work from other cells without
special-casing — same plumbing the table/log cells use. Inside the
map cell's own template, `$col` is the local shortcut for the same
data.

### Rendering

Replace the body of `EventDetailPanel`:

```tsx
interface EventDetailPanelProps {
  event: MapEvent
  template: string
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  cellResults: Record<string, Table>
  cellSelections: Record<string, Record<string, unknown>>
  onClose: () => void
}

export function EventDetailPanel({ event, template, variables, timeRange, cellResults, cellSelections, onClose }: EventDetailPanelProps) {
  const rendered = useMemo(() => {
    const mergedVars: Record<string, VariableValue> = { ...variables, ...event.row }
    return substituteMacros(template, mergedVars, timeRange, cellResults, cellSelections)
  }, [template, event, variables, timeRange, cellResults, cellSelections])

  return (
    <div className="absolute bottom-4 left-4 w-80 max-h-[60%] overflow-y-auto bg-app-panel border border-theme-border rounded-lg shadow-lg z-10">
      <div className="flex items-center justify-between px-4 py-3 border-b border-theme-border">
        <h3 className="text-sm font-semibold text-theme-text-primary">Event Details</h3>
        <button onClick={onClose} ...><X className="w-4 h-4 text-theme-text-muted" /></button>
      </div>
      <div className="prose prose-invert prose-sm max-w-none p-4 ...">
        <Markdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
          {rendered}
        </Markdown>
      </div>
    </div>
  )
}
```

The `MarkdownLink` component (new, colocated in `EventDetailPanel.tsx`
or `analytics-web-app/src/components/MarkdownLink.tsx` so MarkdownCell
can adopt it later):

```tsx
// `node` is the AST node react-markdown passes to component overrides;
// it's not a valid DOM/Link prop, so destructure-and-drop it before the
// spread, otherwise React warns about an unknown prop.
function MarkdownLink({
  href,
  children,
  node: _node,
  ...rest
}: AnchorHTMLAttributes<HTMLAnchorElement> & { node?: unknown }) {
  if (!href) return <a {...rest}>{children}</a>
  if (/^https?:/i.test(href) || href.startsWith('//') || href.startsWith('mailto:')) {
    return <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>{children}</a>
  }
  return <AppLink href={href} {...rest}>{children}</AppLink>
}
```

This single component lets a template author write
`[View process logs](/process?process_id=$process_id)` and get the
base-path-aware client-side route that the hard-coded "View Process
Logs" link uses today.

### Default template

Stored as a constant alongside `DEFAULT_SQL` in `notebook-utils.ts`:

```ts
export const DEFAULT_MAP_DETAIL_TEMPLATE = `### Event

**Location:** ($x, $y, $z)
`
```

Rationale: covers `x`, `y`, `z` — the only columns the documented
map query contract requires. (Aside: the current `arrowTableToMapEvents`
does not strictly *reject* rows with missing `x`/`y`/`z`; it defaults
the value to `0` via `parseFloat(String(row.x ?? '0'))` and only drops
rows whose values are non-numeric NaN. The new implementation keeps
that defaulting for the typed fields — see Phase 2 — so a non-conforming
query still places markers at the origin rather than silently emptying.)
`time` is not currently load-bearing for the renderer — markers are
placed purely from coordinates — and the query contract for `time`
is being relaxed in this plan (see "Relaxing the `time` requirement");
including it in the default template would lock in a column that is
becoming optional. Anything beyond `x`/`y`/`z` is query-specific and
authors extend the template to reference their own columns; the
cell-types docs include a `[View process logs](/process?process_id=$process_id)`
example so the internal-link affordance is discoverable without
baking an optional-column reference into the default. Values are
emitted bare (no code-span backticks) so SQL-escape artifacts in any
column value render as text rather than verbatim inside a code span.

### Migration / legacy notebooks

The detail template is **required at render time**, but old configs
won't have it. Two mechanically equivalent options:

1. **Lazy default in the renderer**: read
   `options?.detailTemplate ?? DEFAULT_MAP_DETAIL_TEMPLATE` at render
   time. Old configs keep working; the on-disk config is only
   rewritten when the author opens the editor and edits the field.
2. **Eager migration on load**: when the notebook loads, walk all map
   cells and stamp `options.detailTemplate = DEFAULT_MAP_DETAIL_TEMPLATE`
   if missing. Persisted on next save.

**Choose option 1.** Notebook configs already carry per-cell defaults
implicitly (no `markerColor` → render with `#bf360c`); adding an
eager-migration pass would be a new pattern and would dirty old
notebooks for users who never open the map editor. Option 1 also
sidesteps any test fixture rewrite. `createDefaultConfig` for new
cells *does* seed the template, so new notebooks save the explicit
value.

### Editor UX

In `MapCellEditor` (`MapCell.tsx:268-384`):

- Add a new section "Detail Template (Markdown)" below "Map Options"
  (above the `AvailableVariablesPanel`). Reuse `SyntaxEditor` with
  `language="markdown"` and `minHeight="160px"`.
- A second validation block under the editor shows
  `validateMacros(template, mergedVars, cellResults, cellSelections)`
  errors, where `mergedVars` is `{ ...variables, ...syntheticColumnVars }`.
  `syntheticColumnVars` is built from the cell's most recent result
  table (one synthetic empty-string entry per column name from
  `data[0]?.schema.fields`). This makes `$columnFromQuery` not flag as
  "Unknown variable" at edit time even though the row hasn't been
  selected yet.
- The existing `AvailableVariablesPanel` already lists
  `$cell.selected.col` for selected upstream cells. Add an
  "Available columns" section above it (or merge into it) that lists
  the column names produced by the most recent run of *this* cell.
  Cleanest: extend `AvailableVariablesPanel` with an optional
  `localRowColumns?: string[]` prop. When present, it renders a
  dedicated "Selected event columns" block listing each column as
  `$colname`.

### Type changes

- `MapEvent.properties: Record<string, string>` →
  `MapEvent.row: Record<string, string>`.
- `MapEvent.processId: string` → **removed**. The only consumer is the
  hard-coded `View Process Logs` link, which is being deleted in this
  PR; template authors get `process_id` via `$process_id` (i.e.
  `row['process_id']`). `MapEvent.id` continues to be derived from
  `row['process_id']` directly (`${row['process_id'] ?? 'unknown'}-${i}`).
- `MapEvent.time: Date` → `MapEvent.time?: Date`. Today the cell
  silently substitutes `new Date()` when the query omits `time`; the
  optional form makes the missing-column case visible rather than
  papering over it with a misleading "now" timestamp. The renderer
  doesn't read `event.time`, so the change is non-breaking for marker
  placement.
- `EventDetailPanelProps` gains `template`, `variables`, `timeRange`,
  `cellResults`, `cellSelections`. The panel becomes a substitution+render
  surface, no longer a data-formatting component.
- `substituteMacros` is unchanged. The panel relies on the same
  SQL-escape behavior MarkdownCell uses today.

### What gets removed

- `RESERVED_COLUMNS` and the split logic at `MapCell.tsx:34, 50-56`.
- The generic-rows loop in `EventDetailPanel.tsx:42-49`.
- The hard-coded Coordinates row at `EventDetailPanel.tsx:51-58`.
- The hard-coded `View Process Logs` link at `EventDetailPanel.tsx:60-69`
  (the default template restores it as authored Markdown).

## Implementation Steps

### Phase 1 — substitution core

1. **`analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`**
   - Add `export const DEFAULT_MAP_DETAIL_TEMPLATE`.
   - Add `export` to the existing `formatArrowValue` helper (currently
     a private function at line 267). Phase 2's `arrowTableToMapEvents`
     rewrite reuses it for timestamp formatting; exporting is cleaner
     than duplicating the `isTimeType` + `timestampToDate` branch
     inline.
   - No changes to `substituteMacros` / `validateMacros` signatures;
     the panel reuses them as-is (same pattern as MarkdownCell).

### Phase 2 — data path

2. **`analytics-web-app/src/components/map/MapViewer.tsx`**
   - Change `MapEvent.properties: Record<string, string>` to
     `MapEvent.row: Record<string, string>`.
3. **`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`**
   - Delete `RESERVED_COLUMNS` and the split.
   - Rewrite `arrowTableToMapEvents`: every column to `row` as a
     formatted string. Iterate `table.schema.fields` so the column's
     `DataType` is in scope; call the now-exported `formatArrowValue(value,
     field.type)` helper from `notebook-utils.ts` (`isTimeType(field.type)`
     → `timestampToDate(value, field.type)?.toISOString()`, else
     `String(value)`). Passing `DataType` is required for BigInt
     timestamp columns, where `timestampToDate` without it falls back
     to "assume nanoseconds" and would mis-convert non-nanosecond
     units. **No fallbacks**: when a column's value is `null`/`undefined`,
     omit the key from `row` entirely (don't coerce to `''`). This
     leaves any `$col` reference unresolved in the template — matching
     the literal "$name stays literal when unknown" rule
     `substituteMacros` already follows for missing variables —
     instead of silently rendering an empty value.
   - The typed `x`/`y`/`z` fields are derived from `row['x']`/`row['y']`/`row['z']`
     using the same `parseFloat(String(row['x'] ?? '0'))` form as
     today (default-to-0 for missing values, drop row only on NaN).
     The typed `time` field is set only when the query produced a
     `time` column (use the Arrow schema, not the stringified
     `row['time']`, to detect presence and parse from the raw value
     via `timestampToDate`); leave it `undefined` otherwise. **Remove
     the `time ?? new Date()` fallback** at `MapCell.tsx:60` — it
     manufactures a misleading "now" value when the query has no
     `time` column and obscures the missing-data case. The `id` field
     stays `${row['process_id'] ?? 'unknown'}-${i}`, matching the
     current scheme (no typed `processId` field needed). Row
     construction is order-preserving so the editor's "Available
     columns" list reflects SELECT order.
   - Wire `onSelectionChange`: stash it in a ref
     (`TableCell.tsx:42-43` pattern) and, when the user clicks a
     marker, call `onSelectionChangeRef.current?.(event ? event.row : null)`
     alongside the existing local `setSelectedEvent`. Also call with
     `null` when `onPointerMissed` clears the selection.
   - Clear the selection on data change: mirror `TableCell.tsx:46-49` —
     when the source `data[0]` reference changes (re-execution), call
     `setSelectedEvent(null)` and `onSelectionChangeRef.current?.(null)`
     so a stale selection from a prior query doesn't keep publishing
     ghost rows to `cellSelections` after the events behind it are gone.
   - In `MapCell` renderer, read
     `template = options?.detailTemplate as string | undefined ?? DEFAULT_MAP_DETAIL_TEMPLATE`.
   - Pass `template`, `variables`, `timeRange`, `cellResults`,
     `cellSelections` to `<EventDetailPanel>`. These come from
     `CellRendererProps` — currently the Map renderer only destructures
     `data`, `status`, `options`; extend the destructure.
   - In `mapMetadata`, add `defaultSelectionMode: 'single'`.
   - In `mapMetadata.createDefaultConfig`, seed
     `options: { ..., detailTemplate: DEFAULT_MAP_DETAIL_TEMPLATE }`.

3a. **`analytics-web-app/src/lib/screen-renderers/cell-registry.ts`**
   - Add `readonly defaultSelectionMode?: 'none' | 'single'` to
     `CellTypeMetadata`.

3b. **`analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx`**
   - At line 605, change the selection-mode lookup to fall back to
     `meta.defaultSelectionMode` before `'none'` (snippet in the
     "Self-reference and `cellSelections` wiring" section above).

3c. **`analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx`**
   - At line 405, apply the same fallback so a map cell nested inside
     an HG group also gets `onSelectionChange` wired without an
     explicit `options.selectionMode`. Unlike the NotebookRenderer
     path, `meta` is not in scope here — add a `getCellTypeMetadata`
     lookup before the assignment:

     ```ts
     const childMeta = getCellTypeMetadata(child.type)
     const childSelectionMode =
       ((child as import('../notebook-types').QueryCellConfig).options?.selectionMode as 'none' | 'single' | undefined)
       ?? childMeta.defaultSelectionMode
       ?? 'none'
     ```

     `getCellTypeMetadata` is already imported (`HorizontalGroupCell.tsx:35`),
     so no new import is needed.

### Phase 3 — render

4. **`analytics-web-app/src/components/map/EventDetailPanel.tsx`**
   - Rewrite as the substitution+render surface described above.
   - Add `MarkdownLink` component (or import from the new
     `components/MarkdownLink.tsx` if we extract it).
   - Use `prose prose-invert prose-sm` classes consistent with
     `MarkdownCell` for visual continuity.

5. **`analytics-web-app/src/components/MarkdownLink.tsx`** (optional
   extraction)
   - Pulled out only if we want `MarkdownCell` to adopt the same link
     behavior in a follow-up. If not extracting, keep it private in
     `EventDetailPanel.tsx`.

### Phase 4 — editor

6. **`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`** (`MapCellEditor`)
   - Add a "Detail Template (Markdown)" section with `SyntaxEditor`.
   - Read/write `mapConfig.options?.detailTemplate`. Falls back to
     `DEFAULT_MAP_DETAIL_TEMPLATE` for display so old cells get the
     baseline visible in the editor without writing it to disk until
     edited.
   - The cell's own column list is the existing
     `availableColumns?: string[]` prop on `CellEditorProps`
     (`cell-registry.ts:85`), already populated by
     `NotebookRenderer.tsx:789` from
     `cellStates[selectedCell.name]?.data[0]?.schema.fields`.
     `TransposedTableCellEditor` already consumes it
     (`TransposedTableCell.tsx:139`); `MapCellEditor` just destructures
     the same prop. No new plumbing.
   - Build `syntheticColumnVars: Record<string, VariableValue>` by
     mapping each name in `availableColumns ?? []` to an empty string
     and run
     `validateMacros(template, { ...variables, ...syntheticColumnVars }, cellResults, cellSelections)`.
     Empty strings are fine — `validateMacros` only checks presence,
     not value. Surface errors below the editor (same pattern as the
     SQL validation block at `MapCell.tsx:314-320`).

7. **`analytics-web-app/src/components/AvailableVariablesPanel.tsx`**
   - Add optional `localRowColumns?: string[]` prop. When present,
     render a "Selected event columns" section above (or below) the
     variables list, listing `$col` for each entry. No collision
     warning — collision precedence is column-first by design.

### Phase 5 — tests

8. **`analytics-web-app/src/components/map/__tests__/EventDetailPanel.test.tsx`** (new)
   - Renders a basic template with `$time`/`$x`/`$y`/`$z` against a
     synthesized event row. Confirms the substituted values appear.
   - Renders `[View process logs](/process?process_id=$process_id)`
     against a row containing `process_id` and asserts the resulting
     link uses `AppLink`'s base-path-aware href and is a router link,
     not a plain anchor with `target=_blank`.
   - URL safety: a `javascript:` href in the *authored template* is
     stripped by react-markdown's default `urlTransform` and does not
     render as a clickable link.
   - `$from`/`$to` substitution works.
   - Mixed `$var` + `$col` substitution with the column winning a
     name collision.
   - External link (`https://example.com`) renders with
     `target="_blank" rel="noopener"`.

9. **`analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx`** (new
   or extend existing if present)
   - `createDefaultConfig` seeds `detailTemplate`.
   - Legacy config (no `detailTemplate`) renders without error using
     the default template after selecting a marker.
   - `arrowTableToMapEvents` produces `row` with every *non-null*
     column as a string — including `time` as RFC3339 when present,
     `process_id` when present, and any extras. Columns whose value
     is `null` are absent from `row`.
   - A query result that omits the `time` column produces
     `MapEvent.time === undefined` (no `new Date()` fallback) and the
     default template renders without `$time` showing up as a literal.
   - `onSelectionChange` is called with the row on click, with `null`
     on deselect. (May require some R3F/three test mocking; the
     existing map components don't yet have tests so a minimal stub
     of `MapViewer` is acceptable — focus the test on the data path,
     not the WebGL one.)

### Phase 6 — docs

10. **`mkdocs/docs/web-app/notebooks/cell-types.md`** — under the Map
    section (`cell-types.md:512-616`):
    - Update the "Required columns" table (`cell-types.md:523-530`):
      move `time` out, since the implementation no longer requires it.
      Required columns are now `x`, `y`, `z`.
    - Update the "Optional columns" paragraph
      (`cell-types.md:532-539`) — add `time` to the optional list, and
      remove the "key-value properties" sentence; columns are
      addressable from the detail template instead.
    - Add an "Options" row for `detailTemplate` and a new
      "Detail template" subsection describing the macro forms, the
      default template, and an example using `process_id` to build a
      `[View process logs](/process?process_id=$process_id)` link
      (so authors discover the internal-link affordance without it
      being baked into the default).
    - Update the Features list (`cell-types.md:591-598`) — replace
      "Event detail panel with properties and link to process logs"
      with "Event detail panel rendered from a Markdown template with
      macro substitution".

11. **`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`**
    (`MapCell.tsx:199` empty-state message)
    - Change `Query must return columns: time, x, y, z` to
      `Query must return columns: x, y, z` to reflect the relaxed
      contract. (The message already fires only when the result has
      zero rows, so it's actually about no-data more than missing
      columns; that's a separate copy issue not in scope here.)

## Files to Modify

Frontend code:
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/cell-registry.ts` (add `defaultSelectionMode` to `CellTypeMetadata`)
- `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` (line 605 fallback)
- `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` (line 405 fallback)
- `analytics-web-app/src/components/map/MapViewer.tsx`
- `analytics-web-app/src/components/map/EventDetailPanel.tsx`
- `analytics-web-app/src/components/AvailableVariablesPanel.tsx`

Frontend tests:
- `analytics-web-app/src/components/map/__tests__/EventDetailPanel.test.tsx` (new)
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/MapCell.test.tsx` (new)

Docs:
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

**Inlining columns into `variables` vs threading a new parameter.**
Inlining keeps `substituteMacros` shape stable for every other caller
and matches author intent — `$x` in a Map template means "the selected
row's `x` column". Threading a parameter is more explicit but bigger
blast radius (every call site, every test). The escape parameter is
the only signature change worth doing; the column source is
caller-merged.

**No Markdown-escape mode — match MarkdownCell.** A dedicated Markdown
escape (backslash-escaping `*`, `_`, `[`, etc. in substituted values)
was considered. It was rejected because backslash escapes are not
processed inside Markdown code spans, so any template that wraps a
substituted value in backticks (a natural reflex for timestamps and
coordinates) would render visible `\.` and `\-` artifacts. Matching
MarkdownCell's existing behavior — pass `substituteMacros` output
verbatim into react-markdown — keeps the two surfaces consistent and
sidesteps the code-span artifact entirely. The known wart is shared:
column values containing inline Markdown (`**bold**`) will render as
authored; URL-context safety is still handled by react-markdown's
default `urlTransform`. Authors who need raw display can wrap the
column in a code span in the template.

**Lazy default vs eager migration for legacy configs.** Lazy keeps
old notebooks unchanged on disk until intentionally edited and
matches the existing per-option fallback pattern (no `markerColor` →
`#bf360c`). Eager would force a write on every legacy notebook open,
producing churn for users who never customize the panel.

**Drop MapEvent.processId in this PR.** The only consumer is the
hard-coded `View Process Logs` link, which the rewrite removes in the
same diff, so keeping the typed field would leave it with zero call
sites. Template authors read `process_id` via `$process_id` from the
row map.

**MarkdownLink in EventDetailPanel vs shared component.** The
behavior is generally useful (any cell rendering substituted Markdown
benefits from `AppLink`-aware internal links). Keep it inline in this
PR; extract to `components/MarkdownLink.tsx` and adopt in `MarkdownCell`
as a separate change if the appetite materializes.

**Apply Markdown escape to `MarkdownCell` too.** Out of scope — that
cell's current behavior is well-understood and changing its escape
mode is a separable change. The new escape mode is opt-in via the
new option, so MarkdownCell stays bit-identical.

**Sanitizer dependency.** Not adding `rehype-sanitize`. react-markdown
v10 in this app does not enable `rehype-raw`, so HTML in the markdown
input is parsed but rendered as text (per the existing MarkdownCell
behavior). Adding the sanitizer would be a defense-in-depth measure
with no current threat surface; revisit if the project ever pulls in
`rehype-raw`.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` — Map section gets a
  Detail Template subsection plus an options-table row.
- No new top-level pages.
- The existing macros section of the notebook docs (if there is a
  dedicated one — verify under `mkdocs/docs/web-app/notebooks/`) gets
  a one-line note that the Map detail template uses the same macro
  dialect, with column lookup added.

## Testing Strategy

- **Unit**: no changes to `substituteMacros` / `validateMacros` — both are
  reused unchanged, so existing tests apply. The new
  `DEFAULT_MAP_DETAIL_TEMPLATE` constant is exercised by the MapCell tests
  below (legacy-config path).
- **Component**: `EventDetailPanel.test.tsx` covers every macro form
  (`$col`, `$from`/`$to`, `$var`, `$var.col`, `$cell[N].col`,
  `$cell.selected.col`), Markdown injection from a column value, link
  routing (internal → AppLink, external → `target=_blank`), and the
  default-template path on a legacy config.
- **Integration**: `MapCell.test.tsx` covers `arrowTableToMapEvents`
  producing a complete `row`, default config seeding, legacy fallback,
  and the `onSelectionChange` wiring.
- **Manual smoke**: spin up `start_analytics_web.py`, open a notebook
  with an existing Map cell (legacy — no template), confirm the
  default panel renders on marker click; edit the template to drop
  the process-log link and add a custom field, confirm the saved
  notebook re-renders with the change; click a marker on a query that
  defines an additional `event_type` column and confirm `$event_type`
  resolves.

## Relaxing the `time` requirement

`time` moves from required to optional in this PR. The rewrite drops
the `time ?? new Date()` fallback in `arrowTableToMapEvents`, makes
`MapEvent.time` optional, updates the cell-types docs (Required/Optional
tables), and updates the empty-state message at `MapCell.tsx:199`.
Marker placement is purely from coordinates, so this is a non-breaking
implementation change.

Future work (out of scope here): if an animated-overlay feature ever
needs `event.time`, it will need to:
- Reintroduce a non-template code path that depends on `event.time`
  (timeline scrubber, marker fade, etc.) and gate on its presence.
- Re-tighten the contract to "time is required iff the animated
  overlay is enabled," with corresponding docs / empty-state copy.
- Re-add `$time` to the default detail template, since at that point
  every animated-overlay query will define it.
